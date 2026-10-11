//! `os::interrupt` holds the first SIGINT or SIGTERM and completes its future at it,
//! and a second one ends the process. This binary has no test harness: the threads
//! of a harness start before the hold, so a signal would end the process. With the
//! argument `child`, it is the process that the test signals, with `blocked`, one
//! that blocks SIGTERM first, with `pending`, one that also makes it pending, and with
//! `ignored`, one that ignores SIGINT and sends it to itself. With `both`, it blocks
//! both and writes whether the future stays pending.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::pin::pin;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[path = "../common/mask.rs"]
#[expect(dead_code, reason = "this binary only sets the mask")]
mod mask;

use rustix::process::{Pid, Signal, getpid, kill_process};
use tokio::time::{sleep, timeout};

/// How long the future must wait with no signal.
const QUIET: Duration = Duration::from_millis(200);
/// The bound of each wait for a signal. A child ends in twice this, so no wait of
/// the test on it hangs.
const BOUND: Duration = Duration::from_secs(10);

fn main() {
    // A child inherits the mask of the test, which holds the signals once a test has
    // called `interrupt`.
    match std::env::args_os().nth(1) {
        Some(arg) if arg == "child" => {
            mask::set(&[]);
            child(&[]);
        }
        Some(arg) if arg == "blocked" => {
            mask::set(&[libc::SIGTERM]);
            child(&[]);
        }
        Some(arg) if arg == "pending" => {
            mask::set(&[libc::SIGTERM]);
            kill_process(getpid(), Signal::TERM).expect("the child signals itself");
            child(&[]);
        }
        Some(arg) if arg == "ignored" => {
            mask::set(&[]);
            ignore(libc::SIGINT);
            child(&[Signal::INT]);
        }
        Some(arg) if arg == "both" => {
            mask::set(&[libc::SIGINT, libc::SIGTERM]);
            both();
        }
        #[cfg(target_os = "linux")] // needs /proc
        Some(arg) if arg == "early" => {
            mask::set(&[]);
            early(false);
        }
        #[cfg(target_os = "linux")] // needs /proc
        Some(arg) if arg == "early-waiting" => {
            mask::set(&[]);
            early(true);
        }
        _ => {
            a_blocked_sigterm_stays_blocked_after_the_first_signal();
            a_blocked_sigterm_pending_before_the_call_does_not_complete_the_future();
            an_ignored_sigint_does_not_complete_the_future_or_end_the_process();
            with_both_blocked_the_future_stays_pending();
            a_second_signal_ends_the_process();
            the_future_completes_at_the_first_signal();
            #[cfg(target_os = "linux")] // needs /proc
            a_signal_before_the_first_poll_completes_the_future();
        }
    }
}

/// Holds the signals and sends itself `own`. Writes `held` when the future then stays
/// pending for [`QUIET`], else `early`, then `fired` when it completes, and waits for
/// the end of the process.
fn child(own: &[Signal]) {
    let interrupt = os::interrupt().expect("the signal thread starts");
    let mut output = std::io::stdout();
    runtime().block_on(async {
        let mut interrupt = pin!(interrupt);
        for &signal in own {
            kill_process(getpid(), signal).expect("the child signals itself");
        }
        if timeout(QUIET, interrupt.as_mut()).await.is_ok() {
            writeln!(output, "early").expect("a write to the test");
            return;
        }
        writeln!(output, "held").expect("a write to the test");
        timeout(BOUND, interrupt)
            .await
            .unwrap_or_else(|_| panic!("no signal came in {BOUND:?}"));
        writeln!(output, "fired").expect("a write to the test");
        sleep(BOUND).await;
    });
}

/// Holds the signals with both blocked and sends both to itself. Writes `held` when
/// the future then stays pending for [`QUIET`], else `early`, and on Linux whether
/// a signal thread started.
fn both() {
    let interrupt = os::interrupt().expect("the call succeeds");
    kill_process(getpid(), Signal::INT).expect("the child signals itself");
    kill_process(getpid(), Signal::TERM).expect("the child signals itself");
    let early = runtime()
        .block_on(async { timeout(QUIET, interrupt).await })
        .is_ok();
    let mut output = std::io::stdout();
    writeln!(output, "{}", if early { "early" } else { "held" }).expect("a write");
    #[cfg(target_os = "linux")] // needs /proc
    writeln!(output, "thread {}", signal_thread_blocks_sigint().is_some())
        .expect("a write to the test");
}

/// Sets the action of `signal` to `SIG_IGN`.
#[expect(unsafe_code, reason = "a signal action is an OS call")]
fn ignore(signal: libc::c_int) {
    // SAFETY: `SIG_IGN` runs no code in the process.
    let old = unsafe { libc::signal(signal, libc::SIG_IGN) };
    assert_ne!(old, libc::SIG_ERR, "{}", std::io::Error::last_os_error());
}

/// Starts a child in `mode`, checks that it holds the signals, sends it `signal`, and
/// checks that the future completes. Gives the child, which waits for its end.
fn fire(mode: &str, signal: Signal) -> Child {
    let mut child = Command::new(std::env::current_exe().expect("the test binary"))
        .arg(mode)
        .stdout(Stdio::piped())
        .spawn()
        .expect("the child starts");
    let pid = Pid::from_child(&child);
    let mut lines = BufReader::new(child.stdout.take().expect("a pipe")).lines();
    let mut line = || lines.next().expect("a line of the child").expect("a read");
    assert_eq!(line(), "held", "{mode}: the future stays pending");
    kill_process(pid, signal).expect("the test signals the child");
    assert_eq!(
        line(),
        "fired",
        "{mode}: the {signal:?} completes the future"
    );
    child
}

/// Ends `child`, which is still there.
fn end(mut child: Child) {
    child.kill().expect("the child is there");
    child.wait().expect("the child ends");
}

/// With no hold, a SIGTERM that the caller blocks does not end the process. After
/// the first SIGINT, the hold must give that back, as it does when its thread
/// cannot start.
fn a_blocked_sigterm_stays_blocked_after_the_first_signal() {
    let mut child = fire("blocked", Signal::INT);
    kill_process(Pid::from_child(&child), Signal::TERM).expect("the test signals");
    runtime().block_on(async { sleep(QUIET).await });
    let ended = child.try_wait().expect("a wait");
    assert_eq!(
        ended, None,
        "a SIGTERM that the caller blocked ended the child"
    );
    end(child);
}

/// The hold leaves out a signal that the caller blocks, so one that is pending
/// before the call stays pending.
fn a_blocked_sigterm_pending_before_the_call_does_not_complete_the_future() {
    end(fire("pending", Signal::INT));
}

/// The hold leaves out a signal that the caller ignores, so it stays ignored.
fn an_ignored_sigint_does_not_complete_the_future_or_end_the_process() {
    end(fire("ignored", Signal::TERM));
}

fn with_both_blocked_the_future_stays_pending() {
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .arg("both")
        .output()
        .expect("the child runs");
    assert!(output.status.success(), "{}", output.status);
    #[cfg(target_os = "linux")] // needs /proc
    let expected = "held\nthread false\n";
    #[cfg(not(target_os = "linux"))]
    let expected = "held\n";
    let found = String::from_utf8(output.stdout).expect("text");
    assert_eq!(
        found, expected,
        "the future stays pending, with no signal thread"
    );
}

fn a_second_signal_ends_the_process() {
    let mut child = fire("child", Signal::TERM);
    kill_process(Pid::from_child(&child), Signal::TERM).expect("the test signals");
    let status = child.wait().expect("the child ends");
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status}");
}

fn the_future_completes_at_the_first_signal() {
    let interrupt = os::interrupt().expect("the signal thread starts");
    runtime().block_on(async {
        let mut interrupt = pin!(interrupt);
        assert!(
            timeout(QUIET, interrupt.as_mut()).await.is_err(),
            "the future completed with no signal"
        );
        kill_process(getpid(), Signal::INT).expect("the process signals itself");
        timeout(BOUND, interrupt)
            .await
            .unwrap_or_else(|_| panic!("no signal came in {BOUND:?}"));
    });
}

/// Holds the signals and signals itself, then, once the signal thread has taken the
/// signal, polls the future once and writes whether it completed. When `waiting`, the
/// signal comes after the signal thread is in `sigwait`.
#[cfg(target_os = "linux")] // needs /proc
fn early(waiting: bool) {
    use std::task::{Context, Waker};

    let interrupt = os::interrupt().expect("the signal thread starts");
    if waiting {
        let in_sigwait = async {
            while signal_thread_blocks_sigint() != Some(false) {
                sleep(Duration::from_millis(1)).await;
            }
        };
        runtime()
            .block_on(async { timeout(BOUND, in_sigwait).await })
            .unwrap_or_else(|_| panic!("no sigwait in {BOUND:?}"));
    }
    kill_process(getpid(), Signal::INT).expect("the process signals itself");
    // The signal thread takes SIGINT again only after it fires the future.
    let taken = async {
        while untaken() {
            sleep(Duration::from_millis(1)).await;
        }
    };
    runtime()
        .block_on(async { timeout(BOUND, taken).await })
        .unwrap_or_else(|_| panic!("no signal came in {BOUND:?}"));
    let polled = pin!(interrupt).poll(&mut Context::from_waker(Waker::noop()));
    let mut output = std::io::stdout();
    writeln!(output, "{}", polled.is_ready()).expect("a write to the test");
}

/// Whether SIGINT is pending in this process, or the thread `signal` has not named
/// itself yet or blocks SIGINT. During `sigwait`, the thread does not block it, so the
/// pending set tells that it is not yet taken. It reads the pending set first: in the
/// other order, it can read the mask during `sigwait` and the pending set after the
/// wake, and see neither.
#[cfg(target_os = "linux")] // needs /proc
fn untaken() -> bool {
    let process = std::fs::read_to_string("/proc/self/status").expect("a status");
    has_sigint(&process, "ShdPnd:") || signal_thread_blocks_sigint() != Some(false)
}

/// Whether the thread `signal` blocks SIGINT, or `None` when no thread has that name
/// yet.
#[cfg(target_os = "linux")] // needs /proc
fn signal_thread_blocks_sigint() -> Option<bool> {
    for task in std::fs::read_dir("/proc/self/task").expect("the tasks list") {
        let task = task.expect("a task").path();
        let name = std::fs::read_to_string(task.join("comm")).expect("a name");
        if name.trim_end() == "signal" {
            let status =
                std::fs::read_to_string(task.join("status")).expect("a status");
            return Some(has_sigint(&status, "SigBlk:"));
        }
    }
    None
}

/// Whether the signal set of `field` in `status`, a status file of `/proc`, has SIGINT.
#[cfg(target_os = "linux")] // needs /proc
fn has_sigint(status: &str, field: &str) -> bool {
    let mask = status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .expect("a signal mask");
    let mask = u64::from_str_radix(mask.trim(), 16).expect("a hex mask");
    mask & (1 << (libc::SIGINT - 1)) != 0
}

#[cfg(target_os = "linux")] // needs /proc
fn a_signal_before_the_first_poll_completes_the_future() {
    for mode in ["early", "early-waiting"] {
        let output = Command::new(std::env::current_exe().expect("the test binary"))
            .arg(mode)
            .output()
            .expect("the child runs");
        assert!(output.status.success(), "{mode}: {}", output.status);
        let polled = String::from_utf8(output.stdout).expect("text");
        assert_eq!(polled, "true\n", "{mode}: the first poll completes");
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime")
}
