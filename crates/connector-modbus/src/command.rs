//! The commands of one connector: the request that writes each command value to a
//! device.

use types::sample::{Type, Value};

use crate::pdu::{Request, Table};
use crate::poll::{self, Order, Read};

/// Where the values of a command channel go in a device: a coil or the holding
/// registers from an address, and the type of the values.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        Ok(Self {
            read,
            order,
            request: read.write(),
        })
    }

    /// The request that writes `value`: function 5 for a coil, 6 for a value of one
    /// register, and 16 for a value of more.
    ///
    /// # Panics
    ///
    /// When `value` is not of the type of the write.
    pub fn request(&mut self, value: Value) -> &Request {
        self.read.put(value, self.order, &mut self.request);
        &self.request
    }
}

#[cfg(test)]
mod tests;
