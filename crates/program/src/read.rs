//! The reads of a session: one read stream each, whose frames arrive as the node
//! sends them.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use transport::Code;
use transport::stream::{Receiver, Sender};
use transport::{Class, Error as Stream};
use types::frame::key_set::KeySet;
use types::frame::{Form, Frame};
use wire::Protocol;
use wire::header::MALFORMED;
use wire::hub::client::{self, Mode, Open, Place, Read};
use wire::hub::{Due, Frames, Step};

use crate::{Error, Handle};

/// A read of a [`Session`](crate::Session): one read stream. It gives the frames of
/// its channels that the node sends, in order, from the open on. A complete reader
/// goes on past seqs that the node's buffer lost: [`Reader::lost`] gives the seqs lost
/// just before each frame, and the reader writes a warning for each gap. A latest
/// reader skips frames by design and gives no gap. It holds a clone of its session.
/// Dropping it stops the stream, and the node ends the read.
#[derive(Debug)]
pub struct Reader {
    session: Rc<Handle>,
    /// `None` once the read ended.
    stream: Option<(Sender, Receiver)>,
    places: Vec<Place>,
    frames: Frames,
    /// The error that ended the read.
    ended: Option<Error>,
}

impl Reader {
    /// Opens the read of `open` on `session`, whose length is at most
    /// `OPEN_BYTES_MAX`. Gives each error of
    /// [`Session::reader`](crate::Session::reader) but [`Error::Open`], with the error
    /// of the close in place of the error that ended the renewal.
    pub(crate) async fn open(session: Rc<Handle>, open: &Open) -> Result<Self, Error> {
        let shared = &session.0;
        let class = match open.mode {
            Mode::Latest => Class::Latest,
            Mode::Complete => Class::Complete,
        };
        let (mut sender, mut receiver) = shared.session.open(class).await?;
        let (places, frames) =
            match start(&session, &mut sender, &mut receiver, open).await {
                Ok(started) => started,
                Err(error) => {
                    end((sender, receiver), &error);
                    return Err(error);
                }
            };
        Ok(Self {
            session,
            stream: Some((sender, receiver)),
            places,
            frames,
            ended: None,
        })
    }

    /// The channels of the reader: the key, sample type, and name of each place, in
    /// the order of the entries of [`Reader::set`].
    #[must_use]
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// The key set of each frame, from [`KeySet::ordered`]. Its entries are the
    /// places, in order. Its slots are the reader's own, not the node's, and its key is
    /// the one key of each set from `KeySet::ordered`, so it does not tell two readers
    /// apart.
    #[must_use]
    pub fn set(&self) -> &Arc<KeySet> {
        self.frames.set()
    }

    /// The seqs of the index lost just before the frame that the last
    /// [`Reader::next`] gave, as the node's buffer did not keep them. Each channel of
    /// that frame lost these seqs. `0..0` when none, before the first frame, and for a
    /// latest reader.
    #[must_use]
    pub fn lost(&self) -> Range<u64> {
        self.frames.lost()
    }

    /// The next frame. Its series are encoded, and its entries index
    /// [`Reader::set`]. It holds a block of [`Config::frames`](crate::Config::frames)
    /// until it drops, and a frame that finds the pool full waits for a block. A frame
    /// whose call drops goes on at the next call. After a gap, [`Reader::lost`] gives
    /// it.
    ///
    /// # Errors
    ///
    /// The error that ended the renewal, when it ended. Else [`Error::Refused`] when
    /// the node stopped the stream (`UNKNOWN` when a channel was removed), and
    /// [`Error::Transport`], [`Error::Reply`] for a reply that `wire` refuses or a
    /// stream that ends early, [`Error::Frame`], or [`Error::Pool`]. After an error,
    /// each call gives the same error.
    pub async fn next(&mut self) -> Result<Frame, Error> {
        if let Some(error) = &self.ended {
            return Err(error.clone());
        }
        let error = match self.frame().await {
            Ok(frame) => return Ok(frame),
            Err(error) => error,
        };
        if let Some(stream) = self.stream.take() {
            end(stream, &error);
        }
        let ended = self.session.0.ended.borrow().clone().unwrap_or(error);
        self.ended = Some(ended.clone());
        Err(ended)
    }

    async fn frame(&mut self) -> Result<Frame, Error> {
        let (_, receiver) = self
            .stream
            .as_mut()
            .expect("invariant: a reader holds its stream until it ends");
        loop {
            let step = match self.frames.due().map_err(Error::Frame)? {
                Due::Draft(layout) => {
                    let shared = &self.session.0;
                    let draft = shared
                        .retry(|| layout.draft(&shared.frames, Form::Encoded))
                        .await?;
                    self.frames.draft(draft)
                }
                Due::Body(body) => match receiver.recv_into(body).await {
                    Ok(Some(len)) => self.frames.received(len).map_err(Error::Reply)?,
                    Ok(None) => return Err(Error::Reply(self.frames.finished())),
                    Err(Stream::TooLarge { .. }) => {
                        let message = recv(receiver, &self.frames).await?;
                        self.frames.decode(&message).map_err(Error::Reply)?
                    }
                    Err(error) => return Err(error.into()),
                },
                Due::Message => {
                    let message = recv(receiver, &self.frames).await?;
                    self.frames.decode(&message).map_err(Error::Reply)?
                }
            };
            match step {
                Step::Head(_) | Step::More => {}
                Step::Frame(frame) => {
                    self.warn();
                    return Ok(frame);
                }
                // #2790 removes this arm.
                Step::Behind => {
                    return Err(Error::Reply(wire::hub::Error::Kind { kind: 3 }));
                }
            }
        }
    }

    /// Writes a warning when the last frame lost seqs.
    fn warn(&self) {
        let lost = self.frames.lost();
        if lost.is_empty() {
            return;
        }
        let index = &self.places[self.set().groups()[0]].name;
        let (start, end) = (lost.start, lost.end);
        tracing::warn!(
            %index,
            start,
            end,
            "the reader lost seqs {start}..{end} of index {index}: the node's buffer did \
             not keep them"
        );
    }
}

/// Sends the header, the read of `open`, and `open`, then takes the places and
/// `Opened`. Gives the places and the frames of the read.
async fn start(
    session: &Handle,
    sender: &mut Sender,
    receiver: &mut Receiver,
    open: &Open,
) -> Result<(Vec<Place>, Frames), Error> {
    let shared = &session.0;
    let mut body = vec![0; open.encoded_len()];
    open.encode(&mut body);
    let signature = shared.pair.sign(&access::proof::signed(
        access::proof::Kind::Read,
        shared.connection,
        &body,
    ));
    let length = u64::try_from(body.len()).expect("invariant: the caller capped it");
    let mut read = [0; Read::LEN];
    Read { length, signature }.encode(&mut read);
    shared
        .send(sender, &wire::header::encode(Protocol::Hub))
        .await?;
    shared.send(sender, &read).await?;
    shared.send_chunks(sender, &body).await?;
    let mut decoder = client::Reader::new(open.mode);
    let start = loop {
        let message = receiver.recv().await?;
        if let Some(start) = decoder.decode(message.as_deref())? {
            break start;
        }
    };
    let message = receiver.recv().await?;
    let (frames, _) = Frames::open(start.home, start.set, message.as_deref())
        .map_err(Error::Reply)?;
    Ok((start.places, frames))
}

/// Ends `stream` after `error`: stops it as `MALFORMED` when the node broke a rule of
/// the wire, else drops it.
fn end((sender, receiver): (Sender, Receiver), error: &Error) {
    if matches!(error, Error::Message(_) | Error::Reply(_) | Error::Frame(_)) {
        sender.reset(Code(MALFORMED));
        receiver.stop(Code(MALFORMED));
    }
}

/// The next message of `receiver`, whose replies `frames` decodes.
async fn recv(receiver: &mut Receiver, frames: &Frames) -> Result<block::Block, Error> {
    receiver
        .recv()
        .await?
        .ok_or_else(|| Error::Reply(frames.finished()))
}
