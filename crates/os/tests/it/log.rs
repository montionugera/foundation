//! Tests of the log dispatch of each shard and thread on real threads.

use crate::panics::{assert_joins, shards, threads};
use crate::recorded::{self, Host};

fn run(case: fn(Host)) {
    case(Host {
        shards: shards(),
        threads: threads(),
        end: Box::new(|| {}),
        joins: assert_joins,
    });
}

recorded::tests!(recorded, run);
