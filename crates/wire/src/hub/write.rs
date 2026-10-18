//! The frames of a program's write stream after its places, and the replies of the
//! node. The messages before them are in [`client::write`](super::client::write).
//!
//! Fields are little-endian.
//!
//! - [`Head`]: kind 7, the time (`u8`, live 0, backfill 1, node 2), the form (`u8`, raw
//!   0, encoded 1), the samples of each series (`u32`), and the count of series
//!   (`u32`).
//! - [`Reply`]: taken 1 or lost 2, then the seq (`u64`) and the count (`u32`); refused
//!   3, then the code (`u8`, waiting 1, reserved 2, expired 3, order 4, codec 5, full
//!   6, large 7); stored 4, then the path (`u8`, live 0, backfill 1) and the last seq
//!   of the path on disk (`u64`).

use types::frame::{self, Form, Path};

use super::{Error, Fields, Writer};

mod frames;

pub use frames::{Due, Frames, Step};

const HEAD: u8 = 7;

const LIVE: u8 = 0;
const BACKFILL: u8 = 1;
const NODE: u8 = 2;

const RAW: u8 = 0;
const ENCODED: u8 = 1;

const TAKEN: u8 = 1;
const LOST: u8 = 2;
const REFUSED: u8 = 3;
const STORED: u8 = 4;

/// The head of one frame that a program writes, after the places. The run of its
/// [`ends`](super::ends) follows, then its body.
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
    /// When `count` or `series` is 0, when a head of [`Time::Node`] has the encoded
    /// form or a count other than 1, or when `out` is not [`Head::LEN`] bytes.
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
        out.put(&[HEAD, time, form]);
        out.put(&self.count.to_le_bytes());
        out.put(&self.series.to_le_bytes());
    }

    /// Decodes a head, and refuses each head that [`Head::encode`] panics on.
    fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = Fields::new(message);
        match fields.kind().ok_or(Error::Empty)? {
            HEAD => {}
            kind => return Err(Error::Kind { kind }),
        }
        let [time, form] = *fields.take()?;
        let count = u32::from_le_bytes(*fields.take()?);
        let series = u32::from_le_bytes(*fields.take()?);
        fields.end()?;
        let time = match time {
            LIVE => Time::Frame(Path::Live),
            BACKFILL => Time::Frame(Path::Backfill),
            NODE => Time::Node,
            byte => return Err(Error::Path { byte }),
        };
        let form = match form {
            RAW => Form::Raw,
            ENCODED => Form::Encoded,
            byte => return Err(Error::Form { byte }),
        };
        if count == 0 {
            return Err(Error::Samples);
        }
        if series == 0 {
            return Err(Error::Series);
        }
        if time == Time::Node && (form != Form::Raw || count != 1) {
            return Err(Error::Stamp { count, form });
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
    /// Each seq of `path` up to `through` that a frame took is on disk.
    Stored {
        /// The path of the seqs. Each path of an index has its own seqs.
        path: Path,
        /// The last seq of `path` on disk.
        through: u64,
    },
}

/// What the home did with one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The home applied the frame at `range`. It is not on disk yet.
    Taken {
        /// The seqs of the frame's samples: at least 1, and they end at most at
        /// `u64::MAX`.
        range: frame::Range,
    },
    /// A live frame found no room: `range` is a gap in the log.
    Lost {
        /// The seqs of the frame's samples: at least 1, and they end at most at
        /// `u64::MAX`.
        range: frame::Range,
    },
    /// The home refused the frame, and no seq moved.
    Refused(Refusal),
}

impl Outcome {
    /// The most bytes that a [`Reply::Outcome`] encodes to.
    pub const LEN_MAX: usize = 13;
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
            Self::Outcome(Outcome::Taken { .. } | Outcome::Lost { .. }) => {
                Outcome::LEN_MAX
            }
            Self::Outcome(Outcome::Refused(_)) => 2,
            Self::Stored { .. } => 10,
        }
    }

    /// Writes the reply into `out`.
    ///
    /// # Panics
    ///
    /// When a range holds no sample or ends past `u64::MAX`, or `out` is not
    /// [`Reply::encoded_len`] bytes.
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
            Self::Stored { path, through } => {
                out.put(&[STORED, super::path_byte(path)]);
                out.put(&through.to_le_bytes());
            }
        }
    }

    /// Decodes a reply.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`], [`Error::Kind`] for a kind that no reply has,
    /// [`Error::Length`], [`Error::Count`] for a range that holds no sample,
    /// [`Error::Range`] for a range that ends past `u64::MAX`, and
    /// [`Error::Refusal`] for a refusal code that no refusal has, and [`Error::Path`]
    /// for a path byte that no path has.
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
            STORED => {
                let [path] = *fields.take()?;
                let through = u64::from_le_bytes(*fields.take()?);
                fields.end()?;
                let path = super::path_of(path)?;
                Self::Stored { path, through }
            }
            kind => return Err(Error::Kind { kind }),
        };
        Ok(reply)
    }
}

fn put_range(out: &mut Writer<'_>, kind: u8, range: frame::Range) {
    assert!(
        range.count > 0 && range.end().is_some(),
        "a reply's range of {} samples from seq {} holds none or ends past the highest \
         seq",
        range.count,
        range.seq
    );
    out.put(&[kind]);
    out.put(&range.seq.to_le_bytes());
    out.put(&range.count.to_le_bytes());
}

/// The range after the kind, the last field of its reply.
fn take_range(fields: &mut Fields<'_>) -> Result<frame::Range, Error> {
    let seq = u64::from_le_bytes(*fields.take()?);
    let count = u32::from_le_bytes(*fields.take()?);
    fields.end()?;
    let range = frame::Range { seq, count };
    if count == 0 {
        return Err(Error::Count { seq });
    }
    if range.end().is_none() {
        return Err(Error::Range { seq, count });
    }
    Ok(range)
}

#[cfg(test)]
mod tests;
