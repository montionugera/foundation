//! A warm `sdk::Writer::write` and `write_series` against a node process make no
//! allocation on the caller's thread. The count covers only that thread, as the
//! client's thread allocates at the ACK of each frame. Calls of `count` must not
//! overlap, and a harness runs tests at once, so this binary has none.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]
#![cfg_attr(not(all(unix, not(loom))), expect(unused, reason = "Unix only"))]

#[cfg(all(unix, not(loom)))]
#[expect(
    dead_code,
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a helper of the node tests fails loud, and the test uses only some"
)]
#[path = "it/plant.rs"]
mod plant;
#[cfg(all(unix, not(loom)))]
#[expect(
    dead_code,
    clippy::missing_assert_message,
    reason = "the test uses only some of the helpers of the node tests"
)]
#[path = "it/rig.rs"]
mod rig;
#[cfg(all(unix, not(loom)))]
#[expect(dead_code, reason = "the test uses only some of the helpers")]
#[path = "it/status.rs"]
mod status;

#[cfg(all(unix, not(loom)))]
#[global_allocator]
static ALLOCATOR: counting::Local = counting::Local::new();

/// Calls before the count, which fill the pools and the channels of the writer.
const WARMUP: usize = 100;

/// Calls in the count.
const CALLS: usize = 1_000;

/// The samples of each series of a `write_series`.
const SAMPLES: u32 = 64;

#[cfg(all(unix, not(loom)))]
fn main() {
    use sdk::Client;
    use sdk::write::{Config, Outcome};
    use types::sample::Value;

    let (mut rig, node, subject) = plant::planted();
    let client = Client::connect(&node, subject).expect("the client connects");
    let channels = ["plant.a", "plant.b"].map(|name| name.parse().expect("a name"));
    let config = || Config::new(channels.to_vec());
    let mut writer = client.open_writer(config()).expect("the writer opens");
    let values = [(0, Value::I64(1)), (1, Value::I64(2))];
    let mut write = || {
        let written = writer.write(&values);
        assert!(matches!(written, Ok(Outcome::Taken { .. })), "{written:?}");
    };
    (0..WARMUP).for_each(|_| write());
    let ((), allocations) = ALLOCATOR.count(|| (0..CALLS).for_each(|_| write()));
    assert_eq!(allocations, 0, "{CALLS} warm writes of values allocate");
    assert_eq!(writer.close(), Ok(()), "the writer closes");

    let mut writer = client.open_writer(config()).expect("the writer opens");
    let index = writer.index();
    let len = 8 * usize::try_from(SAMPLES).expect("a length");
    let (values, mut stamps) = (vec![3_u8; len], vec![0_u8; len]);
    let mut next = now() + 1_000_000_000;
    let mut write = || {
        for stamp in stamps.as_chunks_mut::<8>().0 {
            stamp.copy_from_slice(&next.to_le_bytes());
            next += 1;
        }
        let series = [(0, values.as_slice()), (index, stamps.as_slice())];
        let written = writer.write_series(SAMPLES, &series);
        assert!(matches!(written, Ok(Outcome::Taken { .. })), "{written:?}");
    };
    (0..WARMUP).for_each(|_| write());
    let ((), allocations) = ALLOCATOR.count(|| (0..CALLS).for_each(|_| write()));
    assert_eq!(allocations, 0, "{CALLS} warm writes of series allocate");
    assert_eq!(writer.close(), Ok(()), "the writer closes");
    assert_eq!(client.close(), Ok(()), "the client closes");
    rig.stop();
}

#[cfg(not(all(unix, not(loom))))]
fn main() {}

/// The wall time in ns since the Unix epoch.
#[cfg(all(unix, not(loom)))]
#[expect(clippy::disallowed_methods, reason = "a stamp near the node's clock")]
fn now() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    i64::try_from(since.expect("a time after 1970").as_nanos()).expect("a stamp")
}

#[cfg(all(unix, not(loom)))]
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8")
}
