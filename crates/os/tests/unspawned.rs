//! A shard whose thread cannot spawn. The filter acts on the test thread and each
//! thread it starts, so it runs in a test binary of its own, with this one test only.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]
#![cfg(target_os = "linux")]

#[path = "../../env/tests/common/log.rs"]
#[expect(
    dead_code,
    unused_imports,
    unused_macros,
    reason = "this binary uses only the log and its guard"
)]
mod recorded;
#[path = "common/seccomp.rs"]
mod seccomp;

use env::shards::Config;
use env::thread::Error;
use recorded::{Log, Logs};

/// A failed spawn drops `main` on the calling thread, with the log of the shard.
#[test]
fn a_shard_whose_thread_cannot_spawn_drops_its_main_with_its_log() {
    let shards = os::shards().unwrap();
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    // glibc calls `clone3`, or `clone` where the kernel lacks it.
    seccomp::answer_calls(&[libc::SYS_clone3, libc::SYS_clone], |_, _| {
        Some(libc::EAGAIN)
    });
    let guard = Logs("a main dropped");
    let config = Config {
        name: "a".into(),
        core: None,
    };
    let found = (shards.logged(log.dispatch()))
        .start(config, move |_| async move {
            let _guard = guard;
        })
        .map(drop)
        .unwrap_err();
    let refused = Error::Start {
        name: "a".into(),
        reason: "Resource temporarily unavailable (os error 11)".into(),
    };
    assert_eq!(found, refused);
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a main dropped"]);
}
