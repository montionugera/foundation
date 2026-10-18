//! The writes of a session: one write stream each, on the channels of one index.

use std::rc::Rc;
use std::sync::Arc;

use transport::stream::{Part, Receiver, Sender};
use transport::{Class, Code};
use types::authority::Authority;
use types::frame::key_set::KeySet;
use types::frame::{self, Draft, Form, Path};
use types::name::Name;
use types::time::Span;
use wire::hub::client::Place;
use wire::hub::client::write::{Open, Opening, Start, Write};
use wire::hub::ends;
use wire::hub::write::{Head, Reply};
pub use wire::hub::write::{Outcome, Refusal, Time};

use crate::{Error, Handle, end};

/// What a [`Writer`] opens with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The writer's authority over its channels.
    pub authority: Authority,
    /// How long the writer may go without a write and keep control, or `None` for no
    /// limit.
    pub lease: Option<Span>,
    /// The channels that the writer writes, at least one, all on one index.
    pub channels: Vec<Name>,
}

/// A writer of a [`Session`](crate::Session): one write stream on the channels of one
/// index, at its home. The home applies its frames in the order of the calls. It
/// holds a clone of its session. Dropping it stops the stream, and the home ends the
/// writer.
#[derive(Debug)]
pub struct Writer {
    session: Rc<Handle>,
    /// `None` once the writer ended.
    stream: Option<(Sender, Receiver)>,
    places: Vec<Place>,
    set: Arc<KeySet>,
    /// The frame that the next write sends.
    draft: Option<Draft>,
    /// The frames sent whole.
    sent: u64,
    /// The frames that got an outcome.
    answered: u64,
    /// The count of frames up to the last that the home took.
    taken: u64,
    /// The frames that the last `Stored` covers.
    stored: u64,
    /// A call dropped while it sent a frame, so the stream holds part of it.
    cut: bool,
    /// The error that ended the writer.
    ended: Option<Error>,
}

impl Writer {
    /// Opens the write of `open` on `session`, whose length is at most
    /// `OPEN_BYTES_MAX`. Gives each error of
    /// [`Session::writer`](crate::Session::writer) but [`Error::Unnamed`] and
    /// [`Error::Open`], with the error of the close in place of the error that ended
    /// the renewal. Dropping the stream on `Pool` stops it.
    pub(crate) async fn open(session: Rc<Handle>, open: &Open) -> Result<Self, Error> {
        let (mut sender, mut receiver) =
            session.0.session.open(Class::Complete).await?;
        let Start { places, set } =
            match start(&session, &mut sender, &mut receiver, open).await {
                Ok(started) => started,
                Err(error) => {
                    end((sender, receiver), &error);
                    return Err(error);
                }
            };
        let lead = Head::LEN + places.len() * ends::LEN;
        let largest = session.0.pool.largest();
        if lead > largest {
            return Err(Error::Pool(block::Error::TooLarge {
                requested: lead,
                largest,
            }));
        }
        Ok(Self {
            session,
            stream: Some((sender, receiver)),
            places,
            set,
            draft: None,
            sent: 0,
            answered: 0,
            taken: 0,
            stored: 0,
            cut: false,
            ended: None,
        })
    }

    /// The channels of the writer: the key, sample type, and name of each place.
    /// Place `n` is entry `n` of each draft.
    #[must_use]
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// The draft that the next [`Writer::write`] sends, to fill: a new frame of the
    /// writer's key set from [`Config::frames`](crate::Config::frames), as
    /// [`Draft::new`] takes `series`. The entry of place `n` is `n`. Each frame holds
    /// the index series. Its range starts at 0 samples: set the count of group 0 with
    /// [`Draft::set_count`]. It drops a held draft that no write sent, and does not
    /// wait for a block. The writer holds the draft, so a draft of another writer
    /// reaches its write only through a swap of two `&mut Draft`.
    ///
    /// # Errors
    ///
    /// As [`Draft::new`]: [`frame::Error::Pool`] when the pool has no block now. The
    /// writer then holds no draft.
    pub fn draft(
        &mut self,
        form: Form,
        series: &[(usize, usize)],
    ) -> Result<&mut Draft, frame::Error> {
        self.draft = None;
        let draft = Draft::new(&self.session.0.frames, &self.set, form, series)?;
        Ok(self.draft.insert(draft))
    }

    /// Sends the writer's draft with its stamps from `time`, and waits for what the
    /// home did with it. A frame of [`Time::Node`] holds one sample of each series,
    /// in the raw form, and the home writes a stamp over its index sample: mesh time
    /// now, or 1 ns after the last stamp of this writer when now is not after it.
    /// Behind a later stamp of another writer, the home refuses it with
    /// [`Refusal::Order`]. A refused or lost frame leaves the writer open. A call
    /// dropped after it sent its frame loses only its outcome; a call dropped while it
    /// sends ends the writer. A call takes the draft at its first poll, so after each
    /// result the writer holds no draft: the next write needs a new call of
    /// [`Writer::draft`].
    ///
    /// # Errors
    ///
    /// [`Error::Unsent`] for a frame of no sample, or a frame of [`Time::Node`] that is
    /// encoded or holds more than one sample, with nothing sent and the draft dropped;
    /// the writer goes on. Else the error that ended the writer, given again at each
    /// later call: the error that ended the renewal, or [`Error::Refused`] when the
    /// node stopped the stream (`UNKNOWN` when a channel was removed, `FAILED` when the
    /// home failed, `BUSY` when the home has no block for the outcome of a frame,
    /// `MALFORMED` for a frame that breaks a rule of the wire), [`Error::Transport`] (a
    /// reset of `Code(0)` after a call dropped while it sent), and [`Error::Reply`] for
    /// a reply that `wire` refuses or a stream that ends early.
    ///
    /// # Panics
    ///
    /// When the writer holds no draft.
    pub async fn write(&mut self, time: Time) -> Result<Outcome, Error> {
        let frame = self
            .draft
            .take()
            .expect("a write sends the draft of a call of `Writer::draft`");
        self.check()?;
        let count = frame
            .range(0)
            .expect("invariant: a draft of a writer holds its index")
            .count;
        let form = frame.form();
        if count == 0 || time == Time::Node && (form != Form::Raw || count != 1) {
            return Err(Error::Unsent { count, form });
        }
        let written = match self.send(time, frame, count).await {
            Ok(()) => self.outcome().await,
            Err(error) => Err(error),
        };
        self.ending(written)
    }

    /// Waits until each frame that the home took before the call is on disk. Gives
    /// at once when the home took no frame. A call dropped before it gave loses
    /// nothing: the next call takes the replies that it did not.
    ///
    /// # Errors
    ///
    /// The error that ended the writer, as [`Writer::write`] gives it.
    pub async fn stored(&mut self) -> Result<(), Error> {
        self.check()?;
        let mut waited = Ok(());
        while self.answered < self.sent || self.stored < self.taken {
            if let Err(error) = self.reply().await {
                waited = Err(error);
                break;
            }
        }
        self.ending(waited)
    }

    /// The error that ended the writer, and ends it when a call dropped while it sent.
    fn check(&mut self) -> Result<(), Error> {
        if let Some(error) = &self.ended {
            return Err(error.clone());
        }
        if self.cut {
            let (sender, _) = self
                .stream
                .take()
                .expect("invariant: a cut writer holds its stream");
            sender.reset(Code(0));
            let error = Error::Transport(transport::Error::Reset { code: Code(0) });
            return self.ending(Err(error));
        }
        Ok(())
    }

    /// `result`, and ends the writer when it is an error.
    fn ending<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        let error = match result {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        self.cut = false;
        if let Some(stream) = self.stream.take() {
            end(stream, &error);
        }
        let ended = self.session.0.ended.borrow().clone().unwrap_or(error);
        self.ended = Some(ended.clone());
        Err(ended)
    }

    /// Sends the head, the ends, and the body of `frame`, whose group 0 holds `count`
    /// samples. The body goes from the frame's own block.
    async fn send(
        &mut self,
        time: Time,
        frame: Draft,
        count: u32,
    ) -> Result<(), Error> {
        let Self {
            session,
            stream,
            cut,
            ..
        } = self;
        let shared = &session.0;
        let (sender, _) = stream
            .as_mut()
            .expect("invariant: a writer holds its stream until it ends");
        let head = Head {
            time,
            form: frame.form(),
            count,
            series: to_u32(frame.iter().count()),
        };
        let frame = frame.freeze(match time {
            Time::Frame(path) => path,
            Time::Node => Path::Live,
        });
        let run = to_usize(head.series) * ends::LEN;
        // The open checked that a block of the pool holds the lead.
        let mut lead = shared.alloc(Head::LEN + run).await?;
        let (bytes, rest) = lead.split_at_mut(Head::LEN);
        head.encode(bytes);
        ends::encode(
            frame
                .ends()
                .map(|(entry, end)| (to_u32(entry), to_u32(end))),
            rest,
        );
        let lead = lead.freeze();
        let most = sender.bytes_max();
        let run_most = most / ends::LEN * ends::LEN;
        *cut = true;
        send_range(sender, &lead, 0..Head::LEN).await?;
        for at in (Head::LEN..lead.len()).step_by(run_most) {
            send_range(sender, &lead, at..lead.len().min(at + run_most)).await?;
        }
        let body = frame.body();
        for at in (0..body.len()).step_by(most) {
            send_range(sender, &body, at..body.len().min(at + most)).await?;
        }
        *cut = false;
        self.sent += 1;
        Ok(())
    }

    /// Takes replies until the outcome of the last frame sent.
    async fn outcome(&mut self) -> Result<Outcome, Error> {
        loop {
            if let Reply::Outcome(outcome) = self.reply().await?
                && self.answered == self.sent
            {
                return Ok(outcome);
            }
        }
    }

    /// Takes the next reply, and counts it.
    async fn reply(&mut self) -> Result<Reply, Error> {
        let (_, receiver) = self
            .stream
            .as_mut()
            .expect("invariant: a writer holds its stream until it ends");
        let message = receiver
            .recv()
            .await?
            .ok_or(Error::Reply(wire::hub::Error::Finished))?;
        let reply = Reply::decode(&message).map_err(Error::Reply)?;
        match reply {
            Reply::Outcome(outcome) => {
                self.answered += 1;
                if let Outcome::Taken { .. } = outcome {
                    self.taken = self.answered;
                }
            }
            Reply::Stored { frames } => self.stored = frames,
        }
        Ok(reply)
    }
}

/// Sends `range` of `block` on `sender` as one message, with no copy.
async fn send_range(
    sender: &mut Sender,
    block: &block::Block,
    range: std::ops::Range<usize>,
) -> Result<(), Error> {
    let part = Part { range, zeros: 0 };
    Ok(sender.send_parts(block.clone(), &[part]).await?)
}

/// Sends the write of `open` and `open`, then takes the places and their run.
async fn start(
    session: &Handle,
    sender: &mut Sender,
    receiver: &mut Receiver,
    open: &Open,
) -> Result<Start, Error> {
    let mut body = vec![0; open.encoded_len()];
    open.encode(&mut body);
    session
        .0
        .open(
            sender,
            access::proof::Kind::Write,
            &body,
            |length, signature, out: &mut [u8; Write::LEN]| {
                Write { length, signature }.encode(out);
            },
        )
        .await?;
    let mut decoder = Opening::default();
    loop {
        let message = receiver.recv().await?;
        if let Some(start) = decoder.decode(message.as_deref())? {
            return Ok(start);
        }
    }
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).expect("invariant: a frame of a writer fits the wire")
}

fn to_usize(value: u32) -> usize {
    usize::try_from(value).expect("invariant: a u32 fits a usize")
}
