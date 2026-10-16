//! The commands of one connector: the request that writes each command value to a
//! device.

use types::sample::{Type, Value};

use crate::pdu::{Request, Table};
use crate::poll::{self, Order, Read};

/// Where the values of a command channel go in a device: a coil or the holding
/// registers from an address, and the type of the values. Two writes are equal when
/// they write the same place and type in the same order.
#[derive(Clone, Debug)]
pub struct Write {
    read: Read,
    order: Order,
    request: Request,
}

impl Write {
    /// A write of values of type `sample` at `address` in `table`, with registers in
    /// `order`. A value of more than one register takes the registers from `address`
    /// up.
    ///
    /// # Errors
    ///
    /// - [`poll::Error::ReadOnly`] for discrete inputs and input registers, which
    ///   the protocol does not write.
    /// - [`poll::Error::Type`] and [`poll::Error::Range`], as [`poll::Read::new`]
    ///   gives them.
    pub fn new(
        table: Table,
        address: u16,
        sample: Type,
        order: Order,
    ) -> Result<Self, poll::Error> {
        if table.read_only() {
            return Err(poll::Error::ReadOnly { table });
        }
        let read = Read::new(table, address, sample)?;
        let request = match (table, read.count()) {
            (Table::Coils, _) => Request::WriteCoil {
                address,
                value: false,
            },
            (_, 1) => Request::WriteRegister { address, value: 0 },
            (_, count) => Request::WriteRegisters {
                start: address,
                values: vec![0; usize::from(count)],
            },
        };
        Ok(Self {
            read,
            order,
            request,
        })
    }

    /// The request that writes `value`: function 5 for a coil, 6 for a value of one
    /// register, and 16 for a value of more.
    ///
    /// # Panics
    ///
    /// When `value` is not of the type of the write.
    pub fn request(&mut self, value: Value) -> &Request {
        let Self {
            read,
            order,
            request,
        } = self;
        match request {
            Request::WriteCoil { value: bit, .. } => {
                let Value::Bool(value) = value else {
                    panic!("{value:?} is not a value of the write of {read:?}");
                };
                *bit = value;
            }
            Request::WriteRegister { value: word, .. } => {
                read.words(value, *order, std::slice::from_mut(word));
            }
            Request::WriteRegisters { values, .. } => read.words(value, *order, values),
            Request::Read { .. } | Request::WriteCoils { .. } => {
                unreachable!("invariant: `Write::new` builds function 5, 6, or 16")
            }
        }
        request
    }
}

impl PartialEq for Write {
    fn eq(&self, other: &Self) -> bool {
        (self.read, self.order) == (other.read, other.order)
    }
}

impl Eq for Write {}

#[cfg(test)]
mod tests;
