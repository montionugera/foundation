//! Panics on shards and dedicated threads that abort the process. Each test runs in a
//! child process of this test binary.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "common/panics.rs"]
mod panics;

/// Runs `child` in a new process of this test binary that runs only the calling test,
/// and asserts that the process aborts at a panic that unwinds into the unwind of
/// another panic.
fn assert_aborts(child: impl FnOnce()) {
    const CHILD: &str = "OS_ABORTS_CHILD";
    #[expect(
        clippy::disallowed_methods,
        reason = "the parent sets it for the child"
    )]
    if std::env::var_os(CHILD).is_some() {
        child();
        return;
    }
    let thread = std::thread::current();
    let test = thread.name().expect("invariant: libtest names the thread");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(aborted(output.status), "{:?}: {stderr}", output.status);
    #[cfg(unix)]
    assert!(
        stderr.contains("panic in a destructor during cleanup"),
        "{stderr}"
    );
    // The MSVC runtime ends the process at the panic in the cleanup, before Rust can
    // write its text, and libtest holds what the panic hook wrote.
    #[cfg(windows)]
    assert_eq!(stderr, "");
}

/// Whether `status` is that of an abort: SIGABRT, or on Windows the code of a fast
/// fail, `STATUS_STACK_BUFFER_OVERRUN`.
fn aborted(status: std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(6)
    }
    #[cfg(windows)]
    {
        status.code().map(i32::cast_unsigned) == Some(0xC000_0409)
    }
}

mod shards {
    use std::future::pending;

    use env::tasks::Tasks;

    use super::assert_aborts;
    use crate::panics::{Armed, Bomb, assert_joins, config, panicked, shards};

    #[test]
    fn a_panic_in_the_poll_and_then_the_drop_of_a_tokio_task_aborts_the_process() {
        assert_aborts(|| {
            let main = |_: Tasks| async {
                drop(tokio::task::spawn_local(Armed {
                    faulty: true,
                    _bomb: Bomb,
                }));
                pending::<()>().await;
            };
            let handle = shards().start(config("shard-16"), main).unwrap();
            assert_joins(handle, Ok(()));
        });
    }

    #[test]
    fn a_panic_over_a_local_that_panics_in_its_drop_in_a_task_aborts_the_process() {
        assert_aborts(|| {
            let main = |tasks: Tasks| async move {
                tasks.spawn(async {
                    let _bomb = Bomb;
                    panic!("task");
                });
                pending::<()>().await;
            };
            let handle = shards().start(config("shard-17"), main).unwrap();
            assert_joins(handle, panicked("shard-17"));
        });
    }

    #[test]
    fn a_panic_over_a_local_that_panics_in_its_drop_in_main_aborts_the_process() {
        assert_aborts(|| {
            let main = |_: Tasks| async {
                let _bomb = Bomb;
                panic!("main");
            };
            let handle = shards().start(config("shard-18"), main).unwrap();
            assert_joins(handle, panicked("shard-18"));
        });
    }
}

mod threads {
    use std::future::pending;

    use super::assert_aborts;
    use crate::panics::{Armed, Bomb, assert_joins, panicked, threads};

    #[test]
    fn a_panic_in_the_poll_and_then_the_drop_of_a_tokio_task_aborts_the_process() {
        assert_aborts(|| {
            let body = || async {
                drop(tokio::spawn(Armed {
                    faulty: true,
                    _bomb: Bomb,
                }));
                pending::<()>().await;
            };
            let handle = threads().start("thread-15", body).unwrap();
            assert_joins(handle, Ok(()));
        });
    }

    #[test]
    fn a_panic_over_a_local_that_panics_in_its_drop_in_the_body_aborts_the_process() {
        assert_aborts(|| {
            let body = || async {
                let _bomb = Bomb;
                panic!("body");
            };
            let handle = threads().start("thread-16", body).unwrap();
            assert_joins(handle, panicked("thread-16"));
        });
    }
}
