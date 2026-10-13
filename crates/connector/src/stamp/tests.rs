use std::cell::Cell;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use proptest::prelude::*;
use types::time::Stamp;

use super::Stamps;

/// Gives the output and stamp of a request that `now` sees from `sent` to `answered`.
fn request(stamps: &mut Stamps, sent: i64, answered: i64) -> (u8, Stamp) {
    let now = Cell::new(sent);
    let request = async {
        now.set(answered);
        7
    };
    let mut stamped = pin!(stamps.request(|| Stamp::from_nanos(now.get()), request));
    match stamped
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the request is ready"),
    }
}

#[test]
fn a_request_is_stamped_at_its_midpoint() {
    let mut stamps = Stamps::default();
    assert_eq!(
        request(&mut stamps, 1_000, 3_000),
        (7, Stamp::from_nanos(2_000))
    );
    assert_eq!(
        request(&mut stamps, 5_000, 5_000),
        (7, Stamp::from_nanos(5_000))
    );
}

#[test]
fn a_request_is_stamped_after_the_one_before() {
    let mut stamps = Stamps::default();
    assert_eq!(request(&mut stamps, 0, 10).1, Stamp::from_nanos(5));
    assert_eq!(request(&mut stamps, 10, 10).1, Stamp::from_nanos(10));
    assert_eq!(request(&mut stamps, 10, 10).1, Stamp::from_nanos(11));
    assert_eq!(request(&mut stamps, 10, 14).1, Stamp::from_nanos(12));
}

#[test]
fn a_request_after_a_refusal_is_stamped_after_its_stamp() {
    let mut stamps = Stamps::default();
    assert_eq!(request(&mut stamps, 0, 10).1, Stamp::from_nanos(5));
    assert_eq!(stamps.after(Stamp::from_nanos(20)), Stamp::from_nanos(21));
    assert_eq!(request(&mut stamps, 10, 12).1, Stamp::from_nanos(22));
}

#[test]
fn the_midpoint_of_the_ends_of_time_does_not_overflow() {
    let mut stamps = Stamps::default();
    let (_, stamp) = request(&mut stamps, i64::MAX - 1, i64::MAX);
    assert_eq!(stamp, Stamp::from_nanos(i64::MAX - 1));
}

/// A call of [`Stamps`]: a request that waits and then takes a time, or a refusal
/// whose stamp before is some time after the last stamp.
#[derive(Clone, Debug)]
enum Call {
    Request { wait: i64, took: i64 },
    After { ahead: i64 },
}

fn call() -> impl Strategy<Value = Call> {
    prop_oneof![
        3 => (0..1_000i64, 0..1_000i64).prop_map(|(wait, took)| Call::Request { wait, took }),
        1 => (0..1_000i64).prop_map(|ahead| Call::After { ahead }),
    ]
}

proptest! {
    /// In any order of calls, each stamp is after the one before it. A request is at
    /// its midpoint when that is later, and a refusal gives the nanosecond after its
    /// stamp before.
    #[test]
    fn stamps_strictly_increase_from_the_midpoints(
        calls in prop::collection::vec(call(), 1..50),
    ) {
        let mut stamps = Stamps::default();
        let (mut time, mut last) = (0i64, None::<i64>);
        for call in calls {
            let (stamp, expected) = match call {
                Call::Request { wait, took } => {
                    let sent = time + wait;
                    time = sent + took;
                    let midpoint = sent + took / 2;
                    let (_, stamp) = request(&mut stamps, sent, time);
                    (stamp, last.map_or(midpoint, |last| midpoint.max(last + 1)))
                }
                Call::After { ahead } => {
                    let before = last.unwrap_or(0) + ahead;
                    (stamps.after(Stamp::from_nanos(before)), before + 1)
                }
            };
            prop_assert_eq!(stamp, Stamp::from_nanos(expected));
            prop_assert!(last.is_none_or(|last| stamp.nanos() > last));
            last = Some(stamp.nanos());
        }
    }
}
