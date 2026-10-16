//! The reads of one connector: the requests that read them, and the value of each
//! read in their replies. A write of `command` takes its checks and its codec from
//! [`Read`].

use std::fmt;

use types::sample::{Scalar, Type, Value};

use crate::pdu::{Exception, Reply, Request, Table};

/// Where the value of a channel is in a device: a table, the address of its first
/// item, and its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    table: Table,
    address: u16,
    scalar: Scalar,
    count: u16,
}

impl Read {
    /// A read of a value of type `sample` at `address` in `table`. A value of more
    /// than one register takes the registers from `address` up.
    ///
    /// # Errors
    ///
    /// - [`Error::Type`] for a type that `table` does not hold. Coils and discrete
    ///   inputs hold `bool`. Registers hold `u16` and `i16` (1 register), `u32`,
    ///   `i32`, and `f32` (2), and `u64`, `i64`, and `f64` (4).
    /// - [`Error::Range`] for a value whose last register is past address 65535.
    pub fn new(table: Table, address: u16, sample: Type) -> Result<Self, Error> {
        let refused = Error::Type { table, sample };
        let Type::Scalar(scalar) = sample else {
            return Err(refused);
        };
        let count = match (table.bits(), scalar) {
            (true, Scalar::Bool) | (false, Scalar::U16 | Scalar::I16) => 1,
            (false, Scalar::U32 | Scalar::I32 | Scalar::F32) => 2,
            (false, Scalar::U64 | Scalar::I64 | Scalar::F64) => 4,
            _ => return Err(refused),
        };
        let read = Self {
            table,
            address,
            scalar,
            count,
        };
        if read.end() > 1 << 16 {
            return Err(Error::Range { address, count });
        }
        Ok(read)
    }

    fn end(self) -> u32 {
        u32::from(self.address).saturating_add(u32::from(self.count))
    }

    /// The number of items of the read.
    pub(crate) fn count(self) -> u16 {
        self.count
    }

    /// Puts the registers of `value` in `order` into `out`, in address order: the
    /// inverse of the decode of [`Group::values`].
    ///
    /// # Panics
    ///
    /// When `value` is not of the type of the read, or `out` does not hold its
    /// registers.
    pub(crate) fn words(self, value: Value, order: Order, out: &mut [u16]) {
        assert_eq!(
            value.scalar(),
            self.scalar,
            "{value:?} is not a value of the write of {self:?}"
        );
        words(value, order, out);
    }
}

/// The order of the registers of a value of more than one register. Each register
/// is big-endian.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// The first register holds the high word.
    #[default]
    Big,
    /// The first register holds the low word.
    Little,
}

/// The requests that read each of `reads`, with registers in `order`. The reads of
/// one table whose items touch or overlap share a request of at most 125 registers
/// or 2000 bits. The groups go by table, then by address.
#[must_use]
pub fn groups(reads: &[Read], order: Order) -> Vec<Group> {
    let mut sorted: Vec<(usize, Read)> = reads.iter().copied().enumerate().collect();
    sorted.sort_by_key(|&(index, read)| {
        (read.table.function(), read.address, read.count, index)
    });
    let mut groups = Vec::new();
    let mut rest = sorted.as_slice();
    while let Some(&(_, first)) = rest.first() {
        let max = u32::from(first.table.max_read());
        let mut end = first.end();
        let mut len = rest.len();
        for (at, &(_, read)) in rest.iter().enumerate() {
            let joined = end.max(read.end());
            if read.table != first.table
                || u32::from(read.address) > end
                || joined.saturating_sub(u32::from(first.address)) > max
            {
                len = at;
                break;
            }
            end = joined;
        }
        let (taken, after) = rest.split_at(len);
        groups.push(Group::new(first.table, first.address, end, taken, order));
        rest = after;
    }
    groups
}

/// One request, and the reads that its reply holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    request: Request,
    order: Order,
    slots: Vec<Slot>,
}

/// A read in a group: its index in the list of reads, and its items in the reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    read: usize,
    scalar: Scalar,
    start: u16,
    end: u16,
}

impl Group {
    fn new(
        table: Table,
        start: u16,
        end: u32,
        reads: &[(usize, Read)],
        order: Order,
    ) -> Self {
        let offset = |at: u32| {
            u16::try_from(at.saturating_sub(u32::from(start)))
                .expect("invariant: a group is at most 2000 items")
        };
        let slots = reads
            .iter()
            .map(|&(read, item)| Slot {
                read,
                scalar: item.scalar,
                start: offset(u32::from(item.address)),
                end: offset(item.end()),
            })
            .collect();
        Self {
            request: Request::Read {
                table,
                start,
                count: offset(end),
            },
            order,
            slots,
        }
    }

    /// The request to send.
    #[must_use]
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The index in `reads` of each read that the reply holds, in address order.
    pub fn reads(&self) -> impl Iterator<Item = usize> {
        self.slots.iter().map(|slot| slot.read)
    }

    /// The index in `reads` and the value of each read, from the reply to
    /// [`Group::request`], in the order of [`Group::reads`].
    ///
    /// # Errors
    ///
    /// The exception of a device that refused the request.
    ///
    /// # Panics
    ///
    /// When `reply` is not one that [`Request::decode_reply`] gives for
    /// [`Group::request`].
    pub fn values<'a>(
        &'a self,
        reply: Reply<'a>,
    ) -> Result<impl Iterator<Item = (usize, Value)> + 'a, Exception> {
        let Request::Read { table, count, .. } = self.request else {
            unreachable!("invariant: a group holds a read request");
        };
        let count = usize::from(count);
        let matched = match (reply, table.bits()) {
            (Reply::Exception(exception), _) => return Err(exception),
            (Reply::Bits(bits), true) => bits.len() == count,
            (Reply::Registers(registers), false) => registers.len() == count,
            _ => false,
        };
        assert!(matched, "{reply:?} is not a reply to {:?}", self.request);
        let order = self.order;
        Ok(self.slots.iter().map(move |slot| {
            let value = match reply {
                Reply::Bits(bits) => Value::Bool(
                    bits.get(usize::from(slot.start))
                        .expect("invariant: the reply holds the request's count"),
                ),
                Reply::Registers(registers) => {
                    let words = registers
                        .get(usize::from(slot.start)..usize::from(slot.end))
                        .expect("invariant: the reply holds the request's count");
                    value(slot.scalar, words, order)
                }
                Reply::Written | Reply::Exception(_) => {
                    unreachable!("invariant: checked above")
                }
            };
            (slot.read, value)
        }))
    }
}

/// The value of `scalar` in `words`, the registers of one read in address order.
fn value(
    scalar: Scalar,
    words: impl DoubleEndedIterator<Item = u16>,
    order: Order,
) -> Value {
    let mut sample = [0; 8];
    let slots = sample.as_chunks_mut::<2>().0.iter_mut();
    let put = |(slot, word): (&mut [u8; 2], u16)| *slot = word.to_le_bytes();
    match order {
        Order::Big => slots.zip(words.rev()).for_each(put),
        Order::Little => slots.zip(words).for_each(put),
    }
    let sample = sample.get(..scalar.width()).unwrap_or_else(|| {
        panic!("invariant: `Read::new` gives no register read of {scalar:?}")
    });
    Value::read(scalar, sample)
}

/// Puts the registers of `value` in `order` into `out`, in address order: the
/// inverse of [`value`].
///
/// # Panics
///
/// When `out` does not hold exactly the registers of `value`.
fn words(value: Value, order: Order, out: &mut [u16]) {
    let mut sample = [0; 8];
    let scalar = value.scalar();
    let sample = sample.get_mut(..scalar.width()).unwrap_or_else(|| {
        panic!("invariant: `Read::new` gives no register read of {scalar:?}")
    });
    value.write(sample);
    let words = sample
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair));
    assert_eq!(words.len(), out.len(), "the registers of {value:?}");
    let put = |(slot, word): (&mut u16, u16)| *slot = word;
    match order {
        Order::Big => out.iter_mut().rev().zip(words).for_each(put),
        Order::Little => out.iter_mut().zip(words).for_each(put),
    }
}

/// Why a read or a write is not valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A write to a table that the protocol does not write. [`Read::new`] never
    /// gives it.
    ReadOnly {
        /// The table.
        table: Table,
    },
    /// A type that the table does not hold.
    Type {
        /// The table.
        table: Table,
        /// The type.
        sample: Type,
    },
    /// A value whose registers run past address 65535.
    Range {
        /// The address of the first register.
        address: u16,
        /// The number of registers.
        count: u16,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly { table } => write!(f, "the {} are read-only", table.name()),
            Self::Type { table, sample } => {
                write!(f, "the {} hold no {sample} values", table.name())
            }
            Self::Range { address, count } => write!(
                f,
                "a value of {count} registers from address {address} runs past address \
                 65535"
            ),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests;
