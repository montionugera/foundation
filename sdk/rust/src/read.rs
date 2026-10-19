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
    /// lost seqs. [`Mode::Latest`] gives the newest frame, and skips frames by design.
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
/// Frames that the reader got and the caller did not read fill the client's receive
/// budget. When it is full, the answers to each writer of the client wait too, so do
/// not write and read on one thread with one client.
#[derive(Debug)]
pub struct Reader {
    places: Vec<Place>,
    calls: mpsc::Sender<Frame>,
    read: Receiver<Read>,
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
        let (answer, read) = sync_channel(1);
        let (sender, opened) = sync_channel(1);
        let open = Open {
            config,
            opened: sender,
            calls: received,
            read: answer,
        };
        opens.send(open.into()).map_err(|_ended| lost(closed))?;
        Ok(Self {
            places: wait(&opened, closed)??,
            calls,
            read,
            closed: Arc::clone(closed),
            frame: Frame::default(),
            ended: None,
        })
    }

    /// The channels of the reader: the key, sample type, and name of each place. The
    /// places of each index are in the order that the node gives, and the indexes are
    /// in the order of their first channel in `Config::channels`. The index of each is
    /// a place, also when `Config::channels` does not name it.
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
    /// `Refused(Refusal::Unknown)` when a channel was removed. After an error, each
    /// call gives the same error.
    #[expect(
        clippy::should_implement_trait,
        reason = "the frame borrows the reader, which `Iterator` cannot give"
    )]
    pub fn next(&mut self) -> Result<&Frame, Error> {
        if let Some(error) = &self.ended {
            return Err(error.clone());
        }
        let frame = mem::take(&mut self.frame);
        // Each call takes its answer, so the channel is never full.
        self.calls
            .try_send(frame)
            .map_err(|_ended| lost(&self.closed))?;
        let (read, frame) = wait(&self.read, &self.closed)?;
        self.frame = frame;
        match read {
            Ok(()) => Ok(&self.frame),
            Err(error) => Err(self.ended.insert(error).clone()),
        }
    }
}

/// A frame of a [`Reader`]: the series of the channels of one index, `count` samples
/// each, at the stamps of the series of the index.
#[derive(Debug, Default)]
pub struct Frame {
    count: u32,
    index: usize,
    /// The series of the frame, in place order, in the first `len`. The rest keep
    /// their bytes for later frames.
    series: Vec<Series>,
    len: usize,
}

/// A series of a [`Frame`].
#[derive(Debug, Default)]
struct Series {
    place: usize,
    /// The samples in the raw form of the type of `place`.
    bytes: Vec<u8>,
    lost: Range<u64>,
}

impl Frame {
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
        self.series[..self.len]
            .iter()
            .find(|series| series.place == place)
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
    read: SyncSender<Read>,
}

/// The outcome of a call of [`Reader::next`], and its frame.
type Read = (Result<(), Error>, Frame);

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
        read,
    } = open;
    let readers = match readers(&session, &config).await {
        Ok(readers) => readers,
        Err(error) => {
            drop(opened.try_send(Err(failed(&session, error))));
            return;
        }
    };
    let places = readers.iter().flat_map(program::Reader::places).cloned();
    drop(opened.try_send(Ok(places.collect())));
    let mut offset = 0;
    let mut parts = Vec::with_capacity(readers.len());
    let mut merged = Vec::with_capacity(readers.len());
    for reader in readers {
        let (given, taken) = mpsc::channel(1);
        let (free, freed) = mpsc::channel(1);
        let places = reader.places().len();
        parts.push(Box::pin(part(
            session.clone(),
            reader,
            offset,
            given,
            freed,
        )));
        merged.push(Merged {
            taken,
            free,
            held: None,
        });
        offset += places;
    }
    while let Some(mut frame) = calls.recv().await {
        let next = poll_fn(|cx| merge(cx, &mut parts, &mut merged, &mut frame)).await;
        let ended = next.is_err();
        drop(read.try_send((next, frame)));
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
/// channel of each index. No request of the node gives the index of a channel, so
/// when one reader of all the channels does not open, it opens one reader of each
/// channel, then one of each index with more than one channel.
async fn readers(
    session: &program::Session,
    config: &Config,
) -> Result<Vec<program::Reader>, program::Error> {
    let names = &config.channels;
    match reader(session, names, config.mode).await {
        Ok(reader) if names.iter().all(|name| placed(&reader, name)) => {
            return Ok(vec![reader]);
        }
        Err(error) if names.len() == 1 => return Err(error),
        _ => {}
    }
    // Each index, its names, and the reader of its name while it has one. At most
    // one reader of each index stays open at the node.
    let mut groups: Vec<(types::channel::Key, Vec<Name>, Option<program::Reader>)> =
        Vec::new();
    for name in names {
        let probe = reader(session, slice::from_ref(name), config.mode).await?;
        let index = index(&probe);
        match groups.iter_mut().find(|(key, ..)| *key == index) {
            Some((_, members, only)) => {
                members.push(name.clone());
                *only = None;
            }
            None => groups.push((index, vec![name.clone()], Some(probe))),
        }
    }
    let mut readers = Vec::with_capacity(groups.len());
    for (_, members, only) in groups {
        readers.push(match only {
            Some(kept) => kept,
            None => reader(session, &members, config.mode).await?,
        });
    }
    Ok(readers)
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

/// Whether `name` is a place of `reader`.
fn placed(reader: &program::Reader, name: &Name) -> bool {
    reader.places().iter().any(|place| place.name == *name)
}

/// The key of the index of `reader`.
fn index(reader: &program::Reader) -> types::channel::Key {
    reader.places()[reader.set().groups()[0]].key
}

/// The frames of one read on their way to the merge, and the frames that it gives
/// back to fill again.
#[derive(Debug)]
struct Merged {
    taken: mpsc::Receiver<Taken>,
    free: mpsc::Sender<Frame>,
    /// The frame that the read gave and the merge did not, with its first stamp.
    held: Option<(Option<i64>, Frame)>,
}

/// A frame of a read with its first stamp, or the error that ended the read with a
/// frame to give back.
type Taken = (Result<Option<i64>, Error>, Frame);

/// Polls each read of `parts` and each frame of `merged` that it holds none of, and
/// when it holds any, swaps the earliest into `frame`.
fn merge<F: Future<Output = Infallible>>(
    cx: &mut std::task::Context<'_>,
    parts: &mut [Pin<Box<F>>],
    merged: &mut [Merged],
    frame: &mut Frame,
) -> Poll<Result<(), Error>> {
    for part in parts.iter_mut() {
        if let Poll::Ready(never) = part.as_mut().poll(cx) {
            match never {}
        }
    }
    for read in merged.iter_mut().filter(|read| read.held.is_none()) {
        if let Poll::Ready(Some((first, taken))) = read.taken.poll_recv(cx) {
            read.held = Some((first?, taken));
        }
    }
    let firsts = merged
        .iter()
        .map(|read| read.held.as_ref().map(|held| held.0));
    let Some(at) = earliest(firsts) else {
        return Poll::Pending;
    };
    let read = &mut merged[at];
    let (_, held) = read
        .held
        .take()
        .expect("invariant: the earliest holds a frame");
    // The read takes its frame back before it reads again, so the channel has room.
    drop(read.free.try_send(mem::replace(frame, held)));
    Poll::Ready(Ok(()))
}

/// The position of the earliest of `firsts`, the first stamp of each frame that a read
/// holds: a frame of no sample first, then the earliest stamp, then the first read.
/// `None` when no read holds a frame.
fn earliest(firsts: impl Iterator<Item = Option<Option<i64>>>) -> Option<usize> {
    firsts
        .enumerate()
        .filter_map(|(at, first)| Some((at, first?)))
        .min_by_key(|&(_, first)| first)
        .map(|(at, _)| at)
}

/// Reads the frames of `reader`, the places of `Reader::places` from `offset` on.
/// It decodes each frame into a free frame, gives it on `given`, and takes a free
/// frame from `freed` before it reads again. It runs until the merge drops it: after an
/// error, the merge frees no frame.
#[expect(
    clippy::infinite_loop,
    reason = "the merge drops the part, and an async fn cannot give `!`"
)]
async fn part(
    session: program::Session,
    mut reader: program::Reader,
    offset: usize,
    given: mpsc::Sender<Taken>,
    mut freed: mpsc::Receiver<Frame>,
) -> Infallible {
    let mut frame = Frame::default();
    loop {
        let read = match reader.next().await {
            Ok(read) => {
                let index = offset + reader.set().groups()[0];
                let lost = |entry| reader.lost(entry);
                decoded(reader.places(), index, lost, offset, &read, &mut frame)
            }
            Err(error) => Err(failed(&session, error)),
        };
        // The merge holds each end of `given` and `freed` while it polls the part, and it
        // takes each frame before it frees one, so the channel has room.
        given
            .try_send((read, frame))
            .expect("invariant: the merge took the last frame");
        frame = freed
            .recv()
            .await
            .expect("invariant: the merge outlives its parts");
    }
}

/// The samples of each series of `read`, a frame of a reader, as a `u32` and a
/// `usize`.
fn count(read: &types::frame::Frame) -> (u32, usize) {
    let count = read
        .range(0)
        .expect("invariant: a frame of a reader holds the index group")
        .count;
    let samples = usize::try_from(count).expect("invariant: a u32 fits a usize");
    (count, samples)
}

/// Decodes `read`, a frame of a reader of `places`, into `frame`, whose index is
/// `index`, with the places from `offset` on and the `lost` seqs of each entry with a
/// series. Gives its first stamp.
fn decoded(
    places: &[Place],
    index: usize,
    lost: impl Fn(usize) -> Range<u64>,
    offset: usize,
    read: &types::frame::Frame,
    frame: &mut Frame,
) -> Result<Option<i64>, Error> {
    let (count, samples) = count(read);
    frame.count = count;
    frame.index = index;
    frame.len = 0;
    for (entry, bytes) in read.iter() {
        if frame.len == frame.series.len() {
            frame.series.push(Series::default());
        }
        let series = &mut frame.series[frame.len];
        series.place = offset + entry;
        let data_type = places[entry].data_type;
        codec::decode(data_type, samples, bytes, &mut series.bytes).map_err(
            |cause| Error::Decode {
                place: series.place,
                cause,
            },
        )?;
        series.lost = lost(entry);
        frame.len += 1;
    }
    Ok(frame.first())
}

#[cfg(test)]
mod tests;
