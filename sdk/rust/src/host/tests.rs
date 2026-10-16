use env::clock::Driver as _;
use env::entropy::Driver as _;
use types::time::Span;

use super::*;

#[test]
fn reads_from_its_epoch() {
    let clock = Clock::new();
    let now = Duration::from_nanos(clock.now().0);
    #[expect(clippy::disallowed_methods, reason = "the test reads the OS clock")]
    let after = Instant::now();
    assert!(clock.epoch() + now <= after);
    assert!(clock.epoch() + Duration::from_secs(1) > after);
}

#[test]
fn a_timer_never_fires_before_its_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let clock = env::clock::Clock::new(Clock::new());
    runtime.block_on(async {
        let mut sleep = clock.sleep(Span::MILLISECOND);
        for millis in [5, 1, 0, 7] {
            let deadline = clock.now() + Span::from_nanos(millis * 1_000_000);
            sleep.reset(deadline);
            (&mut sleep).await;
            assert!(clock.now() >= deadline, "early for {millis} ms");
        }
    });
}

#[test]
fn fills_from_the_os() {
    let mut bytes = [0; 32];
    Entropy.fill(&mut bytes);
    assert_ne!(bytes, [0; 32]);
}
