//! The writes of a client: one write stream each, on the channels of one index.

use std::future::poll_fn;
use std::mem;
use std::pin::pin;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::task::Poll;

pub use program::Place;
pub use program::write::{Outcome, Refusal};
use tokio::sync::{mpsc, watch};
use types::authority::Authority;
use types::frame::{Form, Path};
use types::name::Name;
use types::sample::{Scalar, Type, Value};
use types::time::Span;

use crate::Error;
use program::write::Time;

/// What [`Client::open_writer`](crate::Client::open_writer) opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The channels that the writer writes, at least one, all on one index.
    pub channels: Vec<Name>,
    /// The writer's authority over its channels. [`Config::new`] gives 0, the least:
    /// a writer of a higher authority, such as a connector, takes control from it.
    pub authority: Authority,
    /// How long the writer may go with no write and keep control, or `None` for no
    /// limit, which [`Config::new`] gives. Control ends when the writer closes or
    /// drops.
    pub lease: Option<Span>,
}

impl Config {
    /// A writer of `channels` at authority 0 with no lease.
    #[must_use]
    pub fn new(channels: Vec<Name>) -> Self {
        Self {
            channels,
            authority: Authority(0),
            lease: None,
        }
    }
}

/// A writer of a [`Client`](crate::Client): one write stream on the channels of one
/// index, which the client's thread holds. It can move between threads. Each call
/// blocks until the node answers, and the node applies the frames in the order of the
/// calls. Dropping it stops the stream, and the node ends the writer;
/// [`Writer::close`] first waits until each frame is on disk.
#[derive(Debug)]
pub struct Writer {
    places: Vec<Place>,
    index: usize,
    calls: mpsc::Sender<Call>,
    written: Receiver<Written>,
    /// The bytes of the next frame, which go to the client's thread and back.
    frame: Frame,
}

impl Writer {
    /// Sends the open of `config` on `opens`, and blocks until the client's thread
    /// opened it.
    pub(crate) fn open(
        opens: &mpsc::UnboundedSender<Open>,
        config: Config,
    ) -> Result<Self, Error> {
        let (calls, received) = mpsc::channel(1);
        let (answer, written) = sync_channel(1);
        let (sender, opened) = sync_channel(1);
        let open = Open {
            config: program::write::Config {
                authority: config.authority,
                lease: config.lease,
                channels: config.channels,
            },
            opened: sender,
            calls: received,
            written: answer,
        };
        opens.send(open).map_err(|_closed| Error::Closed)?;
        let (places, index) = wait(&opened)??;
        Ok(Self {
            places,
            index,
            calls,
            written,
            frame: Frame::default(),
        })
    }

    /// The channels of the writer: the key, sample type, and name of each place, in
    /// the order of their keys, not of `Config::channels`. The index is a place, also
    /// when `Config::channels` does not name it, and its samples are the stamps of
    /// each frame.
    #[must_use]
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// Writes a frame of one sample of each place of `values`, in any order, and
    /// blocks until the node took, lost, or refused it. A value of the index place is
    /// the stamp of the frame. With none, the node stamps it: mesh time now, or 1 ns
    /// after the last stamp of this writer when now is not after it.
    ///
    /// # Errors
    ///
    /// [`Error::Value`] when a value is not of the type of its place, with nothing
    /// sent; the writer goes on. [`Error::Closed`] when the client's session ended at
    /// a close or a drop. [`Error::Session`] when the session failed. Else
    /// [`Error::Stream`] with each error of `program::Writer::draft` and
    /// `program::Writer::write`, such as `Layout` for a place past the places or a
    /// place given twice, after which the writer goes on.
    pub fn write(&mut self, values: &[(usize, Value)]) -> Result<Outcome, Error> {
        self.frame.clear(1);
        let mut time = Time::Node;
        for &(place, value) in values {
            let scalar = value.scalar();
            if let Some(at) = self.places.get(place)
                && at.data_type != Type::Scalar(scalar)
            {
                return Err(Error::Value {
                    place,
                    scalar,
                    data_type: at.data_type,
                });
            }
            if place == self.index {
                time = Time::Frame(Path::Live);
            }
            self.frame
                .push(place, scalar.width(), |sample| value.write(sample));
        }
        if time == Time::Node {
            self.frame.push(self.index, Scalar::Stamp.width(), |_| {});
        }
        self.call(time)
    }

    /// Writes a frame of `count` samples of each place of `series`, in any order, each
    /// in the raw form of its place's type, and blocks until the node took, lost, or
    /// refused it. The series of the index place holds the stamps of the frame.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] and [`Error::Session`] as [`Writer::write`] gives them. Else
    /// [`Error::Stream`] with each error of `program::Writer::draft` and
    /// `program::Writer::write`, such as `Layout` for a place past the places or a
    /// place given twice, and `Unsent` for a frame with no series; the writer goes on
    /// after each of these.
    pub fn write_series(
        &mut self,
        count: u32,
        series: &[(usize, &[u8])],
    ) -> Result<Outcome, Error> {
        self.frame.clear(count);
        for &(place, bytes) in series {
            self.frame
                .push(place, bytes.len(), |to| to.copy_from_slice(bytes));
        }
        self.call(Time::Frame(Path::Live))
    }

    /// Finishes the stream, and blocks until each frame that the node took is on disk
    /// and the node finished the stream. Control ends then.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`], [`Error::Session`], and [`Error::Stream`] as
    /// [`Writer::write`] gives them.
    pub fn close(self) -> Result<(), Error> {
        let (sender, finished) = sync_channel(1);
        self.calls
            .try_send(Call::Close(sender))
            .map_err(|_closed| Error::Closed)?;
        wait(&finished)?
    }

    /// Sends the frame with its stamps from `time`, and blocks for its outcome.
    fn call(&mut self, time: Time) -> Result<Outcome, Error> {
        self.frame.sort();
        let frame = mem::take(&mut self.frame);
        // Each call takes its answer, so the channel is never full.
        self.calls
            .try_send(Call::Write { frame, time })
            .map_err(|_closed| Error::Closed)?;
        let (outcome, frame) = wait(&self.written)?;
        self.frame = frame;
        outcome
    }
}

/// Blocks for the answer of the client's thread, or [`Error::Closed`] when the thread
/// dropped its sender.
fn wait<T>(answers: &Receiver<T>) -> Result<T, Error> {
    #[expect(
        clippy::disallowed_methods,
        reason = "a call of a writer blocks its caller, which may run a Tokio runtime"
    )]
    let answer = answers.recv();
    answer.map_err(|_closed| Error::Closed)
}

/// The open of a writer, which the client's thread serves in a task of its own.
#[derive(Debug)]
pub(crate) struct Open {
    config: program::write::Config,
    /// The places of the writer and the place of its index.
    opened: SyncSender<Result<(Vec<Place>, usize), Error>>,
    calls: mpsc::Receiver<Call>,
    written: SyncSender<Written>,
}

/// A call of a [`Writer`] to its task.
#[derive(Debug)]
enum Call {
    Write { frame: Frame, time: Time },
    Close(SyncSender<Result<(), Error>>),
}

/// The outcome of a write, and its frame to fill again.
type Written = (Result<Outcome, Error>, Frame);

/// The bytes of a frame, kept between writes so that a warm write takes no
/// allocation.
#[derive(Debug, Default)]
struct Frame {
    count: u32,
    /// The place, start in `bytes`, and length of each series.
    parts: Vec<(usize, usize, usize)>,
    /// The place and length of each series, in place order, as `Layout` takes them.
    series: Vec<(usize, usize)>,
    bytes: Vec<u8>,
}

impl Frame {
    /// Empties the frame for a frame of `count` samples.
    fn clear(&mut self, count: u32) {
        self.count = count;
        self.parts.clear();
        self.bytes.clear();
    }

    /// Adds a series of `len` bytes at `place`, which `fill` writes.
    fn push(&mut self, place: usize, len: usize, fill: impl FnOnce(&mut [u8])) {
        let start = self.bytes.len();
        self.bytes.resize(start + len, 0);
        fill(&mut self.bytes[start..]);
        self.parts.push((place, start, len));
    }

    /// The place and bytes of each series, in the order of the pushes, or in place
    /// order after [`Frame::sort`].
    fn each(&self) -> impl Iterator<Item = (usize, &[u8])> {
        let bytes = |&(place, start, len): &_| (place, &self.bytes[start..start + len]);
        self.parts.iter().map(bytes)
    }

    /// Puts the series in place order.
    fn sort(&mut self) {
        self.parts.sort_unstable_by_key(|&(place, _, _)| place);
        self.series.clear();
        self.series
            .extend(self.parts.iter().map(|&(place, _, len)| (place, len)));
    }
}

/// Opens the writer of `open` on `session`, and serves the calls of its [`Writer`]
/// until the writer closes or drops, or `closed` changes. Its sender drops at the
/// close of the client, so the writer drops before the session closes.
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
        written,
    } = open;
    let failed = |error| match session.ended() {
        Some(cause) => Error::Session(cause),
        None => Error::Stream(error),
    };
    let mut writer = match session.writer(config).await {
        Ok(writer) => writer,
        Err(error) => {
            drop(opened.try_send(Err(failed(error))));
            return;
        }
    };
    drop(opened.try_send(Ok((writer.places().to_vec(), writer.index()))));
    while let Some(call) = calls.recv().await {
        match call {
            Call::Write { frame, time } => {
                let outcome = sent(&mut writer, &frame, time).await;
                drop(written.try_send((outcome.map_err(failed), frame)));
            }
            Call::Close(finished) => {
                let closed = writer.finish().await.map_err(failed);
                drop(finished.try_send(closed));
                return;
            }
        }
    }
}

/// Drafts `frame` on `writer`, and writes it with its stamps from `time`.
async fn sent(
    writer: &mut program::Writer,
    frame: &Frame,
    time: Time,
) -> Result<Outcome, program::Error> {
    let draft = writer.draft(Form::Raw, frame.count, &frame.series).await?;
    for (place, bytes) in frame.each() {
        draft
            .series_mut(place)
            .expect("invariant: the layout holds each series of the frame")
            .copy_from_slice(bytes);
    }
    writer.write(time).await
}

#[cfg(test)]
mod tests;
