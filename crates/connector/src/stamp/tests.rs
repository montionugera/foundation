use std::future;

use hub::writer::{Config, Writer};
use proptest::prelude::*;
use types::authority::Authority;
use types::time::{Span, Stamp};

use super::Stamps;
use crate::common::{create_status, env, run_on};

/// Runs `main` with a writer on the status index of `plant.stamp` and the clock of
/// its node.
fn with_writer<T: Send + 'static>(
    main: impl AsyncFnOnce(Writer, env::clock::Clock) -> T + Send + 'static,
) -> T {
    run_on(|node, tasks| async move {
        let (hub, _) = hub::testing::open(env(&node, tasks)).await;
        let status = create_status("plant.stamp");
        hub.set_definitions(status.iter().map(|(name, def)| (name, def)));
        let config = Config {
            subject: "plant.stamp".parse().expect("a valid name"),
            authority: Authority(1),
            lease: None,
            channels: vec!["plant.stamp.status.state".parse().expect("a valid name")],
        };
        let writer = hub.writer(config).await.expect("the writer opens");
        main(writer, node.clock()).await
    })
}

#[test]
fn a_request_is_stamped_at_its_midpoint() {
    let (sent, answered, output, stamp) = with_writer(async |writer, clock| {
        let mut stamps = Stamps::default();
        let sent = writer.now();
        let request = async {
            clock.sleep(Span::from_nanos(3_000)).await;
            7
        };
        let (output, stamp) = stamps.request(&writer, request).await;
        (sent, writer.now(), output, stamp)
    });
    assert_eq!(answered - sent, Span::from_nanos(3_000));
    assert_eq!(output, 7);
    assert_eq!(stamp, sent + Span::from_nanos(1_500));
}

#[test]
fn a_request_with_no_wait_is_stamped_after_the_one_before() {
    let (now, stamps) = with_writer(async |writer, _| {
        let mut stamps = Stamps::default();
        let mut given = Vec::new();
        for _ in 0..3 {
            given.push(stamps.request(&writer, future::ready(())).await.1);
        }
        (writer.now(), given)
    });
    let after = |n| now + Span::from_nanos(n);
    assert_eq!(stamps, [now, after(1), after(2)]);
}

#[test]
fn a_request_after_a_refusal_is_stamped_after_its_stamp() {
    let (now, refused, stamp) = with_writer(async |writer, _| {
        let mut stamps = Stamps::default();
        let now = writer.now();
        let refused = stamps.after(now + Span::from_nanos(20));
        let ((), stamp) = stamps.request(&writer, future::ready(())).await;
        (now, refused, stamp)
    });
    assert_eq!(refused, now + Span::from_nanos(21));
    assert_eq!(stamp, now + Span::from_nanos(22));
}

#[test]
fn a_refusal_under_a_request_stamp_is_stamped_after_it() {
    let (stamp, refused, later) = with_writer(async |writer, _| {
        let mut stamps = Stamps::default();
        let ((), stamp) = stamps.request(&writer, future::ready(())).await;
        let refused = stamps.after(stamp - Span::from_nanos(5));
        let ((), later) = stamps.request(&writer, future::ready(())).await;
        (stamp, refused, later)
    });
    assert_eq!(refused, stamp + Span::NANOSECOND);
    assert_eq!(later, stamp + Span::from_nanos(2));
}

#[test]
#[should_panic(expected = "stamp overflow: 9223372036854775807 ns + 1 ns")]
fn a_stamp_after_the_last_instant_panics() {
    let mut stamps = Stamps::default();
    stamps.next(Stamp::from_nanos(i64::MAX));
    stamps.next(Stamp::from_nanos(0));
}

#[test]
#[should_panic(expected = "stamp overflow: 9223372036854775807 ns + 1 ns")]
fn a_refusal_at_the_last_instant_panics() {
    Stamps::default().after(Stamp::from_nanos(i64::MAX));
}

#[test]
fn a_refusal_under_the_last_stamp_is_stamped_after_the_last_stamp() {
    let mut stamps = Stamps::default();
    stamps.next(Stamp::from_nanos(10));
    assert_eq!(stamps.after(Stamp::from_nanos(9)), Stamp::from_nanos(11));
}

#[test]
fn a_refusal_at_the_last_stamp_is_stamped_after_it() {
    let mut stamps = Stamps::default();
    stamps.next(Stamp::from_nanos(10));
    assert_eq!(stamps.after(Stamp::from_nanos(10)), Stamp::from_nanos(11));
}

/// A call of [`Stamps`]: a stamp at some time, or a refusal whose stamp before is
/// some time before or after the last stamp.
#[derive(Clone, Debug)]
enum Call {
    Next { time: i64 },
    After { ahead: i64 },
}

fn call() -> impl Strategy<Value = Call> {
    prop_oneof![
        3 => (0..1_000i64).prop_map(|time| Call::Next { time }),
        1 => (-1_000..1_000i64).prop_map(|ahead| Call::After { ahead }),
    ]
}

proptest! {
    /// In any order of calls, each stamp is after the one before it. A stamp is at
    /// its time when that is later, and a refusal gives the nanosecond after its
    /// stamp before when that is later.
    #[test]
    fn stamps_strictly_increase_from_their_times(
        calls in prop::collection::vec(call(), 1..50),
    ) {
        let mut stamps = Stamps::default();
        let mut last = None::<i64>;
        for call in calls {
            let (stamp, expected) = match call {
                Call::Next { time } => (
                    stamps.next(Stamp::from_nanos(time)),
                    last.map_or(time, |last| time.max(last + 1)),
                ),
                Call::After { ahead } => {
                    let before = last.unwrap_or(0) + ahead;
                    let expected = last.map_or(before, |last| before.max(last)) + 1;
                    (stamps.after(Stamp::from_nanos(before)), expected)
                }
            };
            prop_assert_eq!(stamp, Stamp::from_nanos(expected));
            prop_assert!(last.is_none_or(|last| stamp.nanos() > last));
            last = Some(stamp.nanos());
        }
    }
}
