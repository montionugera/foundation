//! The node's status channels and the collector that fills them from each crate.

#[cfg(test)]
#[cfg(not(loom))]
mod tests;

use std::rc::Rc;

use clock::Status;
use estimate::Measurement;
use types::sample::{Scalar, Type};
use types::time::Span;

/// One status channel, under the node's name.
#[derive(Debug)]
pub(crate) struct Channel {
    /// The name after `<node>.`.
    pub(crate) name: &'static str,
    pub(crate) data_type: Type,
    /// The channel's value from the pulled status, or `None` for no sample.
    read: fn(&Pulled) -> Option<Value>,
}

/// The status of each crate, pulled one time for a sample of each channel.
struct Pulled {
    clock: Status,
    /// `None` when the node has no chunk store.
    store: Option<blob::Status>,
}

/// The node's status channels, a fixed set per release. [`Collector::collect`] gives
/// a value for each, in this order.
pub(crate) const TABLE: [Channel; 5] = [
    Channel {
        name: "clock.status",
        data_type: Type::Scalar(Scalar::U8),
        read: |pulled| {
            let code = match pulled.clock {
                Status::Unsynced(_) => 0,
                Status::Synced(_) => 1,
                Status::Holdover(..) => 2,
            };
            Some(Value::U8(code))
        },
    },
    // Mesh time minus monotonic time.
    Channel {
        name: "clock.offset",
        data_type: Type::Scalar(Scalar::Span),
        read: |pulled| time(pulled.clock).map(|m| Value::Span(m.offset())),
    },
    // An unknown error is the bound that `estimate::Measurement::unknown` gives.
    Channel {
        name: "clock.error",
        data_type: Type::Scalar(Scalar::Span),
        read: |pulled| time(pulled.clock).map(|m| Value::Span(m.error())),
    },
    Channel {
        name: "blob.corruptions",
        data_type: Type::Scalar(Scalar::U64),
        read: |pulled| pulled.store.map(|status| Value::U64(status.corruptions)),
    },
    Channel {
        name: "blob.failures",
        data_type: Type::Scalar(Scalar::U64),
        read: |pulled| pulled.store.map(|status| Value::U64(status.failures)),
    },
];

/// One sample of a status channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    U8(u8),
    U64(u64),
    Span(Span),
}

/// Pulls the status of each crate from a reader that the crate gives.
#[derive(Debug)]
pub(crate) struct Collector {
    clock: clock::Reader,
    store: Option<Rc<blob::Store>>,
}

impl Collector {
    /// A collector for the shard of `store`, the node's chunk store, or `None` for a
    /// node with no region.
    pub(crate) fn new(clock: clock::Reader, store: Option<Rc<blob::Store>>) -> Self {
        Self { clock, store }
    }

    /// The value of each channel in [`TABLE`] now, in its order, or `None` for a
    /// channel with no sample now. Reads each crate once. An unsynced clock has no
    /// offset or error, and a node with no chunk store has no count of one.
    pub(crate) fn collect(&self) -> [Option<Value>; TABLE.len()] {
        let pulled = Pulled {
            clock: self.clock.status(),
            store: self.store.as_deref().map(blob::Store::status),
        };
        TABLE.each_ref().map(|channel| (channel.read)(&pulled))
    }
}

fn time(status: Status) -> Option<Measurement> {
    match status {
        Status::Unsynced(_) => None,
        Status::Synced(m) | Status::Holdover(m, _) => Some(m),
    }
}
