//! The reads of a session: one read stream each, whose frames arrive as the node
//! sends them.

use std::rc::Rc;
use std::sync::Arc;

use transport::Code;
use transport::stream::{Receiver, Sender};
use transport::{Class, Error as Stream};
use types::frame::key_set::{Group, Interner, KeySet};
use types::frame::{Draft, Form, Frame, Layout};
use wire::Protocol;
use wire::header::MALFORMED;
use wire::hub::client::{self, FromNode, Mode, Open, Place, Read};
use wire::hub::{FromHome, Head};

use crate::{Error, Handle};

/// A read of a [`Session`](crate::Session): one read stream. It gives the frames of
/// its channels that the node sends, in order, from the open on. It holds a clone of
/// its session. Dropping it stops the stream, and the node ends the read.
#[derive(Debug)]
pub struct Reader {
    session: Rc<Handle>,
    /// `None` once the read ended.
    stream: Option<(Sender, Receiver)>,
    places: Vec<Place>,
    set: Arc<KeySet>,
    decoder: wire::hub::Reader,
    /// The frame that is arriving: its head, its ends, and its draft.
    head: Option<Head>,
    ends: Vec<(usize, usize)>,
    /// Whether the ends are whole and the draft is not made. A call of `next` that
    /// drops while the draft waits for the pool leaves it set.
    drafting: bool,
    draft: Option<Draft>,
    /// The error that ended the read.
    ended: Option<Error>,
}

impl Reader {
    /// Opens the read of `open` on `session`, whose length is at most
    /// `OPEN_BYTES_MAX`. Gives each error of [`Session::reader`](crate::Session::reader)
    /// but [`Error::Open`], with the error of the close in place of the error that
    /// ended the renewal.
    pub(crate) async fn open(session: Rc<Handle>, open: &Open) -> Result<Self, Error> {
        let shared = &session.0;
        let class = match open.mode {
            Mode::Latest => Class::Latest,
            Mode::Complete => Class::Complete,
        };
        let (mut sender, mut receiver) = shared.session.open(class).await?;
        let (places, set, decoder) =
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
            set,
            decoder,
            head: None,
            ends: Vec::new(),
            drafting: false,
            draft: None,
            ended: None,
        })
    }

    /// The channels of the reader: the key, sample type, and name of each place, in
    /// the order of the entries of [`Reader::set`].
    #[must_use]
    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// The key set of each frame. Its entries are the places, in order.
    #[must_use]
    pub fn set(&self) -> &Arc<KeySet> {
        &self.set
    }

    /// The next frame. Its series are encoded, and its entries index
    /// [`Reader::set`]. It holds a block of the session's pool until it drops, and a
    /// frame that finds the pool full waits for a block. A frame whose call drops goes
    /// on at the next call.
    ///
    /// # Errors
    ///
    /// The error that ended the renewal, when it ended. Else [`Error::Behind`] when a
    /// complete reader missed a frame, [`Error::Refused`] when the node stopped the
    /// stream (`UNKNOWN` when a channel was removed), and [`Error::Transport`],
    /// [`Error::Reply`] for a reply that `wire` refuses or a stream that ends without
    /// `Behind`, [`Error::Frame`], or [`Error::Pool`]. After an error, each call gives
    /// the same error.
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
            if self.drafting {
                let layout =
                    Layout::from_ends(&self.set, &self.ends).map_err(Error::Frame)?;
                let shared = &self.session.0;
                let draft = shared
                    .retry(|| layout.draft(&shared.pool, Form::Encoded))
                    .await?;
                self.drafting = false;
                self.draft = Some(draft);
                if self.decoder.body().is_none() {
                    return Ok(self.freeze());
                }
            }
            if let Some(start) = self.decoder.body() {
                let draft = self
                    .draft
                    .as_mut()
                    .expect("invariant: a body arrives into its draft");
                let body = &mut draft.body_mut()[start..];
                let last = match receiver.recv_into(body).await {
                    Ok(Some(len)) => last(self.decoder.decode(&body[..len]))?,
                    Ok(None) => return Err(finished(&self.decoder)),
                    Err(Stream::TooLarge { .. }) => {
                        let message = recv(receiver, &self.decoder).await?;
                        last(self.decoder.decode(&message))?
                    }
                    Err(error) => return Err(error.into()),
                };
                if last {
                    return Ok(self.freeze());
                }
                continue;
            }
            let message = recv(receiver, &self.decoder).await?;
            match self.decoder.decode(&message).map_err(Error::Reply)? {
                FromHome::Head(head) => {
                    self.head = Some(head);
                    self.ends.clear();
                }
                FromHome::Ends { ends, last } => {
                    self.ends.extend(
                        ends.map(|(place, end)| (to_usize(place), to_usize(end))),
                    );
                    self.drafting = last;
                }
                FromHome::Behind => return Err(Error::Behind),
                FromHome::Opened | FromHome::Body { .. } => {
                    unreachable!("invariant: the decoder gives no opened or body here")
                }
            }
        }
    }

    /// The frame of the head and the draft that arrived.
    fn freeze(&mut self) -> Frame {
        let head = self.head.take().expect("invariant: a frame has a head");
        let mut draft = self.draft.take().expect("invariant: a frame has a draft");
        draft.set_count(0, head.range.count);
        draft.set_seq(0, head.range.seq);
        draft.freeze(head.path)
    }
}

/// Sends the header, the read of `open`, and `open`, then takes the places and
/// `Opened`. Gives the places, their key set, and the decoder of the replies.
async fn start(
    session: &Handle,
    sender: &mut Sender,
    receiver: &mut Receiver,
    open: &Open,
) -> Result<(Vec<Place>, Arc<KeySet>, wire::hub::Reader), Error> {
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
    let mut places = Vec::new();
    let mut index = 0;
    let mut decoder = loop {
        let message = receiver.recv().await?.ok_or(client::Error::Finished)?;
        match decoder.decode(&message)? {
            FromNode::Places(head) => index = to_usize(head.index),
            FromNode::Run { places: run, home } => {
                places.extend(run);
                if let Some(home) = home {
                    break home;
                }
            }
        }
    };
    let set = set(&places, index)?;
    let message = recv(receiver, &decoder).await?;
    match decoder.decode(&message).map_err(Error::Reply)? {
        FromHome::Opened => Ok((places, set, decoder)),
        _ => unreachable!("invariant: the first reply that decodes is Opened"),
    }
}

/// The key set of `places` whose index is at `index`: each place is the entry at its
/// position.
fn set(places: &[Place], index: usize) -> Result<Arc<KeySet>, Error> {
    let mut keys: Vec<_> = places.iter().map(|place| place.key).collect();
    keys.sort_unstable();
    if let Some(&[key, _]) = keys.array_windows().find(|[a, b]| a == b) {
        return Err(Error::Repeated(key));
    }
    let mut interner = Interner::new();
    let slots = interner.slots();
    let mut data = Vec::with_capacity(places.len());
    for (at, place) in places.iter().enumerate() {
        if at == index {
            slots.index(place.key);
        } else {
            slots.data(place.key, place.data_type);
            data.push((place.key, place.data_type));
        }
    }
    let group = Group {
        index: places[index].key,
        data: &data,
    };
    Ok(interner.intern(&[group]))
}

/// Ends `stream` after `error`: stops it as `MALFORMED` when the node broke a rule of
/// the wire, else drops it.
fn end((sender, receiver): (Sender, Receiver), error: &Error) {
    if matches!(
        error,
        Error::Message(_) | Error::Reply(_) | Error::Frame(_) | Error::Repeated(_)
    ) {
        sender.reset(Code(MALFORMED));
        receiver.stop(Code(MALFORMED));
    }
}

/// The next message of `receiver`, whose replies `decoder` decodes.
async fn recv(
    receiver: &mut Receiver,
    decoder: &wire::hub::Reader,
) -> Result<block::Block, Error> {
    receiver.recv().await?.ok_or_else(|| finished(decoder))
}

/// The error of a stream that the node finished where `decoder` is.
fn finished(decoder: &wire::hub::Reader) -> Error {
    Error::Reply(
        decoder
            .end()
            .expect_err("invariant: a reader reads no message after Behind"),
    )
}

/// Whether the body message that `decoded` gave is the last of its body.
fn last(decoded: Result<FromHome<'_>, wire::hub::Error>) -> Result<bool, Error> {
    match decoded.map_err(Error::Reply)? {
        FromHome::Body { last, .. } => Ok(last),
        _ => unreachable!("invariant: inside a body the decoder gives body bytes"),
    }
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).expect("invariant: a u32 fits a usize")
}
