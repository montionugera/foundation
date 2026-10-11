//! Breaker: the gate at the edges of the monotonic clock.

use control::lease::Lease;
use control::{Error, Gate, Writer};
use types::authority::Authority;
use types::time::{Monotonic, Span};

fn writer(subject: &str) -> Writer {
    Writer {
        subject: subject.parse().expect("valid name"),
        authority: Authority(1),
    }
}

fn lease(nanos: i64) -> Lease {
    Lease::new(Span::from_nanos(nanos)).expect("positive lease")
}

/// After `advance`, the deadline is `None` or later than `now`, for each lease
/// that ends near the last reading, also the largest lease.
#[test]
fn breaker_leaves_no_due_deadline_near_the_last_reading() {
    for start in [0, 1, u64::MAX / 2, u64::MAX - 10, u64::MAX - 1] {
        for nanos in [1, 2, 10, i64::MAX] {
            let mut gate = Gate::new();
            let now = Monotonic(start);
            let a = gate.open(writer("a"), Some(lease(nanos)), now);
            let b = gate.open(writer("b"), Some(lease(nanos)), now);
            let permit = gate.check(a, now).expect("a holds");
            gate.renew(permit);
            let Some(due) = gate.deadline() else {
                panic!("a leased holder has a deadline");
            };
            assert!(gate.advance(due), "{start} {nanos}");
            assert!(gate.deadline().is_none_or(|next| next > due), "{start} {nanos}");
            assert_eq!(gate.check(a, due).map(drop), Err(Error::Expired));
            if due < Monotonic(u64::MAX) {
                assert!(gate.check(b, due).is_ok(), "{start} {nanos}");
            } else {
                assert_eq!(gate.check(b, due).map(drop), Err(Error::Expired));
                assert!(gate.holder().is_none());
            }
        }
    }
}
