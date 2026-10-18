//! The blob protocol between this node's store and a peer, over one stream per call.

use std::fmt;

use block::Block;
use env::files;
use transport::stream::{Incoming, Part, Receiver, Sender};
use transport::{Class, Code, Session};
use types::digest::Digest;
use wire::Protocol;
use wire::blob::{
    Body, FULL, FromRequester, MISMATCH, Put, Reply, Requester, Server, TOO_LARGE, get,
};
use wire::header::MALFORMED;

use crate::Store;

/// Serves `incoming`, a blob stream whose header the caller read, from `store`: it
/// answers each get and stores each put, in order, until the peer's half ends. A put
/// gets its reply only once the chunk is durable. A drop of the future ends both
/// halves with code 0, and a put in flight ends as a dropped [`Store::put`].
///
/// # Errors
///
/// The [`Error`] that ended the stream. The stream stops with the code of a refusal
/// ([`Error::Wire`], [`Error::OneWay`], and [`Error::Store`] with `Mismatch`, `Floor`,
/// or a full disk), and with code 0 after another store error.
pub async fn serve(store: &Store, incoming: Incoming) -> Result<(), Error> {
    let Incoming {
        mut receiver,
        sender,
        ..
    } = incoming;
    let Some(mut sender) = sender else {
        receiver.stop(Code(MALFORMED));
        return Err(Error::OneWay);
    };
    let answered = answer(store, &mut receiver, &mut sender).await;
    match &answered {
        Ok(()) => sender.finish()?,
        // With no code, both halves drop, which ends them with code 0.
        Err(error) => {
            if let Some(code) = error.code() {
                stop(sender, receiver, code);
            }
        }
    }
    answered
}

/// Ends both halves of a stream with `code`.
fn stop(sender: Sender, receiver: Receiver, code: Code) {
    receiver.stop(code);
    sender.reset(code);
}

/// Gets each of `digests` from the peer of `session` and puts each chunk it gives in
/// `store`. Gives each digest that the peer did not give, with its cause, in the
/// order of `digests`. A refused chunk stops its stream, and the digests still open
/// go again on a new stream to the same peer. A chunk is stored only when its bytes
/// hash to its digest. A drop of the future ends the stream with code 0, a chunk not
/// yet stored is not stored, and a put in flight ends as a dropped [`Store::put`].
///
/// # Errors
///
/// [`Error::Wire`] for a message that does not decode, [`Error::Unexpected`] for a
/// reply that does not answer the oldest open digest, [`Error::Unanswered`] when the
/// peer ends its half with digests open, [`Error::Stream`] when the stream or its
/// session fails, and [`Error::Store`] when the store refuses or fails a put or the
/// pool has no block. The digests that this call did not answer are not in the
/// result.
pub async fn get(
    store: &Store,
    session: &Session,
    digests: &[Digest],
) -> Result<Vec<Miss>, Error> {
    let mut misses = Vec::new();
    let mut open = digests;
    while !open.is_empty() {
        let (mut sender, mut receiver) = session.open(Class::Complete).await?;
        let fetched = fetch(store, &mut sender, &mut receiver, &mut open, &mut misses);
        let fetched = fetched.await;
        if let Some(code) = fetched.as_ref().map_or_else(Error::code, |code| *code) {
            stop(sender, receiver, code);
        }
        fetched?;
    }
    Ok(misses)
}

/// Gets the digests of `open` on one stream, oldest first, and takes each one that
/// the peer answers off `open`. Gives the code of a refused chunk, which the stream
/// stops with, or `None` once `open` is empty and both halves ended.
async fn fetch(
    store: &Store,
    sender: &mut Sender,
    receiver: &mut Receiver,
    open: &mut &[Digest],
    misses: &mut Vec<Miss>,
) -> Result<Option<Code>, Error> {
    header(store, sender).await?;
    let decoder = Requester::new(store.pool.largest());
    let batch = (sender.bytes_max() - get::encoded_len(0)) / size_of::<Digest>();
    while !open.is_empty() {
        let digests = &open[..open.len().min(batch)];
        let mut message = store
            .pool
            .alloc(get::encoded_len(digests.len()))
            .map_err(crate::Error::Pool)?;
        get::encode(digests, &mut message);
        sender.send(message.freeze()).await?;
        for &digest in digests {
            let Some(message) = receiver.recv().await? else {
                return Err(Error::Unanswered);
            };
            let cause = match decoder.decode(&message) {
                Err(wire::blob::Error::TooLarge { len, max }) => {
                    Some(Cause::TooLarge { len, max })
                }
                Ok(Reply::Absent { digest: of }) if of == digest => Some(Cause::Absent),
                Ok(reply @ Reply::Chunk { digest: of, .. }) if of == digest => {
                    drop(message);
                    let chunk = body(store, receiver, reply.body()).await?;
                    match store.put(digest, &chunk).await {
                        Err(crate::Error::Mismatch { found, .. }) => {
                            Some(Cause::Mismatch { found })
                        }
                        put => {
                            put?;
                            None
                        }
                    }
                }
                Ok(reply) => return Err(Error::Unexpected { reply }),
                Err(error) => return Err(error.into()),
            };
            *open = &open[1..];
            if let Some(cause) = cause {
                misses.push(Miss { digest, cause });
                if let Some(code) = cause.code() {
                    return Ok(Some(code));
                }
            }
        }
    }
    sender.finish()?;
    end(receiver, &decoder).await?;
    Ok(None)
}

/// Puts each of `digests` from `store` on the peer of `session`, in order, one at a
/// time. It returns once the peer stored each one, so each is durable on the peer. A
/// drop of the future ends the stream with code 0.
///
/// # Errors
///
/// [`Error::Absent`] when `store` does not hold a digest, [`Error::Refused`] when the
/// peer stops the stream with a code, and [`Error::Unexpected`], [`Error::Unanswered`],
/// [`Error::Stream`], and [`Error::Store`] as for [`get()`].
pub async fn put(
    store: &Store,
    session: &Session,
    digests: &[Digest],
) -> Result<(), Error> {
    if digests.is_empty() {
        return Ok(());
    }
    let (mut sender, mut receiver) = session.open(Class::Complete).await?;
    let pushed = push(store, &mut sender, &mut receiver, digests).await;
    if let Err(error) = &pushed
        && let Some(code) = error.code()
    {
        stop(sender, receiver, code);
    }
    pushed
}

/// Puts each of `digests` on one stream and waits for the end of the peer's half.
async fn push(
    store: &Store,
    sender: &mut Sender,
    receiver: &mut Receiver,
    digests: &[Digest],
) -> Result<(), Error> {
    header(store, sender).await?;
    let decoder = Requester::new(store.pool.largest());
    for &digest in digests {
        let Some(chunk) = store.get(digest).await? else {
            return Err(Error::Absent { digest });
        };
        let refusal = |error| refused(digest, error);
        let len =
            u32::try_from(chunk.len()).expect("invariant: a block holds at most 2 GiB");
        let mut head = store.pool.alloc(Put::LEN).map_err(crate::Error::Pool)?;
        Put { digest, len }.encode(&mut head);
        sender.send(head.freeze()).await.map_err(refusal)?;
        send_chunk(sender, chunk).await.map_err(refusal)?;
        let Some(message) = receiver.recv().await.map_err(refusal)? else {
            return Err(Error::Unanswered);
        };
        match decoder.decode(&message)? {
            Reply::Stored { digest: of } if of == digest => {}
            reply => return Err(Error::Unexpected { reply }),
        }
    }
    sender.finish()?;
    end(receiver, &decoder).await
}

/// The error of a put of `digest` that `error` ended: the peer's refusal when the
/// peer stopped or reset the stream with a code that is not 0.
fn refused(digest: Digest, error: transport::Error) -> Error {
    match error {
        transport::Error::Stopped { code } | transport::Error::Reset { code }
            if code != Code(0) =>
        {
            Error::Refused { digest, code }
        }
        error => Error::Stream(error),
    }
}

/// Sends the blob header.
async fn header(store: &Store, sender: &mut Sender) -> Result<(), Error> {
    let header = wire::header::encode(Protocol::Blob);
    let block = store.pool.copy(&header).map_err(crate::Error::Pool)?;
    sender.send(block).await?;
    Ok(())
}

/// Waits for the end of the peer's half, once no request is open.
async fn end(receiver: &mut Receiver, decoder: &Requester) -> Result<(), Error> {
    match receiver.recv().await? {
        None => Ok(()),
        Some(message) => Err(Error::Unexpected {
            reply: decoder.decode(&message)?,
        }),
    }
}

/// Answers each request of `receiver` on `sender` until the peer's half ends.
async fn answer(
    store: &Store,
    receiver: &mut Receiver,
    sender: &mut Sender,
) -> Result<(), Error> {
    let server = Server::new(store.pool.largest());
    while let Some(message) = receiver.recv().await? {
        match server.decode(&message)? {
            FromRequester::Get(digests) => {
                for digest in digests {
                    give(store, sender, digest).await?;
                }
            }
            FromRequester::Put(put) => {
                let chunk = body(store, receiver, put.body()).await?;
                store.put(put.digest, &chunk).await?;
                // The chunk can fill the pool, and the reply head needs a block.
                drop(chunk);
                let stored = Reply::Stored { digest: put.digest };
                reply(store, sender, stored).await?;
            }
        }
    }
    Ok(())
}

/// Sends the chunk of `digest`, or that `store` does not hold it.
async fn give(store: &Store, sender: &mut Sender, digest: Digest) -> Result<(), Error> {
    let Some(chunk) = store.get(digest).await? else {
        return reply(store, sender, Reply::Absent { digest }).await;
    };
    let len =
        u32::try_from(chunk.len()).expect("invariant: a block holds at most 2 GiB");
    reply(store, sender, Reply::Chunk { digest, len }).await?;
    send_chunk(sender, chunk).await?;
    Ok(())
}

/// Sends `chunk` as messages of at most `Sender::bytes_max`, each a range of its
/// block, so no byte is copied.
async fn send_chunk(sender: &mut Sender, chunk: Block) -> Result<(), transport::Error> {
    let step = sender.bytes_max();
    let mut at = 0;
    while at < chunk.len() {
        let end = chunk.len().min(at.saturating_add(step));
        let part = Part {
            range: at..end,
            zeros: 0,
        };
        sender.send_parts(chunk.clone(), &[part]).await?;
        at = end;
    }
    Ok(())
}

/// Reads `body` into one block.
async fn body(
    store: &Store,
    receiver: &mut Receiver,
    mut body: Body,
) -> Result<Block, Error> {
    let mut chunk = store
        .pool
        .alloc(body.remain())
        .map_err(crate::Error::Pool)?;
    let mut at = 0;
    while body.remain() > 0 {
        let Some(message) = receiver.recv().await? else {
            break;
        };
        let bytes = body.take(&message)?;
        let end = at + bytes.len();
        chunk[at..end].copy_from_slice(bytes);
        at = end;
    }
    body.end()?;
    Ok(chunk.freeze())
}

async fn reply(store: &Store, sender: &mut Sender, reply: Reply) -> Result<(), Error> {
    let mut head = store
        .pool
        .alloc(reply.encoded_len())
        .map_err(crate::Error::Pool)?;
    reply.encode(&mut head);
    sender.send(head.freeze()).await?;
    Ok(())
}

/// A digest that a get did not give, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Miss {
    /// The digest of the get.
    pub digest: Digest,
    /// Why the peer did not give it.
    pub cause: Cause,
}

/// Why the peer did not give a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The peer does not hold it.
    Absent,
    /// The peer's chunk is longer than the largest block of the store.
    TooLarge {
        /// The bytes of the chunk, as the peer claimed.
        len: u32,
        /// The largest block of the store.
        max: usize,
    },
    /// The peer's bytes hash to `found`.
    Mismatch {
        /// The digest of the bytes.
        found: Digest,
    },
}

impl Cause {
    /// The code that the stream of a refused chunk stops with, or `None` when the
    /// stream goes on.
    fn code(self) -> Option<Code> {
        match self {
            Self::Absent => None,
            Self::TooLarge { .. } => Some(Code(TOO_LARGE)),
            Self::Mismatch { .. } => Some(Code(MISMATCH)),
        }
    }
}

/// Why a blob stream ended with an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A message of the peer that does not decode. The stream stops with
    /// [`wire::blob::Error::code`].
    Wire(wire::blob::Error),
    /// The peer opened the stream one way. The stream stops with code `MALFORMED`.
    OneWay,
    /// The store refused a put, or failed. The stream stops with `MISMATCH` for
    /// [`crate::Error::Mismatch`], with `FULL` for [`crate::Error::Floor`] and a full
    /// disk, and with code 0 for each other error.
    Store(crate::Error),
    /// The stream or its session failed. No code.
    Stream(transport::Error),
    /// The store of a put does not hold the digest. Nothing of it was sent.
    Absent {
        /// The digest of the put.
        digest: Digest,
    },
    /// The peer stopped the stream with `code` in the put of `digest`: `FULL`,
    /// `MISMATCH`, `TOO_LARGE`, or another code that is not 0.
    Refused {
        /// The digest of the put.
        digest: Digest,
        /// The code of the stop.
        code: Code,
    },
    /// A reply that does not answer the oldest open request. The stream stops with
    /// code `MALFORMED`.
    Unexpected {
        /// The reply.
        reply: Reply,
    },
    /// The peer ended its half with requests open. The stream stops with code
    /// `MALFORMED`.
    Unanswered,
}

impl Error {
    /// The code that the stream stops with, or `None` when it drops with code 0.
    fn code(&self) -> Option<Code> {
        match self {
            Self::Wire(error) => Some(Code(error.code())),
            Self::OneWay | Self::Unexpected { .. } | Self::Unanswered => {
                Some(Code(MALFORMED))
            }
            Self::Store(crate::Error::Mismatch { .. }) => Some(Code(MISMATCH)),
            Self::Store(
                crate::Error::Floor { .. }
                | crate::Error::Files(files::Error::Full { .. }),
            ) => Some(Code(FULL)),
            Self::Store(_)
            | Self::Stream(_)
            | Self::Absent { .. }
            | Self::Refused { .. } => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wire(error) => write!(f, "a blob message is not valid: {error}"),
            Self::OneWay => f.write_str("a blob stream needs a two-way stream"),
            Self::Store(error) => error.fmt(f),
            Self::Stream(error) => write!(f, "the blob stream failed: {error}"),
            Self::Absent { digest } => write!(f, "the store does not hold {digest}"),
            Self::Refused { digest, code } => {
                write!(
                    f,
                    "the peer refused the put of {digest} with code {}",
                    code.0
                )
            }
            Self::Unexpected { reply } => {
                write!(f, "the reply {reply:?} does not answer the oldest request")
            }
            Self::Unanswered => {
                f.write_str("the peer ended its half with requests open")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<wire::blob::Error> for Error {
    fn from(error: wire::blob::Error) -> Self {
        Self::Wire(error)
    }
}

impl From<crate::Error> for Error {
    fn from(error: crate::Error) -> Self {
        Self::Store(error)
    }
}

impl From<transport::Error> for Error {
    fn from(error: transport::Error) -> Self {
        Self::Stream(error)
    }
}

#[cfg(test)]
mod tests;
