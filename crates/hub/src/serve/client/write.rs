//! The write stream of a client session.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;

use access::proof;
use transport::stream::{Receiver, Sender};
use types::frame::key_set::KeySet;
use types::frame::{Draft, Label, Path};
use types::name::Name;
use types::time::{Span, Stamp};
use wire::hub::client::write::{Open, Write};
use wire::hub::client::{Place, Places, place};
use wire::hub::write::{Due, Frames, Outcome, Refusal, Reply, Step, Time};

use super::{Reserved, body, verify};
use crate::State;
use crate::link::Session;
use crate::serve::{Error, alloc, unless};
use crate::writer::{self, Failure, Homed, Writer};

/// The bytes that a write holds in the body budget for each channel of its open and
/// for its index, at least what [`Frames`] holds for each place.
const PLACE_BYTES: u64 = 16;

/// Serves the write of `subject` that `write` starts: verifies its open, opens its
/// writer, and sends its places, then writes each frame of the program and sends its
/// outcome, and `Stored` after each commit, until the program finishes the stream.
pub(super) async fn serve(
    session: &Session,
    subject: Name,
    write: Write,
    receiver: &mut Receiver,
    sender: &mut Sender,
) -> Result<(), Error> {
    let state = &session.state;
    let open = open(session, subject.clone(), write, receiver).await?;
    let places =
        u64::try_from(open.channels.len()).expect("invariant: a u64 holds a usize");
    let _reserved = Reserved::new(state, subject.clone(), (places + 1) * PLACE_BYTES)?;
    let Some(writer) = here(state, subject, open, receiver).await? else {
        sender.finish()?;
        return Ok(());
    };
    start(state, sender, writer.set()).await?;
    relay(state, writer, receiver, sender).await
}

/// Reads, verifies, and decodes the open, and holds its body in the budget until then.
async fn open(
    session: &Session,
    subject: Name,
    write: Write,
    receiver: &mut Receiver,
) -> Result<Open, Error> {
    let _reserved = Reserved::new(&session.state, subject, write.length)?;
    let bytes = body(receiver, write.body()).await?;
    verify(session, proof::Kind::Write, &bytes, &write.signature)?;
    Ok(Open::decode(&bytes)?)
}

/// Opens the writer of `open` at this node, the home of its index, or gives `None`
/// when the program finishes first. A writer on more than one index gives
/// [`Error::ManyIndexes`] before it opens, so it takes no control.
async fn here(
    state: &Rc<RefCell<State>>,
    subject: Name,
    open: Open,
    receiver: &mut Receiver,
) -> Result<Option<Writer>, Error> {
    let refused = |error| Error::Writer(Box::new(error));
    let config = writer::Config {
        subject,
        authority: open.authority,
        lease: open.lease,
        channels: open.channels,
    };
    let program = pin!(async {
        match receiver.recv().await? {
            None => Ok(()),
            Some(_) => Err(Error::Client(wire::hub::client::Error::Trailing)),
        }
    });
    let Some(homed) = unless(program, Homed::new(state, config)).await? else {
        return Ok(None);
    };
    let homed = homed.map_err(refused)?;
    if homed.indexes() > 1 {
        return Err(Error::ManyIndexes);
    }
    homed.open().map(Some).map_err(refused)
}

/// Sends the head of the places of `set` and their run: place `n` is entry `n`. Call
/// it with no wait after the writer opens, while each channel of `set` is defined.
async fn start(
    state: &RefCell<State>,
    sender: &mut Sender,
    set: &KeySet,
) -> Result<(), Error> {
    let places: Vec<Place> = {
        let channels = &state.borrow().channels;
        let place = |key| Place {
            key,
            data_type: channels.known(key).sample(),
            name: channels.name(key).clone(),
        };
        set.entries().iter().map(|entry| place(entry.key)).collect()
    };
    let fit = "invariant: an open of a write holds fewer than u32::MAX channels";
    let head = Places {
        count: u32::try_from(places.len()).expect(fit),
        index: u32::try_from(set.groups()[0]).expect(fit),
    };
    let mut block = alloc(state, Places::LEN)?;
    head.encode(&mut block);
    sender.send(block.freeze()).await?;
    let most = sender.bytes_max().min(state.borrow().home.pool().largest());
    for message in place::messages(&places, most) {
        let mut block = alloc(state, message.encoded_len())?;
        message.encode(&mut block);
        sender.send(block.freeze()).await?;
    }
    Ok(())
}

/// Writes each frame of the program with `writer` and sends its outcome, until the
/// program finishes the stream. Then sends `Stored` for each taken frame and finishes
/// the stream.
async fn relay(
    state: &RefCell<State>,
    mut writer: Writer,
    receiver: &mut Receiver,
    sender: &mut Sender,
) -> Result<(), Error> {
    let mut frames = Frames::new(Arc::clone(writer.set()));
    let index = writer.set().groups()[0];
    let mut stored = Stored::default();
    let mut stamped: Option<Stamp> = None;
    loop {
        let step = match frames.due().map_err(Error::Frame)? {
            Due::Message => {
                let Some(message) = stored.race(state, sender, receiver.recv()).await?
                else {
                    frames.finish()?;
                    return stored.finish(state, sender).await;
                };
                frames.decode(&message)?
            }
            Due::Body(rest) => {
                match stored.race(state, sender, receiver.recv_into(rest)).await {
                    Ok(Some(len)) => frames.received(len)?,
                    Ok(None) => {
                        frames.finish()?;
                        unreachable!("invariant: a stream that ends in a body is cut")
                    }
                    // The message stays queued, and its decode gives its error.
                    Err(Error::Stream(transport::Error::TooLarge { .. })) => {
                        let message =
                            stored.race(state, sender, receiver.recv()).await?;
                        let message =
                            message.expect("invariant: the message is queued");
                        frames.decode(&message)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Due::Draft { layout, form } => {
                let drafted = layout.draft(state.borrow().home.pool(), form);
                match drafted {
                    Ok(draft) => frames.draft(draft),
                    Err(error) => {
                        frames.skip();
                        let refusal = match error {
                            block::Error::TooLarge { .. } => Refusal::Large,
                            block::Error::Exhausted { .. }
                            | block::Error::Refused { .. } => Refusal::Full,
                        };
                        send(state, sender, Reply::Outcome(Outcome::Refused(refusal)))
                            .await?;
                        Step::More
                    }
                }
            }
        };
        let Step::Frame { mut draft, time } = step else {
            continue;
        };
        let (label, stamp) = match time {
            Time::Frame(path) => (Label::Path(path), None),
            Time::Node => {
                let stamp = next(writer.now(), stamped);
                stamp_index(&mut draft, index, stamp);
                (Label::Path(Path::Live), Some(stamp))
            }
        };
        let outcome = outcome(writer.write(label, draft))?;
        if let Outcome::Taken { range } = outcome {
            stored.taken(state, range.seq + u64::from(range.count) - 1);
            stamped = stamp.or(stamped);
        }
        send(state, sender, Reply::Outcome(outcome)).await?;
    }
}

/// The stamp of a frame of [`Time::Node`] at mesh time `now`: `now`, or 1 ns after
/// `stamped`, the last stamp of the stream that the home took, when `now` is not
/// after it.
fn next(now: Stamp, stamped: Option<Stamp>) -> Stamp {
    match stamped {
        Some(last) if now <= last => last + Span::from_nanos(1),
        _ => now,
    }
}

/// Writes `stamp` over the one sample of entry `index` of `draft`.
fn stamp_index(draft: &mut Draft, index: usize, stamp: Stamp) {
    let sample = draft
        .series_mut(index)
        .expect("invariant: a frame of a write holds its index");
    sample.copy_from_slice(&stamp.nanos().to_le_bytes());
}

/// The outcome of a frame from what the write gave, or the error that stops the
/// stream.
fn outcome(written: Result<&[::home::Outcome], Failure>) -> Result<Outcome, Error> {
    let outcomes = match written {
        Ok(outcomes) => outcomes,
        Err(Failure::Removed(key)) => return Err(Error::Removed(key)),
        Err(Failure::Home(::home::Error::Disk(error))) => {
            return Err(Error::Buffer(error));
        }
        Err(Failure::Home(::home::Error::Full)) => {
            return Ok(Outcome::Refused(Refusal::Full));
        }
        Err(Failure::Home(::home::Error::Large)) => {
            return Ok(Outcome::Refused(Refusal::Large));
        }
        Err(Failure::Home(::home::Error::Resend)) => {
            unreachable!("invariant: each frame of a write is labeled with a path")
        }
    };
    let [outcome] = outcomes else {
        unreachable!("invariant: a frame of a write holds one group")
    };
    Ok(match *outcome {
        ::home::Outcome::Applied { range, .. } => Outcome::Taken { range },
        ::home::Outcome::Lost { range, .. } => Outcome::Lost { range },
        ::home::Outcome::Refused { ref refusal, .. } => {
            Outcome::Refused(match refusal {
                ::home::Refusal::Waiting => Refusal::Waiting,
                ::home::Refusal::Reserved => Refusal::Reserved,
                ::home::Refusal::Expired => Refusal::Expired,
                ::home::Refusal::Order(_) => Refusal::Order,
                ::home::Refusal::Codec { .. } => Refusal::Codec,
            })
        }
    })
}

/// The commits of the frames that a write took, and their `Stored` replies.
#[derive(Default)]
struct Stored {
    /// The last seq taken since the call of the commit in `due`.
    taken: Option<u64>,
    /// The commit that is due, and the last seq taken at its call.
    due: Option<(::home::Commit, u64)>,
}

impl Stored {
    /// Notes that the home took each seq through `last`.
    fn taken(&mut self, state: &RefCell<State>, last: u64) {
        self.taken = Some(last);
        self.call(state);
    }

    /// Calls a commit for the taken seqs when none is due.
    fn call(&mut self, state: &RefCell<State>) {
        if self.due.is_none()
            && let Some(through) = self.taken.take()
        {
            self.due = Some((state.borrow().home.committed(), through));
        }
    }

    /// Waits for `received`, and sends `Stored` at each commit that ends meanwhile.
    async fn race<T>(
        &mut self,
        state: &RefCell<State>,
        sender: &mut Sender,
        received: impl Future<Output = Result<T, transport::Error>>,
    ) -> Result<T, Error> {
        let mut received = pin!(received);
        loop {
            let Some((commit, through)) = &mut self.due else {
                return Ok(received.await?);
            };
            let through = *through;
            let committed = poll_fn(|cx| {
                if let Poll::Ready(committed) = Pin::new(&mut *commit).poll(cx) {
                    return Poll::Ready(Err(committed));
                }
                received.as_mut().poll(cx).map(Ok)
            });
            match committed.await {
                Ok(received) => return Ok(received?),
                Err(committed) => {
                    committed.map_err(Error::Buffer)?;
                    self.due = None;
                    self.call(state);
                    send(state, sender, Reply::Stored { through }).await?;
                }
            }
        }
    }

    /// Waits for the commit of each taken seq, sends its `Stored`, and finishes the
    /// stream.
    async fn finish(
        mut self,
        state: &RefCell<State>,
        sender: &mut Sender,
    ) -> Result<(), Error> {
        self.call(state);
        while let Some((commit, through)) = self.due.take() {
            commit.await.map_err(Error::Buffer)?;
            self.call(state);
            send(state, sender, Reply::Stored { through }).await?;
        }
        sender.finish()?;
        Ok(())
    }
}

/// Sends `reply` in a block from the home's pool.
async fn send(
    state: &RefCell<State>,
    sender: &mut Sender,
    reply: Reply,
) -> Result<(), Error> {
    let mut block = alloc(state, reply.encoded_len())?;
    reply.encode(&mut block);
    sender.send(block.freeze()).await?;
    Ok(())
}

/// The refusal that stops a write whose writer did not open with `error`.
///
/// # Panics
///
/// For `Empty`, as an open names at least one channel, and for a `Home` error of
/// `Unsynced`, as the writer opens once the node has mesh time.
pub(in crate::serve) fn refused(error: &writer::Error) -> wire::hub::client::Refusal {
    use wire::hub::client::Refusal as Stop;
    match error {
        writer::Error::Unknown(_) => Stop::Unknown,
        writer::Error::Remote { .. } => Stop::NotHome,
        writer::Error::Mesh(_) => Stop::Failed,
        writer::Error::Home(::home::writer::Error::Lease { .. }) => Stop::Malformed,
        writer::Error::Empty | writer::Error::Home(::home::writer::Error::Unsynced) => {
            panic!("invariant: a write does not give {error:?}")
        }
    }
}
