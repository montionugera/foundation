//! The frames of a program's write stream after its places, and the replies of the
//! node. The messages before them are in [`client::write`](super::client::write).
//!
//! Fields are little-endian.
//!
//! - [`Head`]: kind 7, the time (`u8`, live 0, backfill 1, node 2), the form (`u8`, raw
//!   0, encoded 1), the samples of each series (`u32`), and the count of series
//!   (`u32`).
//! - [`Reply`]: taken 1 or lost 2, then the frame (`u64`), the seq (`u64`), and the
//!   count (`u32`); refused 3, then the frame (`u64`) and the code (`u8`, waiting 1,
//!   reserved 2, expired 3, order 4, codec 5, full 6, large 7); stored 4, then the
//!   count of frames that it covers (`u64`).

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
    /// What the home did with one frame.
    Outcome {
        /// The frame that it answers, from 1: the count of frames of the stream
        /// through it.
        frame: u64,
        /// What the home did with the frame.
        outcome: Outcome,
    },
    /// Each frame of the stream among the first `frames` that the home took is on
    /// disk. `frames` counts each frame that got an [`Outcome`], in their order, and
    /// never goes back.
    Stored {
        /// The count of frames that it covers.
        frames: u64,
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
    pub const LEN_MAX: usize = 21;
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
            Self::Outcome {
                outcome: Outcome::Taken { .. } | Outcome::Lost { .. },
                ..
            } => Outcome::LEN_MAX,
            Self::Outcome {
                outcome: Outcome::Refused(_),
                ..
            } => 10,
            Self::Stored { .. } => 9,
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
            Self::Outcome {
                frame,
                outcome: Outcome::Taken { range },
            } => {
                out.put(&[TAKEN]);
                out.put(&frame.to_le_bytes());
                put_range(&mut out, range);
            }
            Self::Outcome {
                frame,
                outcome: Outcome::Lost { range },
            } => {
                out.put(&[LOST]);
                out.put(&frame.to_le_bytes());
                put_range(&mut out, range);
            }
            Self::Outcome {
                frame,
                outcome: Outcome::Refused(refusal),
            } => {
                out.put(&[REFUSED]);
                out.put(&frame.to_le_bytes());
                out.put(&[refusal.code()]);
            }
            Self::Stored { frames } => {
                out.put(&[STORED]);
                out.put(&frames.to_le_bytes());
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
    /// [`Error::Refusal`] for a refusal code that no refusal has.
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = Fields::new(message);
        let kind = fields.kind().ok_or(Error::Empty)?;
        if kind == STORED {
            let frames = u64::from_le_bytes(*fields.take()?);
            fields.end()?;
            return Ok(Self::Stored { frames });
        }
        if !matches!(kind, TAKEN | LOST | REFUSED) {
            return Err(Error::Kind { kind });
        }
        let frame = u64::from_le_bytes(*fields.take()?);
        let outcome = match kind {
            TAKEN => Outcome::Taken {
                range: take_range(&mut fields)?,
            },
            LOST => Outcome::Lost {
                range: take_range(&mut fields)?,
            },
            _ => {
                let [code] = *fields.take()?;
                fields.end()?;
                Outcome::Refused(
                    Refusal::ALL
                        .into_iter()
                        .find(|refusal| refusal.code() == code)
                        .ok_or(Error::Refusal { code })?,
                )
            }
        };
        Ok(Self::Outcome { frame, outcome })
    }
}

/// The order of the replies of a write stream at the program: an outcome answers the
/// oldest frame sent whole with no outcome, and names it, and [`Reply::Stored`] never
/// passes the outcomes given nor goes back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Replies {
    sent: u64,
    answered: u64,
    /// The count of frames up to the last that the home took.
    taken: u64,
    stored: u64,
}

impl Replies {
    /// The replies of a stream that sent no frame.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts a frame that the program sent whole.
    pub fn sent(&mut self) {
        self.sent = self.sent.strict_add(1);
    }

    /// Decodes `message`, the next reply, and checks its order.
    ///
    /// # Errors
    ///
    /// As [`Reply::decode`], [`Error::Unasked`] for an outcome of frame 0 or of a frame
    /// past those sent, [`Error::Order`] for an outcome of another frame than the
    /// oldest sent with no outcome, and [`Error::Stored`] for a [`Reply::Stored`] past
    /// the outcomes given or below the last [`Reply::Stored`]. A reply that it refuses
    /// changes no count.
    pub fn decode(&mut self, message: &[u8]) -> Result<Reply, Error> {
        let reply = Reply::decode(message)?;
        match reply {
            Reply::Outcome { frame, .. } if frame == 0 || frame > self.sent => {
                return Err(Error::Unasked {
                    frame,
                    sent: self.sent,
                });
            }
            Reply::Outcome { frame, outcome } => {
                let next = self.answered.strict_add(1);
                if frame != next {
                    return Err(Error::Order { frame, next });
                }
                self.answered = next;
                if let Outcome::Taken { .. } = outcome {
                    self.taken = next;
                }
            }
            Reply::Stored { frames } if frames < self.stored => {
                let bound = self.stored;
                return Err(Error::Stored { frames, bound });
            }
            Reply::Stored { frames } if frames > self.answered => {
                let bound = self.answered;
                return Err(Error::Stored { frames, bound });
            }
            Reply::Stored { frames } => self.stored = frames,
        }
        Ok(reply)
    }

    /// Whether each frame sent got its outcome.
    #[must_use]
    pub fn answered(&self) -> bool {
        self.answered == self.sent
    }

    /// Whether each frame sent got its outcome, and the last [`Reply::Stored`] covers
    /// each frame that the home took.
    #[must_use]
    pub fn stored(&self) -> bool {
        self.answered() && self.stored >= self.taken
    }
}

fn put_range(out: &mut Writer<'_>, range: frame::Range) {
    assert!(
        range.count > 0 && range.end().is_some(),
        "a reply's range of {} samples from seq {} holds none or ends past the highest \
         seq",
        range.count,
        range.seq
    );
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
