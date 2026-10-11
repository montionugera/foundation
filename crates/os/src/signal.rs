//! The signal mask and `sigwait` calls of [`interrupt`](crate::interrupt).

use std::mem::MaybeUninit;
use std::thread;

use env::thread::Error;

/// Holds each of SIGINT and SIGTERM that reaches the process: one that the calling
/// thread does not block and whose action is not `SIG_IGN`. Holds it on the calling
/// thread and each thread it starts later, and starts thread `signal`, which calls
/// `fire` at the first, then takes them as with no hold, so a signal that the caller
/// blocked stays blocked. The thread has no handle: it lives until the process ends,
/// to take the second signal. Gives whether it holds one: with none, no thread starts
/// and `fire` drops. When the thread cannot start, the mask stays as it was.
#[expect(
    clippy::disallowed_methods,
    reason = "an env Handle joins its thread, and this one never ends"
)]
pub(crate) fn hold(fire: impl FnOnce() + Send + 'static) -> Result<bool, Error> {
    let Some(set) = reaching() else {
        return Ok(false);
    };
    // Before the start, so that the thread inherits the block.
    let old = mask(libc::SIG_BLOCK, &set);
    let name = "signal";
    match thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || serve(&set, &old, fire))
    {
        Ok(_) => Ok(true),
        Err(e) => {
            mask(libc::SIG_SETMASK, &old);
            Err(Error::Start {
                name: name.to_owned(),
                reason: e.to_string(),
            })
        }
    }
}

/// The set of SIGINT and SIGTERM that reach the process, or `None` when neither does.
fn reaching() -> Option<libc::sigset_t> {
    let blocked = mask(libc::SIG_BLOCK, &empty());
    let mut set = empty();
    let mut any = false;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `blocked` is an initialized sigset, and `signal` is a valid signal.
        let masked = unsafe { libc::sigismember(&raw const blocked, signal) } == 1;
        if !masked && !ignored(signal) {
            // SAFETY: `set` is an initialized sigset, and `signal` is a valid signal.
            let rc = unsafe { libc::sigaddset(&raw mut set, signal) };
            assert_eq!(rc, 0, "invariant: SIGINT and SIGTERM are valid signals");
            any = true;
        }
    }
    any.then_some(set)
}

/// Whether the action of `signal` is `SIG_IGN`.
fn ignored(signal: libc::c_int) -> bool {
    let mut action = MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: a null action changes nothing, and `action` is one sigaction for the
    // call to fill.
    let rc = unsafe { libc::sigaction(signal, std::ptr::null(), action.as_mut_ptr()) };
    assert_eq!(rc, 0, "invariant: SIGINT and SIGTERM are valid signals");
    // SAFETY: the call filled it.
    unsafe { action.assume_init() }.sa_sigaction == libc::SIG_IGN
}

fn empty() -> libc::sigset_t {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `set` is one sigset, which the call initializes.
    let rc = unsafe { libc::sigemptyset(set.as_mut_ptr()) };
    assert_eq!(rc, 0, "invariant: sigemptyset does not fail");
    // SAFETY: `sigemptyset` initialized it.
    unsafe { set.assume_init() }
}

/// Calls `fire` at the first signal of `set`, then sets the mask of the thread to
/// `old`, the mask before the hold.
fn serve(set: &libc::sigset_t, old: &libc::sigset_t, fire: impl FnOnce()) -> ! {
    let mut signal = 0;
    // SAFETY: `set` is an initialized sigset, and `signal` an int that the call writes.
    let rc = unsafe { libc::sigwait(set, &raw mut signal) };
    assert_eq!(rc, 0, "invariant: sigwait of a valid set does not fail");
    fire();
    mask(libc::SIG_SETMASK, old);
    // A signal goes only to a thread that takes it, so this one stays.
    loop {
        // SAFETY: `pause` takes no arguments.
        unsafe { libc::pause() };
    }
}

fn mask(how: libc::c_int, set: &libc::sigset_t) -> libc::sigset_t {
    let mut old = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `set` is an initialized sigset, and `old` one sigset that the call fills.
    let rc = unsafe { libc::pthread_sigmask(how, set, old.as_mut_ptr()) };
    assert_eq!(rc, 0, "invariant: a valid set is a valid argument");
    // SAFETY: the call filled it.
    unsafe { old.assume_init() }
}
