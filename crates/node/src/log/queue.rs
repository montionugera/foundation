//! The queue of a node's log and the thread `log` that writes it, so no event waits on
//! the writer.

use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use env::thread::{Error, Handle};
use tracing::Dispatch;
use tracing_subscriber::fmt::MakeWriter;

use super::Line;

/// The lines that the queue holds. A line that finds it full drops.
const LINES: usize = 1024;
/// The longest a panic hook waits for the log.
const PANIC: Duration = Duration::from_secs(1);

/// Waits for the lines of a node's log to reach its writer. Clones wait on the same
/// log.
#[derive(Clone, Debug)]
pub struct Flush {
    queue: Arc<Queue>,
    /// The node's dispatch, whose events go into `queue`.
    pub(crate) log: Dispatch,
}

impl Flush {
    /// The log `line` of `writer`, whose dispatch holds each event of `level` or above,
    /// with no thread `log` yet. Each line waits in the queue until a thread starts:
    /// at [`Flush::hold`], [`Flush::resume`], or [`Flush::wait`].
    pub(super) fn new(
        writer: Box<dyn Write + Send>,
        line: Line,
        level: tracing::Level,
        threads: &env::threads::Threads,
    ) -> Self {
        let (lines, receiver) = mpsc::sync_channel(LINES);
        let queue = Arc::new(Queue {
            lines,
            waker: Mutex::new(None),
            gap: Mutex::new(None),
            sent: AtomicU64::new(0),
            closed: AtomicBool::new(true),
            state: Mutex::new(State {
                parts: Some(Parts { writer, receiver }),
                running: false,
                written: 0,
                handles: Vec::new(),
            }),
            written: Condvar::new(),
            threads: threads.logged(Dispatch::none()),
            line: line.clone(),
        });
        let log = super::dispatch(line, level, Pipe(Arc::clone(&queue)));
        Self { queue, log }
    }

    /// Starts the thread `log`, which runs until each clone of the [`Hold`] has
    /// dropped and the queue is empty.
    ///
    /// # Errors
    ///
    /// [`Error::Start`] when the thread cannot start. The log then has no hold.
    pub(crate) fn hold(&self) -> Result<Hold, Error> {
        self.queue.closed.store(false, Ordering::Release);
        let hold = Hold(Arc::clone(&self.queue));
        self.queue.resume()?;
        Ok(hold)
    }

    /// Starts the thread `log` when none runs, so the lines that came after each
    /// [`Hold`] dropped reach the writer. Call it outside an event: an OS thread
    /// cannot start inside one.
    ///
    /// # Errors
    ///
    /// [`Error::Start`] when the thread cannot start. The lines then wait for the next
    /// call.
    pub(crate) fn resume(&self) -> Result<(), Error> {
        self.queue.resume()
    }

    /// Starts the thread `log` as [`Flush::resume`] does, then blocks until the writer
    /// has written each line that was in the queue at the call, or until `bound`
    /// passes. Once each shard has ended, also blocks until the thread `log` has
    /// ended, and joins it. Returns `false` when `bound` passed first or the thread
    /// could not start. Blocks on real time: for `main` only, never in sim.
    ///
    /// # Panics
    ///
    /// When the thread `log` panicked after it ended its run, which is a defect.
    #[expect(
        clippy::must_use_candidate,
        reason = "the end of `main` has nothing to do when the bound passes"
    )]
    pub fn wait(&self, bound: Duration) -> bool {
        let sent = self.queue.sent.load(Ordering::Acquire);
        if self.queue.resume().is_err() {
            return false;
        }
        self.written(sent, bound)
    }

    /// Logs ERROR `panicked` with the field `panic`, the text of `info`. For a panic
    /// hook: it can run inside an event, so it sets no dispatch and starts no
    /// thread. While a [`Hold`] lives, waits as [`Flush::wait`] does with a bound of
    /// 1 s, and returns `false` when the bound passed first: the writer may then
    /// block a write of the hook too. After each [`Hold`] dropped, returns `true` at
    /// once, and the line waits for the next [`Flush::wait`].
    #[must_use]
    pub fn panicked(&self, info: &PanicHookInfo<'_>) -> bool {
        self.queue.push(self.queue.line.panicked(info).into_bytes());
        if self.queue.closed.load(Ordering::Acquire) {
            return true;
        }
        self.written(self.queue.sent.load(Ordering::Acquire), PANIC)
    }

    /// The wait of [`Flush::wait`] for the first `sent` lines, with no start.
    fn written(&self, sent: u64, bound: Duration) -> bool {
        let queue = &self.queue;
        let busy = |state: &mut State| {
            state.written < sent
                || (queue.closed.load(Ordering::Acquire) && state.running)
        };
        #[expect(
            clippy::disallowed_methods,
            reason = "a stop and a panic hook cannot await"
        )]
        let (mut state, waited) = queue
            .written
            .wait_timeout_while(queue.lock(), bound, busy)
            .expect("invariant: nothing panics while it holds the log lock");
        if waited.timed_out() {
            return false;
        }
        if !state.running {
            for handle in state.handles.drain(..) {
                handle
                    .join()
                    .expect("invariant: the thread log ends with no panic");
            }
        }
        true
    }
}

/// Keeps the thread `log` running while a line can come: each shard holds a clone.
/// The drop of the last one lets the thread end once the queue is empty. A later line
/// waits for [`Flush::resume`] or [`Flush::wait`].
#[derive(Debug)]
pub(crate) struct Hold(Arc<Queue>);

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.wake();
    }
}

/// What each event, each [`Flush`], and the thread `log` share.
#[derive(Debug)]
pub(super) struct Queue {
    lines: SyncSender<Vec<u8>>,
    /// The waker of the thread `log` while it waits for a line.
    waker: Mutex<Option<Waker>>,
    /// The lines that dropped since the last count line.
    gap: Mutex<Option<Gap>>,
    /// The lines that went into the queue.
    sent: AtomicU64,
    /// No [`Hold`] is left, so the thread `log` ends when the queue is empty.
    closed: AtomicBool,
    state: Mutex<State>,
    /// Signals each change of `State::written` and `State::running`.
    written: Condvar,
    threads: env::threads::Threads,
    line: Line,
}

#[derive(Debug)]
struct State {
    /// The writer and the receiver while no thread `log` holds them.
    parts: Option<Parts>,
    /// A thread `log` runs or starts. When false, `parts` is there.
    running: bool,
    /// The lines of the queue that the thread `log` has written.
    written: u64,
    handles: Vec<Handle>,
}

/// Lines that dropped, and where their count line goes.
#[derive(Debug)]
struct Gap {
    /// The lines that went into the queue before the first of them.
    after: u64,
    lines: u64,
}

struct Parts {
    writer: Box<dyn Write + Send>,
    receiver: Receiver<Vec<u8>>,
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts").finish_non_exhaustive()
    }
}

impl Queue {
    /// Puts `line` in the queue, or counts it as dropped when the queue is full.
    fn push(&self, line: Vec<u8>) {
        if self.lines.try_send(line).is_err() {
            let mut gap = lock(&self.gap);
            let after = self.sent.load(Ordering::Acquire);
            gap.get_or_insert(Gap { after, lines: 0 }).lines += 1;
            return;
        }
        self.sent.fetch_add(1, Ordering::Release);
        self.wake();
    }

    fn wake(&self) {
        let waker = lock(&self.waker).take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Starts a thread `log` when none runs, and a [`Hold`] is left or a line waits.
    /// The thread takes the writer from the slot, so a start that fails keeps it.
    fn resume(self: &Arc<Self>) -> Result<(), Error> {
        let mut state = self.lock();
        let waiting = self.sent.load(Ordering::Acquire) > state.written;
        if state.running || (self.closed.load(Ordering::Acquire) && !waiting) {
            return Ok(());
        }
        let queue = Arc::clone(self);
        let drain = move || {
            let mut state = queue.lock();
            let (parts, written) = (state.parts.take(), state.written);
            drop(state);
            Drain {
                queue,
                parts,
                written,
            }
        };
        let handle = self.threads.start("log", drain)?;
        state.running = true;
        state.handles.push(handle);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

/// The writer of the dispatch: it puts each line in the queue.
#[derive(Debug)]
pub(super) struct Pipe(Arc<Queue>);

impl<'a> MakeWriter<'a> for Pipe {
    type Writer = &'a Pipe;

    fn make_writer(&'a self) -> &'a Pipe {
        self
    }
}

impl Write for &Pipe {
    /// `fmt` writes each line in one call.
    fn write(&mut self, line: &[u8]) -> io::Result<usize> {
        self.0.push(line.to_vec());
        Ok(line.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The future of the thread `log`: it writes each line of the queue, and ends once
/// the queue is empty and no [`Hold`] is left.
struct Drain {
    queue: Arc<Queue>,
    parts: Option<Parts>,
    /// [`State::written`], which this thread alone changes.
    written: u64,
}

impl Drain {
    /// Writes `line`, then the count line of the lines that dropped after it. A write
    /// that fails or panics drops its line.
    fn write(&mut self, line: Option<&[u8]>) {
        if let Some(line) = line {
            self.put(line);
            self.written += 1;
        }
        let written = self.written;
        let gap = lock(&self.queue.gap).take_if(|gap| gap.after <= written);
        if let Some(gap) = gap {
            let count = self.queue.line.dropped(gap.lines);
            self.put(count.as_bytes());
        }
        if line.is_some() {
            self.queue.lock().written = written;
            self.queue.written.notify_all();
        }
    }

    fn put(&mut self, line: &[u8]) {
        let parts = self
            .parts
            .as_mut()
            .expect("invariant: a drain has its parts");
        drop(panic::catch_unwind(AssertUnwindSafe(|| {
            parts.writer.write_all(line)
        })));
    }

    fn take(&self) -> Result<Vec<u8>, TryRecvError> {
        let parts = self
            .parts
            .as_ref()
            .expect("invariant: a drain has its parts");
        parts.receiver.try_recv()
    }
}

impl Future for Drain {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let drain = self.get_mut();
        loop {
            if let Ok(line) = drain.take() {
                drain.write(Some(&line));
                continue;
            }
            drain.write(None);
            if drain.queue.closed.load(Ordering::Acquire) {
                let mut state = drain.queue.lock();
                // A line that came before the lock must not wait for the next one.
                if let Ok(line) = drain.take() {
                    drop(state);
                    drain.write(Some(&line));
                    continue;
                }
                state.parts = drain.parts.take();
                state.running = false;
                drop(state);
                drain.queue.written.notify_all();
                return Poll::Ready(());
            }
            *lock(&drain.queue.waker) = Some(cx.waker().clone());
            // A line or a close before the waker went in woke nothing.
            if drain.queue.closed.load(Ordering::Acquire) {
                continue;
            }
            match drain.take() {
                Ok(line) => drain.write(Some(&line)),
                Err(_) => return Poll::Pending,
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .expect("invariant: nothing panics while it holds a log lock")
}
