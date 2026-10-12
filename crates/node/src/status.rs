//! The node's status channels and the collector that fills them from each crate.

#[cfg(test)]
#[cfg(not(loom))]
mod tests;

use std::collections::BTreeSet;
use std::rc::Rc;

use estimate::Measurement;
use types::name::Name;
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
    clock: clock::Status,
    store: blob::Status,
}

/// The node's status channels, a fixed set per release. [`Collector::collect`] gives
/// a value for each, in this order.
pub(crate) const TABLE: [Channel; 5] = [
    Channel {
        name: "clock.status",
        data_type: Type::Scalar(Scalar::U8),
        read: |pulled| {
            let code = match pulled.clock {
                clock::Status::Unsynced(_) => 0,
                clock::Status::Synced(_) => 1,
                clock::Status::Holdover(..) => 2,
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
        read: |pulled| Some(Value::U64(pulled.store.corruptions)),
    },
    Channel {
        name: "blob.failures",
        data_type: Type::Scalar(Scalar::U64),
        read: |pulled| Some(Value::U64(pulled.store.failures)),
    },
];

/// The most bytes of a name of [`TABLE`], so `<node>.<status>` fits a name for each
/// node name of at most [`crate::name::MAX_BYTES`].
pub(crate) const MAX_BYTES: usize = 63;
const _: () = {
    let mut i = 0;
    while i < TABLE.len() {
        assert!(TABLE[i].name.len() <= MAX_BYTES, "a status name fits");
        i += 1;
    }
};

/// The name of each status channel of [`TABLE`], under the node's name. A node gives
/// these in its founding and in its join request.
pub(crate) fn names() -> BTreeSet<Name> {
    let name = |c: &Channel| c.name.parse().expect("invariant: a status name parses");
    TABLE.iter().map(name).collect()
}

/// One sample of a status channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    U8(u8),
    U64(u64),
    Span(Span),
}

/// Pulls the status of each crate: the clock through its reader, and the chunk store on
/// the shard of the store.
#[derive(Debug)]
pub(crate) struct Collector {
    clock: clock::Reader,
    store: Rc<blob::Store>,
}

impl Collector {
    /// A collector of `clock` and of `store`, the node's chunk store. It runs on the
    /// shard of `store`.
    pub(crate) fn new(clock: clock::Reader, store: Rc<blob::Store>) -> Self {
        Self { clock, store }
    }

    /// The value of each channel in [`TABLE`] now, in its order, or `None` for a
    /// channel with no sample now. Reads each crate once. An unsynced clock has no
    /// offset or error.
    pub(crate) fn collect(&self) -> [Option<Value>; TABLE.len()] {
        let pulled = Pulled {
            clock: self.clock.status(),
            store: self.store.status(),
        };
        TABLE.each_ref().map(|channel| (channel.read)(&pulled))
    }
}

fn time(status: clock::Status) -> Option<Measurement> {
    match status {
        clock::Status::Unsynced(_) => None,
        clock::Status::Synced(m) | clock::Status::Holdover(m, _) => Some(m),
    }
}
