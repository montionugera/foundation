//! The messages of a write stream.

use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

use types::authority::Authority;
use types::ed25519::PublicKey;
use types::frame;
use types::name::Name;
use types::node;
use types::time::Span;

use super::{Body, Error, OPEN_BYTES_MAX, WRITE, fields, text};
use crate::common::{Fields, Writer};

/// The most addresses of an [`Away`]: 32, the most of a card.
pub const ADDRESSES_MAX: usize = 32;

const UDP: u8 = 0;
const TCP: u8 = 1;
const RELAY: u8 = 2;

const V4: u8 = 4;
const V6: u8 = 6;

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

/// The reply that finishes a write stream at a node that is not the home of its
/// index: where to dial the home. The node sends it in place of the places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Away {
    /// The home.
    pub node: node::Key,
    /// The key that `transport` pins for the home.
    pub public_key: PublicKey,
    /// Where to dial the home, at most [`ADDRESSES_MAX`].
    pub addresses: Vec<Address>,
}

impl Away {
    /// The bytes of the encoded away.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.addresses
            .iter()
            .map(Address::encoded_len)
            .fold(50, usize::saturating_add)
    }

    /// Writes the away into `out`.
    ///
    /// # Panics
    ///
    /// When `addresses` holds more than [`ADDRESSES_MAX`], an IPv6 address has flow
    /// info or a scope, or `out` is not [`Away::encoded_len`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        let count = self.addresses.len();
        assert!(
            count <= ADDRESSES_MAX,
            "{count} addresses are over the cap of {ADDRESSES_MAX}"
        );
        let mut out = Writer::new(out, self.encoded_len());
        out.put(&[WRITE]);
        out.put(&self.node.as_u128().to_le_bytes());
        out.put(&self.public_key.to_bytes());
        out.put(&[u8::try_from(count).expect("invariant: the count is at most 32")]);
        for address in &self.addresses {
            address.encode(&mut out);
        }
    }

    /// Decodes an away.
    ///
    /// # Errors
    ///
    /// In the order of the bytes: [`Error::Empty`], [`Error::Kind`] for a message that
    /// is not kind 7, [`Error::Length`] when it ends inside a field or holds bytes
    /// after its last address, [`Error::SmallOrder`] for the key,
    /// [`Error::Addresses`] when the count is over [`ADDRESSES_MAX`], and, for an
    /// address, [`Error::Address`] for its kind, [`Error::SmallOrder`] for the key of
    /// a relay, and [`Error::Family`].
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = fields(message, WRITE)?;
        let node = node::Key::from_u128(u128::from_le_bytes(*fields.take()?));
        let public_key = key(*fields.take()?)?;
        let [count] = *fields.take()?;
        let count = usize::from(count);
        if count > ADDRESSES_MAX {
            return Err(Error::Addresses {
                count,
                cap: ADDRESSES_MAX,
            });
        }
        let addresses = (0..count)
            .map(|_| Address::decode(&mut fields))
            .collect::<Result<_, _>>()?;
        fields.end()?;
        Ok(Self {
            node,
            public_key,
            addresses,
        })
    }
}

/// Where to dial a home, as `transport` dials it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Address {
    /// A UDP socket.
    Udp(SocketAddr),
    /// A TCP socket.
    Tcp(SocketAddr),
    /// A relay node that forwards sessions to the home.
    Relay {
        /// The relay's key.
        node: PublicKey,
        /// The relay's socket.
        at: SocketAddr,
    },
}

impl Address {
    fn encoded_len(&self) -> usize {
        let (Self::Udp(at) | Self::Tcp(at) | Self::Relay { at, .. }) = self;
        // The kind, the key of a relay, the family, the IP, and the port.
        match (self, at.is_ipv4()) {
            (Self::Relay { .. }, true) => 40,
            (Self::Relay { .. }, false) => 52,
            (_, true) => 8,
            (_, false) => 20,
        }
    }

    fn encode(&self, out: &mut Writer<'_>) {
        let at = match *self {
            Self::Udp(at) => {
                out.put(&[UDP]);
                at
            }
            Self::Tcp(at) => {
                out.put(&[TCP]);
                at
            }
            Self::Relay { node, at } => {
                out.put(&[RELAY]);
                out.put(&node.to_bytes());
                at
            }
        };
        match at {
            SocketAddr::V4(at) => {
                out.put(&[V4]);
                out.put(&at.ip().octets());
            }
            SocketAddr::V6(at) => {
                assert!(
                    at.flowinfo() == 0 && at.scope_id() == 0,
                    "the address {at} has flow info or a scope"
                );
                out.put(&[V6]);
                out.put(&at.ip().octets());
            }
        }
        out.put(&at.port().to_le_bytes());
    }

    fn decode(fields: &mut Fields<'_>) -> Result<Self, Error> {
        let [kind] = *fields.take()?;
        let relay = match kind {
            UDP | TCP => None,
            RELAY => Some(key(*fields.take()?)?),
            kind => return Err(Error::Address { kind }),
        };
        let at = match *fields.take()? {
            [V4] => {
                let ip = <[u8; 4]>::into(*fields.take()?);
                SocketAddr::V4(SocketAddrV4::new(
                    ip,
                    u16::from_le_bytes(*fields.take()?),
                ))
            }
            [V6] => {
                let ip = <[u8; 16]>::into(*fields.take()?);
                let port = u16::from_le_bytes(*fields.take()?);
                SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0))
            }
            [family] => return Err(Error::Family { family }),
        };
        Ok(match relay {
            Some(node) => Self::Relay { node, at },
            None if kind == UDP => Self::Udp(at),
            None => Self::Tcp(at),
        })
    }
}

fn key(bytes: [u8; 32]) -> Result<PublicKey, Error> {
    PublicKey::new(bytes).map_err(|_small| Error::SmallOrder)
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
    /// [`Error::Length`], and [`Error::Refused`] for a refusal code that no refusal
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
                    .ok_or(Error::Refused { code });
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
