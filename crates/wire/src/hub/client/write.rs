//! The messages of a write stream.

use std::sync::Arc;

use types::authority::Authority;
use types::frame::key_set::KeySet;
use types::frame::{self, Form, Path};
use types::name::Name;
use types::time::Span;

use super::run::Run;
use super::{Body, Error, OPEN_BYTES_MAX, Place, Places, WRITE, fields, text};
use crate::common::{Fields, Writer};

const LIVE: u8 = 0;
const BACKFILL: u8 = 1;
const NODE: u8 = 2;

const RAW: u8 = 0;
const ENCODED: u8 = 1;

const NO_LEASE: u8 = 0;
const LEASE: u8 = 1;

const TAKEN: u8 = 1;
const LOST: u8 = 2;
const REFUSED: u8 = 3;
const STORED: u8 = 4;

/// The first message of a write stream. The open follows as a body of `length` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    /// The bytes of the open, at most [`OPEN_BYTES_MAX`].
    pub length: u64,
    /// The signature of the hello's key over the signed bytes of the open, of the
    /// kind of a write: the connection and the open.
    pub signature: [u8; 64],
}

impl Write {
    /// The bytes of an encoded write.
    pub const LEN: usize = 73;

    /// Writes the write into `out`.
    ///
    /// # Panics
    ///
    /// When `length` is over [`OPEN_BYTES_MAX`], or `out` is not [`Write::LEN`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        self.check();
        let mut out = Writer::new(out, Self::LEN);
        out.put(&[WRITE]);
        out.put(&self.length.to_le_bytes());
        out.put(&self.signature);
    }

    /// Decodes a write.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`], [`Error::Kind`] for a message that is not kind 7,
    /// [`Error::Length`], and [`Error::Oversize`] for an open over
    /// [`OPEN_BYTES_MAX`].
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = fields(message, WRITE)?;
        let length = u64::from_le_bytes(*fields.take()?);
        let signature = *fields.take()?;
        fields.end()?;
        if length > OPEN_BYTES_MAX {
            return Err(Error::Oversize {
                length,
                cap: OPEN_BYTES_MAX,
            });
        }
        Ok(Self { length, signature })
    }

    /// The open that follows this write.
    ///
    /// # Panics
    ///
    /// When `length` is over [`OPEN_BYTES_MAX`].
    #[must_use]
    pub fn body(&self) -> Body {
        self.check();
        Body::new(self.length)
    }

    fn check(&self) {
        assert!(
            self.length <= OPEN_BYTES_MAX,
            "an open of {} bytes is over the cap of {OPEN_BYTES_MAX}",
            self.length
        );
    }
}

/// The open of a write: the body after a [`Write`], and what its signature covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    /// The writer's authority.
    pub authority: Authority,
    /// How long the writer may go without a write and keep control, or `None` for no
    /// limit.
    pub lease: Option<Span>,
    /// The channels the writer writes, at least one, all on one index. The cap of the
    /// open keeps them under [`PLACES_MAX`](super::PLACES_MAX).
    pub channels: Vec<Name>,
}

impl Open {
    /// The bytes of the encoded open.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        let lease = if self.lease.is_some() { 10 } else { 2 };
        self.channels
            .iter()
            .map(|name| name.as_str().len().saturating_add(1))
            .fold(lease, usize::saturating_add)
    }

    /// Writes the open into `out`.
    ///
    /// # Panics
    ///
    /// When `channels` is empty, or `out` is not [`Open::encoded_len`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        assert!(
            !self.channels.is_empty(),
            "the open of a write names no channel"
        );
        let mut out = Writer::new(out, self.encoded_len());
        out.put(&[self.authority.0]);
        match self.lease {
            None => out.put(&[NO_LEASE]),
            Some(span) => {
                out.put(&[LEASE]);
                out.put(&span.nanos().to_le_bytes());
            }
        }
        for name in &self.channels {
            out.put(&[u8::try_from(name.as_str().len())
                .expect("invariant: a name is at most 255 bytes")]);
            out.put(name.as_str().as_bytes());
        }
    }

    /// Decodes the open in `bytes`, all the bytes of its body.
    ///
    /// # Errors
    ///
    /// In the order of the bytes: [`Error::Empty`] when `bytes` is empty,
    /// [`Error::Length`] when the open ends inside a field, [`Error::Lease`] when the
    /// lease byte is neither 0 nor 1, and [`Error::Channel`] when a name does not
    /// read. Then [`Error::Unnamed`] when it names no channel.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut fields = Fields::new(bytes);
        let authority = fields.kind().ok_or(Error::Empty)?;
        let lease = match *fields.take()? {
            [NO_LEASE] => None,
            [LEASE] => Some(Span::from_nanos(i64::from_le_bytes(*fields.take()?))),
            [byte] => return Err(Error::Lease { byte }),
        };
        let mut channels = Vec::new();
        while !fields.is_empty() {
            let [len] = *fields.take()?;
            let name = text::read(fields.take_slice(usize::from(len))?)
                .map_err(Error::Channel)?;
            channels.push(name);
        }
        if channels.is_empty() {
            return Err(Error::Unnamed);
        }
        Ok(Self {
            authority: Authority(authority),
            lease,
            channels,
        })
    }
}

/// A decoder of the start of a write stream at the program: the places and their run.
#[derive(Debug, Default)]
pub struct Opening(Next);

/// The message the stream expects next.
#[derive(Debug, Default)]
enum Next {
    #[default]
    Places,
    Run(Run),
    Given,
}

/// The start of a write stream, at the program.
#[derive(Debug)]
pub struct Start {
    /// The places, in place order.
    pub places: Vec<Place>,
    /// The key set of each frame of the write, from [`KeySet::ordered`]: entry `n` is
    /// place `n`.
    pub set: Arc<KeySet>,
}

impl Opening {
    /// Decodes the next message from the node, up to the last place, or `None` when
    /// the node finished the stream. Gives the start with the last place, else
    /// `None`.
    ///
    /// # Errors
    ///
    /// [`Error::Finished`] when `message` is `None`. The first message gives the errors
    /// of [`Places::decode`]. A message of the run gives the errors of a run of places,
    /// in order: the error of its bytes ([`Error::Empty`], [`Error::Length`],
    /// [`Error::Sample`], or [`Error::Name`]), [`Error::Run`], and
    /// [`Error::Unstamped`], then [`Error::Repeated`] for the last message.
    ///
    /// The stream is then not valid ([`MALFORMED`](crate::header::MALFORMED)), and
    /// the caller stops it. A refused message leaves the decoder as it was.
    ///
    /// # Panics
    ///
    /// When it gave the start before.
    pub fn decode(&mut self, message: Option<&[u8]>) -> Result<Option<Start>, Error> {
        match (&mut self.0, message) {
            (Next::Given, _) => {
                panic!("a write stream is decoded after the decoder gave its start")
            }
            (_, None) => Err(Error::Finished),
            (Next::Places, Some(message)) => {
                self.0 = Next::Run(Run::new(Places::decode(message)?));
                Ok(None)
            }
            (Next::Run(run), Some(message)) => {
                let Some((places, set)) = run.decode(message)? else {
                    return Ok(None);
                };
                self.0 = Next::Given;
                Ok(Some(Start {
                    places,
                    set: Arc::new(set),
                }))
            }
        }
    }
}

/// The head of one frame that a program writes, after the open. The run of its
/// [`ends`](crate::hub::ends) follows, then its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    /// Where the frame's stamps come from, and its path.
    pub time: Time,
    /// How the frame's series hold their samples.
    pub form: Form,
    /// The samples of each series, at least 1.
    pub count: u32,
    /// The count of the frame's series, at least 1 and at most the count of places.
    pub series: u32,
}

/// Where the stamps of a frame come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Time {
    /// The frame's index series holds them. The frame goes to this path.
    Frame(Path),
    /// The node writes a stamp over the frame's one sample of the index, which holds
    /// any 8 bytes. The frame goes to the live path, and holds one sample of each
    /// series, in the raw form.
    Node,
}

impl Head {
    /// The bytes of an encoded head.
    pub const LEN: usize = 11;

    /// Writes the head into `out`.
    ///
    /// # Panics
    ///
    /// When `count` or `series` is 0, when a head of [`Time::Node`] is encoded or has
    /// a count other than 1, or when `out` is not [`Head::LEN`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        assert!(
            self.count > 0 && self.series > 0,
            "a head has {} samples of {} series, not at least 1 of 1",
            self.count,
            self.series
        );
        assert!(
            self.time != Time::Node || (self.form == Form::Raw && self.count == 1),
            "a head that the node stamps has {} samples in the {:?} form",
            self.count,
            self.form
        );
        let time = match self.time {
            Time::Frame(Path::Live) => LIVE,
            Time::Frame(Path::Backfill) => BACKFILL,
            Time::Node => NODE,
        };
        let form = match self.form {
            Form::Raw => RAW,
            Form::Encoded => ENCODED,
        };
        let mut out = Writer::new(out, Self::LEN);
        out.put(&[WRITE, time, form]);
        out.put(&self.count.to_le_bytes());
        out.put(&self.series.to_le_bytes());
    }

    /// Decodes a head.
    ///
    /// # Errors
    ///
    /// In order: [`Error::Empty`], [`Error::Kind`] for a message that is not kind 7,
    /// [`Error::Length`], [`Error::Time`], [`Error::Form`], [`Error::Count`] and
    /// [`Error::Series`] for 0, and [`Error::Untimed`] for a head of [`Time::Node`]
    /// that is encoded or has a count other than 1.
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = fields(message, WRITE)?;
        let [time, form] = *fields.take()?;
        let count = u32::from_le_bytes(*fields.take()?);
        let series = u32::from_le_bytes(*fields.take()?);
        fields.end()?;
        let time = match time {
            LIVE => Time::Frame(Path::Live),
            BACKFILL => Time::Frame(Path::Backfill),
            NODE => Time::Node,
            time => return Err(Error::Time { time }),
        };
        let form = match form {
            RAW => Form::Raw,
            ENCODED => Form::Encoded,
            form => return Err(Error::Form { form }),
        };
        if count == 0 {
            return Err(Error::Count);
        }
        if series == 0 {
            return Err(Error::Series);
        }
        if time == Time::Node && (form != Form::Raw || count != 1) {
            return Err(Error::Untimed { form, count });
        }
        Ok(Self {
            time,
            form,
            count,
            series,
        })
    }
}

/// A reply of the node on a write stream after the places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// What the home did with the next frame.
    Outcome(Outcome),
    /// Each seq of the index up to `through` that a frame took is on disk.
    Stored {
        /// The last seq on disk.
        through: u64,
    },
}

/// What the home did with one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The home applied the frame at `range`. It is not on disk yet.
    Taken {
        /// The seqs of the frame's samples.
        range: frame::Range,
    },
    /// A live frame found no room: `range` is a gap in the log.
    Lost {
        /// The seqs of the frame's samples.
        range: frame::Range,
    },
    /// The home refused the frame, and no seq moved.
    Refused(Refusal),
}

/// Why the home refused a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Another writer holds control.
    Waiting,
    /// Control is held for a writer from before a restart.
    Reserved,
    /// The writer's control lease ran out. Open the writer again.
    Expired,
    /// A stamp is not after the stamp before it.
    Order,
    /// A series is not a series of its channel's type.
    Codec,
    /// A backfill frame found no room in the log, or the frame found no block of the
    /// node's pool. Write it again later.
    Full,
    /// The frame is too large for one write. Split it.
    Large,
}

impl Refusal {
    const ALL: [Self; 7] = [
        Self::Waiting,
        Self::Reserved,
        Self::Expired,
        Self::Order,
        Self::Codec,
        Self::Full,
        Self::Large,
    ];

    const fn code(self) -> u8 {
        match self {
            Self::Waiting => 1,
            Self::Reserved => 2,
            Self::Expired => 3,
            Self::Order => 4,
            Self::Codec => 5,
            Self::Full => 6,
            Self::Large => 7,
        }
    }
}

impl Reply {
    /// The bytes of the encoded reply.
    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        match self {
            Self::Outcome(Outcome::Taken { .. } | Outcome::Lost { .. }) => 13,
            Self::Outcome(Outcome::Refused(_)) => 2,
            Self::Stored { .. } => 9,
        }
    }

    /// Writes the reply into `out`.
    ///
    /// # Panics
    ///
    /// When `out` is not [`Reply::encoded_len`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        let mut out = Writer::new(out, self.encoded_len());
        match *self {
            Self::Outcome(Outcome::Taken { range }) => {
                put_range(&mut out, TAKEN, range);
            }
            Self::Outcome(Outcome::Lost { range }) => put_range(&mut out, LOST, range),
            Self::Outcome(Outcome::Refused(refusal)) => {
                out.put(&[REFUSED, refusal.code()]);
            }
            Self::Stored { through } => {
                out.put(&[STORED]);
                out.put(&through.to_le_bytes());
            }
        }
    }

    /// Decodes a reply.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`], [`Error::Kind`] for a kind that no reply has,
    /// [`Error::Length`], and [`Error::Refusal`] for a refusal code that no refusal
    /// has.
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = Fields::new(message);
        let reply = match fields.kind().ok_or(Error::Empty)? {
            TAKEN => Self::Outcome(Outcome::Taken {
                range: take_range(&mut fields)?,
            }),
            LOST => Self::Outcome(Outcome::Lost {
                range: take_range(&mut fields)?,
            }),
            REFUSED => {
                let [code] = *fields.take()?;
                fields.end()?;
                return Refusal::ALL
                    .into_iter()
                    .find(|refusal| refusal.code() == code)
                    .map(|refusal| Self::Outcome(Outcome::Refused(refusal)))
                    .ok_or(Error::Refusal { code });
            }
            STORED => Self::Stored {
                through: u64::from_le_bytes(*fields.take()?),
            },
            kind => return Err(Error::Kind { kind }),
        };
        fields.end()?;
        Ok(reply)
    }
}

fn put_range(out: &mut Writer<'_>, kind: u8, range: frame::Range) {
    out.put(&[kind]);
    out.put(&range.seq.to_le_bytes());
    out.put(&range.count.to_le_bytes());
}

fn take_range(fields: &mut Fields<'_>) -> Result<frame::Range, Error> {
    Ok(frame::Range {
        seq: u64::from_le_bytes(*fields.take()?),
        count: u32::from_le_bytes(*fields.take()?),
    })
}

#[cfg(test)]
mod tests;
