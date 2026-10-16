//! The commands of one connector: the request that writes each command value to a
//! device.

use types::sample::{Type, Value};

use crate::pdu::{Request, Table};
use crate::poll::{self, Order, Read};

/// Where the values of a command channel go in a device: a coil or the holding
/// registers from an address, and the type of the values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    read: Read,
    order: Order,
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
        Ok(Self {
            read: Read::new(table, address, sample)?,
            order,
        })
    }

    /// The request that writes `value`: function 5 for a coil, 6 for a value of one
    /// register, and 16 for a value of more.
    ///
    /// # Panics
    ///
    /// When `value` is not of the type of the write.
    #[must_use]
    pub fn request(&self, value: Value) -> Request {
        let Read {
            table,
            address,
            scalar,
            ..
        } = self.read;
        assert_eq!(
            value.scalar(),
            scalar,
            "{value:?} is not a value of the write {self:?}"
        );
        if table == Table::Coils {
            let Value::Bool(value) = value else {
                unreachable!("invariant: `Read::new` gives coils only `bool`");
            };
            return Request::WriteCoil { address, value };
        }
        let words = poll::words(value, self.order);
        if let [value] = *words.as_slice() {
            return Request::WriteRegister { address, value };
        }
        Request::WriteRegisters {
            start: address,
            values: words,
        }
    }
}

#[cfg(test)]
mod tests;
