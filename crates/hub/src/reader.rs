//! Reader sessions: which frames one gets, why one does not open, and the session.

pub(crate) mod remote;

use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::poll_fn;
use std::mem;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use ::home::reader::Next;
use types::channel;
use types::frame::key_set::KeySet;
use types::frame::{Frame, Mask, View};
use types::name::{Name, Selector};
use types::time::Span;

use crate::channel::Channel;
use crate::pending::Pending;
use crate::{Away, End, Ending, State};
use remote::Remote;

/// The credit a complete reader has past the frames it gave back: a fixed window until
/// the hub sizes it from the link.
const WINDOW: u64 = 1 << 20;
/// Frames that [`Reader::next`] gives in a row before it yields once.
const STREAK: u32 = 128;

/// Which frames a reader gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Each live frame, after the commit that holds it. Once the frames that the
    /// reader has not given back (the one it holds and those it has not taken) reach
    /// a window, a later frame waits until the reader gives frames back. A frame that
    /// still waits when the hub releases the next commit with frames of the index is
    /// a miss: the session ends with [`Ended::Behind`] after the frames before it. So
    /// a reader that takes the frames of each commit before the next such commit ends
    /// misses none. Samples that found no room in the buffer never reach it: the
    /// next frame gives them as its [`Received::gap`].
    Complete,
    /// The newest live frame, before its commit.
    Latest,
}

/// What a reader session reads, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The channels it reads: each channel the hub knows at the open whose name the
    /// selector matches.
    pub select: Selector,
    /// Which frames it gets.
    pub mode: Mode,
    /// The subject that opens the reader. A named reader belongs to it: an open by the
    /// same subject and name on the same index takes over the session, and an open by
    /// another subject opens another reader.
    pub subject: Name,
    /// The reader's name, or `None`. A named reader has at most one session on each
    /// index: an open takes over the session of the same subject and name on its index.
    /// A named complete reader that opens while the home holds its position resumes at
    /// the position that the acks of its last complete session recorded, or where that
    /// session opened when they recorded none. It ends with [`Ended::Behind`] when a
    /// frame after that position was released or dropped because no complete session
    /// on its index was open.
    pub name: Option<Name>,
    /// How long the home holds a named complete reader's position after its session
    /// closes. Zero or more. It must be zero when the reader is unnamed or latest.
    pub hold: Span,
}

/// One frame that a reader got, through the reader's mask (M2): only the reader's
/// channels and their index.
#[derive(Debug)]
pub struct Received<'a> {
    view: View<'a>,
    lens: &'a Lens,
    lent: &'a Lent,
}

impl<'a> Received<'a> {
    /// The frame through the mask. Its series are encoded.
    #[must_use]
    pub fn view(&self) -> View<'a> {
        self.view
    }

    /// The key set that the view's entries index.
    #[must_use]
    pub fn set(&self) -> &'a Arc<KeySet> {
        &self.lens.set
    }

    /// The name of the channel at `entry` of [`Self::set`], as it was when the reader
    /// opened. The entry's `data_type` is the type of its samples.
    ///
    /// # Panics
    ///
    /// If `entry` is not an entry that [`Self::view`] gives.
    #[must_use]
    pub fn name(&self, entry: usize) -> &'a str {
        let names =
            self.lent.names.as_deref().expect(
                "invariant: the hub calls no `name` on a reader of a client read",
            );
        let place = names.find(&self.lens.set, entry).unwrap_or_else(|| {
            panic!("the entry {entry} is not a channel of the reader")
        });
        names.get(place)
    }

    /// The reader's position after this frame. Give it to [`Reader::ack`] when the
    /// frame is safe at its target.
    #[must_use]
    pub fn position(&self) -> Position {
        let position = self.lens.after(self.view.frame());
        self.lent.given.set(Some(position.live));
        position
    }

    /// The seqs of the index just before this frame that the reader did not get: a run
    /// that a trim freed (STORE TRIM) or that was lost for no room. `None` for most
    /// frames, and never an empty range. Its end is the seq of the frame's first
    /// sample. A session that opens again at the same position gives the same gap with
    /// the same frame.
    #[must_use]
    pub fn gap(&self) -> Option<Range<u64>> {
        let start = self.lens.seqs(self.view.frame()).start;
        let from = self.lent.from;
        (from < start).then_some(from..start)
    }
}

/// A position on one index: a reader at it has each sample below it. Only
/// [`Received`] makes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    index: channel::Slot,
    /// The seq of the first live sample that the reader has not received.
    live: u64,
}

/// A key set, the mask of a reader's channels in it, and the reader's index and its
/// group in it.
#[derive(Debug)]
pub(crate) struct Lens {
    pub(crate) set: Arc<KeySet>,
    pub(crate) mask: Mask,
    index: channel::Slot,
    group: u32,
}

impl Lens {
    /// The lens of `set` for a reader of `slots` on the index at `index`.
    ///
    /// # Panics
    ///
    /// If `set` does not hold `index`.
    pub(crate) fn new(
        set: &Arc<KeySet>,
        slots: impl IntoIterator<Item = channel::Slot>,
        index: channel::Slot,
    ) -> Self {
        let entry = set
            .find(index)
            .expect("invariant: the key set of a frame on an index holds the index");
        Self {
            set: Arc::clone(set),
            mask: Mask::new(set, slots),
            index,
            group: set.entries()[entry].group,
        }
    }

    /// The position of the reader after `frame`, a frame of this lens's key set. The
    /// mask holds the index, so the frame's range of its group is the view's.
    fn after(&self, frame: &Frame) -> Position {
        Position {
            index: self.index,
            live: self.seqs(frame).end,
        }
    }

    /// The seqs of the index's samples in `frame`, a frame of this lens's key set.
    fn seqs(&self, frame: &Frame) -> Range<u64> {
        let range = frame
            .range(self.group)
            .expect("invariant: a frame holds the range of each group");
        let end = range
            .end()
            .expect("invariant: the home gives no range past u64::MAX");
        range.seq..end
    }
}

/// Why a reader session gives no more frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The shard's buffer failed.
    Buffer(env::files::Error),
    /// A complete reader missed a frame: [`Mode::Complete`] and [`Config::name`] state
    /// when.
    Behind,
    /// A channel of the reader was removed from the definitions.
    Removed(channel::Key),
    /// A later open of the same subject and name took over the session.
    Replaced,
    /// The stream to the home of another node broke.
    Stream(transport::Error),
    /// The home of another node stopped or reset the stream with this HUB WIRE code.
    Refused(wire::hub::Refusal),
    /// A message from the home of another node broke HUB WIRE. The reader stopped the
    /// stream with code `MALFORMED`.
    Message(wire::hub::Error),
    /// The ends of a frame from the home of another node break a rule of a frame. The
    /// reader stopped the stream with code `MALFORMED`.
    Frame(types::frame::Error),
    /// The shard's pool had no block for a frame or a credit. The reader stopped the
    /// stream with code `BUSY`.
    Pool(block::Error),
    /// The home of another node started a frame once the charges of the frames that
    /// arrived reached the grant that the reader sent. The reader stopped the stream
    /// with code `MALFORMED`.
    Credit {
        /// The grant that the reader sent.
        limit_bytes: u64,
    },
}

impl fmt::Display for Ended {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Buffer(error) => write!(f, "the buffer of the shard failed: {error}"),
            Self::Behind => f.write_str(
                "the reader missed a frame and gets no later one: open a new reader",
            ),
            Self::Removed(key) => {
                write!(f, "channel {key} was removed: open a new reader")
            }
            Self::Replaced => f.write_str(
                "a later open of the same named reader took over the session",
            ),
            Self::Stream(error) => write!(f, "the stream to the home broke: {error}"),
            Self::Refused(refusal) => write!(
                f,
                "the home ended the session with code {}: {refusal}",
                refusal.code()
            ),
            Self::Message(error) => {
                write!(f, "a message from the home broke the hub protocol: {error}")
            }
            Self::Frame(error) => {
                write!(f, "a frame from the home is not valid: {error}")
            }
            Self::Pool(error) => {
                write!(f, "the pool had no block for the reader: {error}")
            }
            Self::Credit { limit_bytes } => write!(
                f,
                "the home sent a frame past the credit of {limit_bytes} bytes"
            ),
        }
    }
}

impl std::error::Error for Ended {}

/// Why a reader session did not open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The selector matches no channel.
    Empty,
    /// The channels are on more than one index: `first` is the least matched name,
    /// and `other` the least matched name on another index.
    ManyIndexes {
        /// The least matched name.
        first: Name,
        /// The least matched name on another index than `first`.
        other: Name,
    },
    /// The reader is named and the node has no mesh time yet. Open it again later.
    Unsynced,
    /// The home of the index is `home`, another node, and this reader opens only at the
    /// home of its index: a named reader, or the reader of a program's read.
    Remote {
        /// The home.
        home: types::node::Key,
    },
    /// The mesh stopped, so the home of the index is not known.
    Mesh(mesh::Stopped),
    /// The session or the stream to the home of another node failed.
    Transport(transport::Error),
    /// The home of another node stopped or reset the stream with this HUB WIRE code.
    Refused(wire::hub::Refusal),
    /// The reply of the home of another node broke HUB WIRE. The reader stopped the
    /// stream with code `MALFORMED`.
    Message(wire::hub::Error),
    /// The shard's pool had no block for a message to the home of another node. The
    /// reader stopped the stream with code `BUSY`.
    Pool(block::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the selector matches no channel"),
            Self::ManyIndexes { first, other } => write!(
                f,
                "the channels {first} and {other} are on different indexes: open a \
                 reader per index"
            ),
            Self::Unsynced => f.write_str(
                "the node has no mesh time yet: open the named reader again later",
            ),
            Self::Remote { home } => write!(
                f,
                "the home of the index of the reader is node {home}, and this reader \
                 opens only at the home"
            ),
            Self::Mesh(stopped) => write!(f, "the mesh stopped: {stopped}"),
            Self::Transport(error) => {
                write!(f, "the transport to the home failed: {error}")
            }
            Self::Refused(refusal) => write!(
                f,
                "the home refused the reader with code {}: {refusal}",
                refusal.code()
            ),
            Self::Message(error) => {
                write!(f, "the reply of the home broke the hub protocol: {error}")
            }
            Self::Pool(error) => {
                write!(f, "the pool had no block for the open: {error}")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<::home::reader::Unsynced> for Error {
    fn from(::home::reader::Unsynced: ::home::reader::Unsynced) -> Self {
        Self::Unsynced
    }
}

/// A reader session through the reader's channels. Dropping it closes the session;
/// frames that wait do not go out.
#[derive(Debug)]
pub struct Reader {
    source: Source,
    /// The slot of the reader's index.
    index: channel::Slot,
    /// The seq of the first live sample that a complete reader did not get, or
    /// `None` for a latest reader.
    next: Option<u64>,
    lent: Lent,
    /// The frame that the last [`Received`] lends.
    frame: Option<Frame>,
}

/// What a reader lends to each [`Received`], by one reference, so that `Received`
/// stays as small as with no names.
#[derive(Debug, Default)]
struct Lent {
    /// `None` for a reader of a client read. A box keeps the reader small.
    names: Option<Box<Names>>,
    /// The `live` of the last position that [`Received::position`] gave, or `None`
    /// before it gave one. The positions of a reader rise, so it is the highest.
    given: Cell<Option<u64>>,
    /// The first live seq that the reader had not got before the frame that the last
    /// [`Received`] lends: the start of its gap.
    from: u64,
}

/// Where a reader's frames come from.
#[derive(Debug)]
enum Source {
    /// A session at this node's home.
    Local(Local),
    /// A session at the home of another node.
    Remote(Box<Remote>),
}

/// A reader session at this node's home, with the grant and ack of a complete one
/// until it gave an end, and the charge of each frame it gave back.
#[derive(Debug)]
struct Local {
    session: Session,
    /// `None` for a latest reader, and once `next` gave an end, so no later ack moves
    /// the home position.
    complete: Option<(Complete, u64)>,
}

impl Local {
    /// Opens a session on `channels` at this node, the home of their index.
    /// Returns the session and, for a complete one, the seq of the first live sample
    /// that it can get.
    fn open(
        state: &Rc<RefCell<State>>,
        channels: Channels,
        mode: Mode,
        named: Option<::home::reader::named::Key>,
        hold: Span,
    ) -> Result<(Self, Option<u64>), Error> {
        let charge = ::home::reader::complete::Charge::Whole;
        let (session, complete) = match (mode, named) {
            (Mode::Complete, None) => {
                let (session, complete, live) =
                    Session::complete(state, channels, WINDOW, charge);
                (session, Some((complete, live)))
            }
            (Mode::Complete, Some(named)) => {
                let (key, live) = {
                    let mut state = state.borrow_mut();
                    let opened = state.home.open_named_complete(
                        channels.index,
                        named,
                        hold,
                        WINDOW,
                        charge,
                    )?;
                    let live = opened.live;
                    (state.take_over(opened), live)
                };
                let (session, complete) = Session::with_complete(state, key, channels);
                (session, Some((complete, live)))
            }
            (Mode::Latest, None) => (Session::latest(state, channels), None),
            (Mode::Latest, Some(named)) => {
                let key = {
                    let mut state = state.borrow_mut();
                    let opened = state.home.open_named_latest(channels.index, named)?;
                    state.take_over(opened)
                };
                (Session::new(state, key, channels), None)
            }
        };
        let live = complete.as_ref().map(|&(_, live)| live);
        let local = Self {
            session,
            complete: complete.map(|(complete, _)| (complete, 0)),
        };
        Ok((local, live))
    }

    /// Raises the grant by the charge of `frame`, which the reader gave back.
    fn give_back(&mut self, frame: &Frame) {
        if let Some((complete, taken_bytes)) = &mut self.complete {
            *taken_bytes += frame.charge();
            complete.grant(*taken_bytes + WINDOW);
        }
    }
}

impl Reader {
    /// Opens a reader on the channels that `config.select` matches.
    ///
    /// # Panics
    ///
    /// When `config.hold` is negative, or not zero for an unnamed or latest reader.
    pub(crate) async fn open(
        state: &Rc<RefCell<State>>,
        config: Config,
    ) -> Result<Self, Error> {
        let Config {
            select,
            mode,
            subject,
            name,
            hold,
        } = config;
        assert!(
            hold >= Span::ZERO,
            "the hold {hold} of a reader is negative"
        );
        assert!(
            hold == Span::ZERO || (name.is_some() && mode == Mode::Complete),
            "a hold of {hold} for a reader that is unnamed or latest"
        );
        let named = name.map(|name| ::home::reader::named::Key { subject, name });
        let mut listed = None;
        let items = |found: &[(&Name, &Channel)]| {
            listed = Some(Names::new(found));
            let item =
                |&(_, channel): &(&Name, &Channel)| (channel.key(), channel.sample());
            found.iter().map(item).collect()
        };
        let (matched, home) = find(state, || &select, items).await?;
        let mut listed =
            listed.expect("invariant: `find` calls `items` before it returns");
        let mut reader = match home {
            Home::Here(channels) => {
                listed.slot(&channels.slots);
                Self::local(state, channels, mode, named, hold)?
            }
            Home::Away(home) if named.is_some() => return Err(Error::Remote { home }),
            Home::Away(home) => {
                let index = matched.index;
                let (remote, slots, live) =
                    Remote::open(state, home, matched, mode).await?;
                listed.slot(&slots);
                Self {
                    index: slots[index],
                    source: Source::Remote(Box::new(remote)),
                    next: (mode == Mode::Complete).then_some(live),
                    lent: Lent::default(),
                    frame: None,
                }
            }
        };
        reader.lent.names = Some(Box::new(listed));
        Ok(reader)
    }

    /// Opens an unnamed reader on `channels` from [`Home::Here`], with no wait since
    /// [`find`] gave them. It holds no names: the places of a served read carry them,
    /// so [`Received::name`] panics.
    pub(crate) fn here(
        state: &Rc<RefCell<State>>,
        channels: Channels,
        mode: Mode,
    ) -> Result<Self, Error> {
        Self::local(state, channels, mode, None, Span::ZERO)
    }

    /// The first live seq that a complete reader did not get: its open position until
    /// its first frame. 0 for a latest reader.
    pub(crate) fn live(&self) -> u64 {
        self.next.unwrap_or(0)
    }

    /// Opens a reader on `channels` at this node, the home of their index.
    fn local(
        state: &Rc<RefCell<State>>,
        channels: Channels,
        mode: Mode,
        named: Option<::home::reader::named::Key>,
        hold: Span,
    ) -> Result<Self, Error> {
        let slot = channels.index;
        let (local, next) = Local::open(state, channels, mode, named, hold)?;
        Ok(Self {
            source: Source::Local(local),
            index: slot,
            next,
            lent: Lent::default(),
            frame: None,
        })
    }

    /// The next frame, as a view of the reader's channels. The view borrows the
    /// reader, so the frame stays in use, and spends the reader's credit, until the
    /// next call, not its first poll, or the drop. After a run of frames, it yields
    /// once, so a task that loops on it lets the shard's other tasks run.
    ///
    /// # Errors
    ///
    /// [`Ended::Removed`] once a channel of the reader is removed, or
    /// [`Ended::Replaced`] once a later open takes over the session, before any frame
    /// that waits. Else [`Ended`] once no frame waits and the session can give no
    /// more. Either on this and every later call: [`Ended::Behind`] before
    /// [`Ended::Buffer`].
    #[expect(
        clippy::should_implement_trait,
        reason = "it gives a future, which `Iterator::next` cannot"
    )]
    pub fn next(&mut self) -> impl Future<Output = Result<Received<'_>, Ended>> {
        if let Some(frame) = self.frame.take() {
            match &mut self.source {
                Source::Local(local) => local.give_back(&frame),
                Source::Remote(remote) => remote.give_back(&frame),
            }
        }
        async move {
            let (frame, lens) = match &mut self.source {
                Source::Local(local) => match local.session.take().await {
                    Ok(taken) => taken,
                    Err(stop) => {
                        local.complete = None;
                        return Err(stop.into());
                    }
                },
                Source::Remote(remote) => remote.take().await?,
            };
            let seqs = lens.seqs(&frame);
            self.lent.from = match &mut self.next {
                Some(next) => mem::replace(next, seqs.end),
                None => seqs.start,
            };
            let frame = self.frame.insert(frame);
            Ok(Received {
                view: View::new(frame, &lens.mask),
                lens,
                lent: &self.lent,
            })
        }
    }

    /// Records that the reader has each sample up to `position`. A named complete
    /// reader that opens again starts there. An ack at or below the reader's last ack
    /// changes nothing. Nor does the ack of a latest reader, which holds nothing, of a
    /// reader whose index has its home at another node, or of a reader that ended:
    /// after `next` gave an [`Ended`], or once the hub ended it with
    /// [`Ended::Removed`] or [`Ended::Replaced`].
    ///
    /// # Panics
    ///
    /// If `position` is of another index than the reader's, or is past each position
    /// that [`Received::position`] gave for this reader. Before it gave one, each
    /// `position` panics.
    pub fn ack(&mut self, position: Position) {
        assert!(
            position.index == self.index,
            "the position is of another index than the reader's"
        );
        let Some(given) = self.lent.given.get() else {
            panic!(
                "the position is past each position that this reader gave: it gave none"
            );
        };
        assert!(
            position.live <= given,
            "the position is past each position that this reader gave: live {} past {}",
            position.live,
            given
        );
        match &self.source {
            Source::Local(Local {
                complete: Some((complete, _)),
                ..
            }) => complete.ack(position.live),
            Source::Local(_) | Source::Remote(_) => {}
        }
    }
}

/// Why a [`Session`] at this node's home gives no more frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The shard's buffer failed.
    Buffer(env::files::Error),
    /// A complete session missed a frame.
    Behind,
    /// A channel of the session was removed from the definitions.
    Removed(channel::Key),
    /// A later open of the same named reader took over the session.
    Replaced,
}

impl From<End> for Stop {
    fn from(end: End) -> Self {
        match end {
            End::Removed(key) => Self::Removed(key),
            End::Replaced => Self::Replaced,
        }
    }
}

impl From<Stop> for Ended {
    fn from(stop: Stop) -> Self {
        match stop {
            Stop::Buffer(error) => Self::Buffer(error),
            Stop::Behind => Self::Behind,
            Stop::Removed(key) => Self::Removed(key),
            Stop::Replaced => Self::Replaced,
        }
    }
}

/// The name of each channel of a reader, in the order of its channels, in one text,
/// so an open makes no allocation for each name, and the places of the key set of the
/// last name.
#[derive(Debug)]
struct Names {
    /// The end in `text` of the name of each channel.
    ends: Box<[usize]>,
    text: Box<str>,
    /// Made again on the first name after each change of key set, so a reader that
    /// gives no name does no work for them.
    places: RefCell<Places>,
}

impl Names {
    /// The names of `found`, in their order, with no slots.
    fn new(found: &[(&Name, &Channel)]) -> Self {
        let len = found.iter().map(|(name, _)| name.as_str().len()).sum();
        let mut text = String::with_capacity(len);
        let ends = found
            .iter()
            .map(|(name, _)| {
                text.push_str(name.as_str());
                text.len()
            })
            .collect();
        Self {
            ends,
            text: text.into_boxed_str(),
            places: RefCell::default(),
        }
    }

    /// Gives the slot of each channel, in their order.
    fn slot(&mut self, slots: &[channel::Slot]) {
        self.places.get_mut().channels =
            slots.iter().map(|&slot| (slot, Places::NONE)).collect();
    }

    /// The place in the channels of `entry` of `set`, or `None` when it is not one.
    fn find(&self, set: &Arc<KeySet>, entry: usize) -> Option<usize> {
        let mut places = self.places.borrow_mut();
        places.make(set);
        let place = *places.entries.get(entry)?;
        (place != Places::NONE).then(|| to_usize(place))
    }

    /// The name of the channel at `place`.
    fn get(&self, place: usize) -> &str {
        let start = place.checked_sub(1).map_or(0, |before| self.ends[before]);
        &self.text[start..self.ends[place]]
    }
}

/// The place in a reader's channels of each entry of one key set.
#[derive(Debug, Default)]
struct Places {
    /// The key set that the entries are of, or `None` before the first name. Held, so
    /// that no other key set has its key.
    set: Option<Arc<KeySet>>,
    /// The slot of each channel, and its entry in `set` or [`Places::NONE`]. Empty
    /// until the open knows the home of the index.
    channels: Box<[(channel::Slot, u32)]>,
    /// The place of each entry of `set`, or [`Places::NONE`]. As long as the largest
    /// key set so far, with [`Places::NONE`] past `set`, so that a new key set
    /// allocates only when it is larger.
    entries: Vec<u32>,
}

impl Places {
    /// The place of an entry that is not a channel of the reader.
    const NONE: u32 = u32::MAX;

    /// Makes the places those of `set`. It costs a search of `set` for each channel,
    /// and work for each other entry of `set` only when `set` is larger than each key
    /// set before it.
    fn make(&mut self, set: &Arc<KeySet>) {
        if self
            .set
            .as_ref()
            .is_some_and(|held| held.key() == set.key())
        {
            return;
        }
        for &(_, entry) in &self.channels {
            if entry != Self::NONE {
                self.entries[to_usize(entry)] = Self::NONE;
            }
        }
        let len = self.entries.len().max(set.entries().len());
        self.entries.resize(len, Self::NONE);
        for (place, (slot, entry)) in self.channels.iter_mut().enumerate() {
            *entry = set.find(*slot).map_or(Self::NONE, |at| {
                self.entries[at] = u32::try_from(place)
                    .expect("invariant: a reader has fewer than u32::MAX channels");
                u32::try_from(at)
                    .expect("invariant: a key set has fewer than u32::MAX entries")
            });
        }
        self.set = Some(Arc::clone(set));
    }
}

fn to_usize(value: u32) -> usize {
    usize::try_from(value).expect("invariant: a usize holds a u32")
}

/// What the selector of a reader matched.
#[derive(Debug)]
pub(crate) struct Matched<T> {
    /// An item for each channel that the selector matched, in name order, then for
    /// their index when the selector did not match it: a frame without the reader's
    /// channels still shows that time moved.
    pub(crate) items: Vec<T>,
    /// The position of the index in `items`.
    pub(crate) index: usize,
}

/// The home of the index of what a selector matched.
#[derive(Debug)]
pub(crate) enum Home {
    /// This node, which carries the index, with the channels as the frames of a reader
    /// hold them.
    Here(Channels),
    /// Another node.
    Away(types::node::Key),
}

/// What the selector that `select` gives matches once the home of its index is known,
/// with the items that `items` makes, one for each channel in the order of the channels
/// it gets, and that home. It applies the spec in use before each match. A change of a
/// channel that it matched ends a wait for the home at once, and it matches again. It
/// calls `select` before and after each wait for the home, and keeps no selector or
/// item across a wait. It calls `items` once, after the last wait.
pub(crate) async fn find<S: std::borrow::Borrow<Selector>, T>(
    state: &Rc<RefCell<State>>,
    mut select: impl FnMut() -> S,
    mut items: impl FnMut(&[(&Name, &Channel)]) -> Vec<T>,
) -> Result<(Matched<T>, Home), Error> {
    let (index, homed) = loop {
        let (index, keys) = {
            let mut borrowed = state.borrow_mut();
            borrowed.catch_up();
            let defined = defined(&borrowed, select().borrow())?;
            let index = defined[0].1.index();
            let keys = defined.iter().map(|(_, channel)| channel.key());
            (index, keys.collect())
        };
        let pending = Pending::new(state, keys);
        if let Ok(homed) = pending.wait(crate::homes(state, &[index])).await {
            break (index, homed);
        }
    };
    let away = match homed {
        Ok(()) => None,
        Err(Away::Remote(home)) => Some(home),
        Err(Away::Mesh(stopped)) => return Err(Error::Mesh(stopped)),
    };
    // A spec that only adds a channel ends no wait, and the selector can match it now.
    let (keys, found) = matched(&state.borrow(), select().borrow(), &mut items)?;
    assert_eq!(
        keys[found.index], index,
        "invariant: no registered channel changed, so the index stays"
    );
    let home = if let Some(home) = away {
        Home::Away(home)
    } else {
        // With no wait since `matched`, so each of `keys` is defined.
        let slots = state.borrow_mut().slots(index, &keys);
        Home::Here(Channels {
            index: slots[found.index],
            keys,
            slots,
        })
    };
    Ok((found, home))
}

/// The defined channels that `select` matches, in name order, each on one index.
fn defined<'a>(
    state: &'a State,
    select: &Selector,
) -> Result<Vec<(&'a Name, &'a Channel)>, Error> {
    let mut found: Vec<(&Name, &Channel)> = state
        .channels
        .iter()
        .filter(|&(name, _)| select.matches(name).is_some())
        .collect();
    found.sort_unstable_by_key(|&(name, _)| name);
    let &(first, channel) = found.first().ok_or(Error::Empty)?;
    let index = channel.index();
    if let Some(&(other, _)) = found.iter().find(|(_, other)| other.index() != index) {
        return Err(Error::ManyIndexes {
            first: first.clone(),
            other: other.clone(),
        });
    }
    Ok(found)
}

/// The key of each channel that `select` matches, in name order, then of their one
/// index when `select` does not match it, and what `select` matched, with the items
/// that `items` makes.
fn matched<T>(
    state: &State,
    select: &Selector,
    mut items: impl FnMut(&[(&Name, &Channel)]) -> Vec<T>,
) -> Result<(Box<[channel::Key]>, Matched<T>), Error> {
    let mut found = defined(state, select)?;
    let index = found[0].1.index();
    let at = if let Some(at) =
        found.iter().position(|(_, channel)| channel.key() == index)
    {
        at
    } else {
        let named = state
            .channels
            .iter()
            .find(|(_, channel)| channel.key() == index)
            .expect("invariant: the index of a defined channel is defined");
        found.push(named);
        found.len() - 1
    };
    let keys = found.iter().map(|(_, channel)| channel.key()).collect();
    let items = items(&found);
    Ok((keys, Matched { items, index: at }))
}

/// A session at the shard's home, through a mask of the reader's channels: the frames
/// it takes, and why it ends. Each [`Reader`] drives one, and so does each stream of a
/// remote reader.
#[derive(Debug)]
pub(crate) struct Session {
    state: Rc<RefCell<State>>,
    key: ::home::reader::Key,
    /// Why the hub ended the session.
    ending: Ending,
    /// The slots of the reader's channels.
    slots: Box<[channel::Slot]>,
    /// The slot of their index.
    index: channel::Slot,
    /// The lens of the key set of the last frame.
    lens: Option<Lens>,
    streak: Streak,
}

/// The channels of a [`Session`]: the key and slot of each, and the slot of their
/// index.
#[derive(Debug)]
pub(crate) struct Channels {
    pub(crate) keys: Box<[channel::Key]>,
    pub(crate) slots: Box<[channel::Slot]>,
    pub(crate) index: channel::Slot,
}

impl Session {
    /// Opens a complete session on `channels`, with a grant of `limit_bytes` that each
    /// frame spends as `charge` says. Returns the session, its grant and ack, and the
    /// seq of the first live sample that it can get.
    pub(crate) fn complete(
        state: &Rc<RefCell<State>>,
        channels: Channels,
        limit_bytes: u64,
        charge: ::home::reader::complete::Charge,
    ) -> (Self, Complete, u64) {
        let opened =
            state
                .borrow_mut()
                .home
                .open_complete(channels.index, limit_bytes, charge);
        let (session, complete) = Self::with_complete(state, opened.key, channels);
        (session, complete, opened.live)
    }

    /// Opens a latest session on `channels`.
    pub(crate) fn latest(state: &Rc<RefCell<State>>, channels: Channels) -> Self {
        let key = state.borrow_mut().home.open_latest(channels.index);
        Self::new(state, key, channels)
    }

    fn with_complete(
        state: &Rc<RefCell<State>>,
        key: ::home::reader::complete::Key,
        channels: Channels,
    ) -> (Self, Complete) {
        let session = Self::new(state, key.into(), channels);
        let complete = Complete {
            state: Rc::clone(state),
            key,
            ending: session.ending.clone(),
        };
        (session, complete)
    }

    fn new(
        state: &Rc<RefCell<State>>,
        key: ::home::reader::Key,
        channels: Channels,
    ) -> Self {
        let ending = state
            .borrow_mut()
            .readers
            .add(key, channels.keys.into_vec());
        Self {
            state: Rc::clone(state),
            key,
            ending,
            slots: channels.slots,
            index: channels.index,
            lens: None,
            streak: Streak::default(),
        }
    }

    /// The next frame, and the lens of its key set. After [`STREAK`] frames in a row,
    /// it yields once.
    ///
    /// # Errors
    ///
    /// [`Stop::Removed`] once a channel of the session is removed, or
    /// [`Stop::Replaced`] once a takeover ends it, before any frame that waits. Else
    /// [`Stop`] once no frame waits and the session can give no more. Either on this
    /// and every later call: [`Stop::Behind`] before [`Stop::Buffer`].
    ///
    /// # Panics
    ///
    /// When the interner does not hold the key set of the frame, which a writer of
    /// this hub made.
    pub(crate) async fn take(&mut self) -> Result<(Frame, &Lens), Stop> {
        let frame = poll_fn(|cx| {
            if let Some(end) = self.ending.get() {
                return Poll::Ready(Err(end.into()));
            }
            ready!(self.streak.poll(cx));
            let polled = poll_take(&mut self.state.borrow_mut(), self.key, cx);
            self.streak.count(&polled);
            polled
        })
        .await?;
        let key = frame.key_set();
        let lens = match self.lens.take() {
            Some(lens) if lens.set.key() == key => self.lens.insert(lens),
            _ => {
                let snapshot = self.state.borrow().interner.snapshot();
                let set = snapshot
                    .get(key)
                    .expect("invariant: a frame's key set is known");
                let lens = Lens::new(set, self.slots.iter().copied(), self.index);
                self.lens.insert(lens)
            }
        };
        Ok((frame, lens))
    }
}

/// The next frame of the session `key`, or the waker of `cx` kept to wake once one
/// waits.
fn poll_take(
    state: &mut State,
    key: ::home::reader::Key,
    cx: &Context<'_>,
) -> Poll<Result<Frame, Stop>> {
    match state.home.take(key) {
        Next::Frame(frame) => return Poll::Ready(Ok(frame)),
        Next::Behind => return Poll::Ready(Err(Stop::Behind)),
        Next::Empty => {}
    }
    if let Some(error) = &state.failed {
        return Poll::Ready(Err(Stop::Buffer(error.clone())));
    }
    state.wakers.insert(key, cx.waker().clone());
    Poll::Pending
}

/// The frames that a source gave in a row since it last waited.
#[derive(Debug, Default)]
pub(super) struct Streak(u32);

impl Streak {
    /// `Pending` once, with a wake, after [`STREAK`] frames in a row, so the shard's
    /// other tasks run; else `Ready`.
    fn poll(&mut self, cx: &Context<'_>) -> Poll<()> {
        if self.0 < STREAK {
            return Poll::Ready(());
        }
        self.0 = 0;
        cx.waker().wake_by_ref();
        Poll::Pending
    }

    /// Counts one poll of the source: a wait starts the count again.
    fn count<T>(&mut self, polled: &Poll<T>) {
        self.0 = if polled.is_pending() { 0 } else { self.0 + 1 };
    }
}

/// The grant and the ack of a complete [`Session`]: only a complete open gives one.
#[derive(Debug)]
pub(crate) struct Complete {
    state: Rc<RefCell<State>>,
    key: ::home::reader::complete::Key,
    ending: Ending,
}

impl Complete {
    /// Raises the grant of the session to `limit_bytes` since the open. Changes
    /// nothing once the hub ended the session.
    pub(crate) fn grant(&self, limit_bytes: u64) {
        if self.ending.get().is_none() {
            self.state.borrow_mut().home.grant(self.key, limit_bytes);
        }
    }

    /// Records that the session has each sample below seq `live`. Changes nothing
    /// once the hub ended the session.
    fn ack(&self, live: u64) {
        if self.ending.get().is_some() {
            return;
        }
        let position = ::home::reader::Position {
            live,
            backfill: None,
        };
        self.state
            .borrow_mut()
            .home
            .ack(self.key, position)
            .expect("invariant: a hub position keeps the reader's paths");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.state.borrow_mut().close_reader(self.key);
    }
}

#[cfg(test)]
mod tests {
    use wire::hub::Refusal;

    use super::*;

    /// `Reader::next` returns a `Received` for each frame, so the names add no word
    /// to it.
    #[test]
    fn keeps_a_received_at_32_bytes() {
        assert_eq!(size_of::<Received<'_>>(), 32);
    }

    #[test]
    fn names_the_code_of_a_refusal_and_what_the_home_meant() {
        assert_eq!(
            Error::Refused(Refusal::NotHome).to_string(),
            "the home refused the reader with code 17: the node is not the home of \
             the index"
        );
        assert_eq!(
            Ended::Refused(Refusal::Failed).to_string(),
            "the home ended the session with code 18: the home's buffer failed, or \
             its mesh stopped"
        );
    }
}
