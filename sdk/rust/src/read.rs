//! The reads of a client: one read stream for each index of the channels, merged by
//! time.

use std::convert::Infallible;
use std::future::poll_fn;
use std::mem;
use std::ops::Range;
use std::pin::{Pin, pin};
use std::slice;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::task::Poll;

pub use program::Mode;
use program::Refusal;
use tokio::sync::{mpsc, watch};
use types::name::{Name, Selector, Written};

use crate::{Error, Place, lost, wait};

/// What [`Client::open_reader`](crate::Client::open_reader) opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The channels that the reader reads, at least one, on any indexes.
    pub channels: Vec<Name>,
    /// Which frames the reader gets. [`Config::new`] gives [`Mode::Complete`]: each
    /// frame from the open on, in order, past a loss of the node's buffer with the
    /// lost seqs. [`Mode::Latest`] skips by design each frame that a newer one replaces
    /// before the node sends it, with no lost seqs. The frames that the client got
    /// come in order.
    pub mode: Mode,
}

impl Config {
    /// A complete reader of `channels`.
    #[must_use]
    pub fn new(channels: Vec<Name>) -> Self {
        Self {
            channels,
            mode: Mode::Complete,
        }
    }
}

/// A reader of a [`Client`](crate::Client): one read stream for each index of its
/// channels, which the client's thread holds. It can move between threads. The frames
/// of one index come in order. Across indexes, of the frames that the client got, the
/// one with the earliest first stamp comes first, and no call waits for an index with
/// no frame. Dropping it stops the streams, and the node ends the reads.
///
/// Each frame that the node sent and the caller did not read holds a part of the
/// receive window of the client's session. When the window is full, each stream of the
/// client waits: the answers to its writers, its other readers, and the renewal of its
/// hello, and the node closes the session when the hello ends. Read each reader as its
/// frames come, or read on a client of its own.
#[derive(Debug)]
pub struct Reader {
    places: Vec<Place>,
    calls: mpsc::Sender<Frame>,
    answers: Receiver<Answer>,
    /// Set when the client closed or dropped.
    closed: Arc<AtomicBool>,
    /// The frame of the last call, which goes to the client's thread and back.
    frame: Frame,
    /// The error that ended the reader.
    ended: Option<Error>,
}

impl Reader {
    /// Sends the open of `config` on `opens`, and blocks until the client's thread
    /// opened it. `closed` is set when the client closed or dropped.
    pub(crate) fn open<O: From<Open>>(
        opens: &mpsc::UnboundedSender<O>,
        closed: &Arc<AtomicBool>,
        config: Config,
    ) -> Result<Self, Error> {
        if config.channels.is_empty() {
            return Err(Error::Stream(program::Error::Unnamed));
        }
        let (calls, received) = mpsc::channel(1);
        let (answer, answers) = sync_channel(1);
        let (sender, opened) = sync_channel(1);
        let open = Open {
            config,
            opened: sender,
            calls: received,
            answer,
        };
        opens.send(open.into()).map_err(|_ended| lost(closed))?;
        Ok(Self {
            places: wait(&opened, closed)??,
            calls,
            answers,
            closed: Arc::clone(closed),
            frame: Frame::new(),
            ended: None,
        })
    }

    /// The channels of the reader: the key, sample type, and name of each place.
    /// Place `n` is `Config::channels[n]`, so a channel named twice is at each of its
    /// places. Then the index of each index that `Config::channels` does not name, in
    /// the order of the first channel of each index.
    #[must_use]
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// Blocks until the next frame. The frame is the reader's own, and the next call
    /// fills it again.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] when the client's session ended at a close or a drop, and
    /// [`Error::Panicked`] when the client's thread panicked. [`Error::Session`] when
    /// the session failed. [`Error::Decode`] when a series does not decode. Else
    /// [`Error::Stream`] with each error of `program::Reader::next`, such as
    /// `Refused(Refusal::Unknown)` when a channel was removed. An error of the read of
    /// one index ends the reader, and the frames that the reads of the other indexes
    /// hold are not given. After an error, each call gives the same error.
    #[expect(
        clippy::should_implement_trait,
        reason = "the frame borrows the reader, which `Iterator` cannot give"
    )]
    pub fn next(&mut self) -> Result<&Frame, Error> {
        if let Some(error) = &self.ended {
            return Err(error.clone());
        }
        let frame = mem::replace(&mut self.frame, Frame::new());
        // Each call takes its answer, so the channel is never full.
        self.calls
            .try_send(frame)
            .map_err(|_ended| lost(&self.closed))?;
        let (answer, frame) = wait(&self.answers, &self.closed)?;
        self.frame = frame;
        match answer {
            Ok(()) => Ok(&self.frame),
            Err(error) => Err(self.ended.insert(error).clone()),
        }
    }
}

/// A frame of a [`Reader`]: the series of the channels of one index, `count` samples
/// each, at the stamps of the series of the index.
#[derive(Debug)]
pub struct Frame {
    count: u32,
    index: usize,
    /// The series of each place. Those that the frame does not hold keep their bytes
    /// for later frames.
    series: Vec<Series>,
}

/// The series of a place of a [`Frame`].
#[derive(Debug, Default)]
struct Series {
    /// Set when the frame holds the series.
    held: bool,
    /// The samples in the raw form of the type of the place.
    bytes: Vec<u8>,
    lost: Range<u64>,
}

impl Frame {
    /// A frame of no series, which allocates nothing.
    const fn new() -> Self {
        Self {
            count: 0,
            index: 0,
            series: Vec::new(),
        }
    }

    /// The samples of each series.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The place of the index of the frame. Its series holds the stamps of the frame.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The series of `place`, in the raw form of its type, or `None` when the frame
    /// holds no series of it.
    #[must_use]
    pub fn series(&self, place: usize) -> Option<&[u8]> {
        self.find(place).map(|series| series.bytes.as_slice())
    }

    /// The seqs of the index of `place` lost after the last series of `place` with a
    /// sample, up to this frame: one range from the first lost seq to the last. A seq
    /// in it that the reader got held no sample of `place`. Empty when the frame holds
    /// no sample of `place`, when none was lost, and for a latest reader.
    #[must_use]
    pub fn lost(&self, place: usize) -> Range<u64> {
        self.find(place).map_or(0..0, |series| series.lost.clone())
    }

    fn find(&self, place: usize) -> Option<&Series> {
        self.series.get(place).filter(|series| series.held)
    }

    /// The first stamp of the frame, or `None` for a frame of no sample.
    fn first(&self) -> Option<i64> {
        let index = self.series(self.index)?;
        index.first_chunk().map(|stamp| i64::from_le_bytes(*stamp))
    }
}

/// The open of a reader, which the client's thread serves in a task of its own.
#[derive(Debug)]
pub(crate) struct Open {
    config: Config,
    opened: SyncSender<Result<Vec<Place>, Error>>,
    calls: mpsc::Receiver<Frame>,
    answer: SyncSender<Answer>,
}

/// The outcome of a call of [`Reader::next`], and its frame.
type Answer = (Result<(), Error>, Frame);

/// Opens the reads of `open` on `session`, and serves the calls of its [`Reader`]
/// until the reader ends or drops, or `closed` changes. Its sender drops at the close
/// of the client, so the reads drop before the session closes.
pub(crate) async fn serve(
    session: program::Session,
    open: Open,
    mut closed: watch::Receiver<()>,
) {
    let mut served = pin!(served(session, open));
    let mut changed = pin!(closed.changed());
    poll_fn(|cx| {
        if changed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(());
        }
        served.as_mut().poll(cx)
    })
    .await;
}

async fn served(session: program::Session, open: Open) {
    let Open {
        config,
        opened,
        mut calls,
        answer,
    } = open;
    let (readers, layout) = match readers(&session, &config).await {
        Ok(opened) => opened,
        Err(error) => {
            drop(opened.try_send(Err(failed(&session, error))));
            return;
        }
    };
    drop(opened.try_send(Ok(layout.places)));
    let mut feeds = Vec::with_capacity(readers.len());
    let mut parts = Vec::with_capacity(readers.len());
    for (reader, targets) in readers.into_iter().zip(layout.parts) {
        let (given, taken) = mpsc::channel(1);
        let (free, freed) = mpsc::channel(1);
        feeds.push(Box::pin(feed(
            session.clone(),
            reader,
            targets,
            given,
            freed,
        )));
        parts.push(Part {
            taken,
            free,
            held: None,
        });
    }
    while let Some(mut frame) = calls.recv().await {
        let next = poll_fn(|cx| merge(cx, &mut feeds, &mut parts, &mut frame)).await;
        let ended = next.is_err();
        drop(answer.try_send((next, frame)));
        if ended {
            return;
        }
    }
}

/// [`Error::Session`] when `session` ended, else [`Error::Stream`] with `error`.
fn failed(session: &program::Session, error: program::Error) -> Error {
    match session.ended() {
        Some(cause) => Error::Session(cause),
        None => Error::Stream(error),
    }
}

/// Opens one reader for each index of `config.channels`, in the order of the first
/// channel of each index, and gives their layout. No request of the node gives the
/// index of a channel, so when one reader of all the channels fails or leaves a name
/// out, it opens a latest reader of each channel, keeps only its index, and drops it,
/// then opens one reader of each index. So it holds at most one stream for each index.
async fn readers(
    session: &program::Session,
    config: &Config,
) -> Result<(Vec<program::Reader>, Layout), program::Error> {
    let names = &config.channels;
    if let Ok(reader) = reader(session, names, config.mode).await
        && let Some(layout) = laid(names, slice::from_ref(&reader))
    {
        return Ok((vec![reader], layout));
    }
    let mut indexes = Vec::with_capacity(names.len());
    for name in names {
        let probe = reader(session, slice::from_ref(name), Mode::Latest).await?;
        indexes.push(probe.places()[probe.index()].key);
    }
    let mut readers = Vec::new();
    for members in grouped(names, &indexes) {
        readers.push(reader(session, &members, config.mode).await?);
    }
    // A channel removed after its probe is no place of its reader.
    let layout =
        laid(names, &readers).ok_or(program::Error::Refused(Refusal::Unknown))?;
    Ok((readers, layout))
}

/// Opens a reader of `names`, at least one, on `session`.
async fn reader(
    session: &program::Session,
    names: &[Name],
    mode: Mode,
) -> Result<program::Reader, program::Error> {
    let written = names.iter().map(|name| Written::Include(name.as_str()));
    let select = Selector::from_written(written)
        .expect("invariant: a name reads as a pattern, and the names are not empty");
    session.reader(select, mode).await
}

/// The layout of a reader of `names` from `readers`, or `None` when a name is no
/// place of them.
fn laid(names: &[Name], readers: &[program::Reader]) -> Option<Layout> {
    let parts: Vec<_> = readers
        .iter()
        .map(|reader| (reader.places(), reader.index()))
        .collect();
    layout(names, &parts)
}

/// `names` in groups of the same index, each in the order of its first name, where
/// `indexes[n]` is the index of `names[n]`.
fn grouped(names: &[Name], indexes: &[types::channel::Key]) -> Vec<Vec<Name>> {
    let mut groups: Vec<(types::channel::Key, Vec<Name>)> = Vec::new();
    for (name, &index) in names.iter().zip(indexes) {
        match groups.iter_mut().find(|(key, _)| *key == index) {
            Some((_, members)) => members.push(name.clone()),
            None => groups.push((index, vec![name.clone()])),
        }
    }
    groups.into_iter().map(|(_, members)| members).collect()
}

/// The places of a reader, and where the frames of each of its parts go in them.
#[derive(Debug, PartialEq, Eq)]
struct Layout {
    places: Vec<Place>,
    parts: Vec<Targets>,
}

/// Where the series of the frames of one part go in the places of its reader.
#[derive(Debug, PartialEq, Eq)]
struct Targets {
    /// The place of the index.
    index: usize,
    /// The places of each entry of the part, in order.
    entries: Vec<Vec<usize>>,
    /// The places of the reader.
    places: usize,
}

/// The layout of a reader of `names` from `parts`, the places of each part and the
/// entry of its index: place `n` is `names[n]`, then the index of each part that
/// `names` does not name. `None` when a name is no place of a part.
fn layout(names: &[Name], parts: &[(&[Place], usize)]) -> Option<Layout> {
    let mut entries: Vec<Vec<Vec<usize>>> = parts
        .iter()
        .map(|(places, _)| vec![Vec::new(); places.len()])
        .collect();
    let mut places = Vec::with_capacity(names.len() + parts.len());
    for name in names {
        let (part, entry) =
            parts.iter().enumerate().find_map(|(part, (each, _))| {
                let entry = each.iter().position(|place| place.name == *name)?;
                Some((part, entry))
            })?;
        entries[part][entry].push(places.len());
        places.push(parts[part].0[entry].clone());
    }
    for (part, &(each, index)) in parts.iter().enumerate() {
        if entries[part][index].is_empty() {
            entries[part][index].push(places.len());
            places.push(each[index].clone());
        }
    }
    let total = places.len();
    let parts = parts
        .iter()
        .zip(entries)
        .map(|(&(_, index), entries)| Targets {
            index: entries[index][0],
            entries,
            places: total,
        })
        .collect();
    Some(Layout { places, parts })
}

/// The frames of one part on their way to the merge, and the frames that it gives
/// back to fill again.
#[derive(Debug)]
struct Part {
    taken: mpsc::Receiver<Taken>,
    free: mpsc::Sender<Frame>,
    /// The frame that the part gave and the merge did not, with its first stamp.
    held: Option<(Option<i64>, Frame)>,
}

/// A frame of a part with its first stamp, or the error that ended the part with a
/// frame to give back.
type Taken = (Result<Option<i64>, Error>, Frame);

/// Polls each of `feeds` and each of `parts` that holds no frame, and when a part
/// holds one, swaps the earliest into `frame`.
fn merge<F: Future<Output = Infallible>>(
    cx: &mut std::task::Context<'_>,
    feeds: &mut [Pin<Box<F>>],
    parts: &mut [Part],
    frame: &mut Frame,
) -> Poll<Result<(), Error>> {
    for feed in feeds.iter_mut() {
        if let Poll::Ready(never) = feed.as_mut().poll(cx) {
            match never {}
        }
    }
    for part in parts.iter_mut().filter(|part| part.held.is_none()) {
        if let Poll::Ready(Some((first, taken))) = part.taken.poll_recv(cx) {
            part.held = Some((first?, taken));
        }
    }
    let firsts = parts
        .iter()
        .map(|part| part.held.as_ref().map(|held| held.0));
    let Some(at) = earliest(firsts) else {
        return Poll::Pending;
    };
    let part = &mut parts[at];
    let (_, held) = part
        .held
        .take()
        .expect("invariant: the earliest holds a frame");
    // The part takes its frame back before it reads again, so the channel has room.
    drop(part.free.try_send(mem::replace(frame, held)));
    Poll::Ready(Ok(()))
}

/// The position of the earliest of `firsts`, the first stamp of each frame that a part
/// holds: a frame of no sample first, then the earliest stamp, then the first part.
/// `None` when no part holds a frame.
fn earliest(firsts: impl Iterator<Item = Option<Option<i64>>>) -> Option<usize> {
    firsts
        .enumerate()
        .filter_map(|(at, first)| Some((at, first?)))
        .min_by_key(|&(_, first)| first)
        .map(|(at, _)| at)
}

/// Reads the frames of `reader`, one part of a reader, into the places of `targets`.
/// It decodes each frame into a free frame, gives it on `given`, and takes a free
/// frame from `freed` before it reads again. It runs until the merge drops it: after an
/// error, the merge frees no frame.
#[expect(
    clippy::infinite_loop,
    reason = "the merge drops the feed, and an async fn cannot give `!`"
)]
async fn feed(
    session: program::Session,
    mut reader: program::Reader,
    targets: Targets,
    given: mpsc::Sender<Taken>,
    mut freed: mpsc::Receiver<Frame>,
) -> Infallible {
    let mut frame = Frame::new();
    loop {
        let decoded = match reader.next().await {
            Ok(encoded) => {
                let lost = |entry| reader.lost(entry);
                decoded(reader.places(), &targets, lost, &encoded, &mut frame)
            }
            Err(error) => Err(failed(&session, error)),
        };
        // The merge holds each end of `given` and `freed` while it polls the feed, and
        // it takes each frame before it frees one, so the channel has room.
        given
            .try_send((decoded, frame))
            .expect("invariant: the merge took the last frame");
        frame = freed
            .recv()
            .await
            .expect("invariant: the merge outlives its feeds");
    }
}

/// The samples of each series of `encoded`, a frame of a reader, as a `u32` and a
/// `usize`.
fn count(encoded: &types::frame::Frame) -> (u32, usize) {
    let count = encoded
        .range(0)
        .expect("invariant: a frame of a reader holds the index group")
        .count;
    let samples = usize::try_from(count).expect("invariant: a u32 fits a usize");
    (count, samples)
}

/// Decodes `encoded`, a frame of a part of `places`, into `frame` at the places of
/// `targets`, with the `lost` seqs of each entry with a series. Gives its first stamp.
fn decoded(
    places: &[Place],
    targets: &Targets,
    lost: impl Fn(usize) -> Range<u64>,
    encoded: &types::frame::Frame,
    frame: &mut Frame,
) -> Result<Option<i64>, Error> {
    let (count, samples) = count(encoded);
    frame.count = count;
    frame.index = targets.index;
    if frame.series.len() < targets.places {
        frame.series.resize_with(targets.places, Series::default);
    }
    for series in &mut frame.series {
        series.held = false;
    }
    for (entry, bytes) in encoded.iter() {
        let Some((&first, rest)) = targets.entries[entry].split_first() else {
            continue;
        };
        let series = &mut frame.series[first];
        let data_type = places[entry].data_type;
        codec::decode(data_type, samples, bytes, &mut series.bytes).map_err(
            |cause| Error::Decode {
                place: first,
                cause,
            },
        )?;
        series.lost = lost(entry);
        series.held = true;
        // The places of an entry ascend, so each copy is after the first.
        for &place in rest {
            let (head, tail) = frame.series.split_at_mut(place);
            let (from, to) = (&head[first], &mut tail[0]);
            to.bytes.clone_from(&from.bytes);
            to.lost.clone_from(&from.lost);
            to.held = true;
        }
    }
    Ok(frame.first())
}

#[cfg(test)]
mod tests;
