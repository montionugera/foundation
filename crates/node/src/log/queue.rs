//! The queue of a node's log and the thread `log` that writes it, so no event waits on
//! the writer.

mod lines;

use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::thread::{self, ThreadId};
use std::time::Duration;

use env::thread::{Error, Handle};
use tracing::Dispatch;
use tracing_subscriber::fmt::MakeWriter;

use self::lines::{Batch, Lines, Taken};
use super::Line;

/// The lines that the queue holds. A line that finds it full drops.
const LINES: usize = 1024;

/// The bytes of the lines that the queue holds. A line that does not fit drops.
const BYTES: usize = 1 << 20;

/// A panic hook of the process, as [`std::panic::set_hook`] takes it.
type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

/// Waits for the lines of a node's log to reach its writer. Clones wait on the same
/// log.
#[derive(Clone, Debug)]
pub struct Flush {
    queue: Arc<Queue>,
    /// The node's dispatch, whose events go into `queue`.
    pub(crate) dispatch: Dispatch,
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
        let queue = Arc::new(Queue {
            lines: Lines::new(LINES, BYTES),
            state: Mutex::new(State {
                writer: Some(Writer(writer)),
                running: false,
                thread: None,
                written: 0,
                handle: None,
            }),
            written: Condvar::new(),
            threads: threads.logged(Dispatch::none()),
            line: line.clone(),
        });
        let dispatch = super::dispatch(line, level, Pipe(Arc::clone(&queue)));
        Self { queue, dispatch }
    }

    /// Starts the thread `log`, which runs until the [`Hold`] drops and the queue is
    /// empty.
    ///
    /// # Errors
    ///
    /// [`Error::Start`] when the thread cannot start. The log then has no hold.
    pub(crate) fn hold(&self) -> Result<Hold, Error> {
        self.queue.lines.open();
        let hold = Hold(Arc::clone(&self.queue));
        self.queue.resume()?;
        Ok(hold)
    }

    /// Starts the thread `log` when none runs, so the lines that came after each
    /// [`Hold`] dropped reach the writer. Returns `false` when the thread could not
    /// start: the lines then wait for the next call. Call it outside an event: an OS
    /// thread cannot start inside one.
    pub(crate) fn resume(&self) -> bool {
        self.queue.resume().is_ok()
    }

    /// Starts the thread `log` as [`Flush::resume`] does, then blocks until the writer
    /// has written each line that was in the queue at the call, and the count line of
    /// each line that had dropped, or until `bound` passes. Once each shard has ended,
    /// also blocks until the thread `log` has ended, and joins it. Returns `false` when
    /// `bound` passed first or the thread could not start. Blocks on real time: for
    /// `main` only, never in sim.
    ///
    /// # Panics
    ///
    /// When the thread `log` panicked after it ended its run, which is a defect.
    #[expect(
        clippy::must_use_candidate,
        reason = "the end of `main` has nothing to do when the bound passes"
    )]
    pub fn wait(&self, bound: Duration) -> bool {
        let sent = self.queue.lines.sent();
        if !self.resume() {
            return false;
        }
        self.written(sent, bound)
    }

    /// The panic hook for `main` to set, over `previous`, the hook before it. It
    /// logs ERROR `panicked` with the field `panic`, the text of the panic, after each
    /// line in the queue; this line never drops. Then it calls `previous`. It sets no
    /// dispatch and starts no thread, so a panic inside an event does not abort. While
    /// a thread `log` runs, it first waits up to `bound` for the log to write each
    /// line, and when `bound` passes, it does not call `previous`: the writer may block
    /// that write too. When no thread `log` runs, as after each shard has ended, and
    /// for a panic of the writer, it calls `previous` at once.
    #[must_use]
    pub fn hook(&self, previous: Hook, bound: Duration) -> Hook {
        let flush = self.clone();
        Box::new(move |info| {
            if flush.panicked(info, bound) {
                previous(info);
            }
        })
    }

    /// Logs the panic line of `info`, then gives whether the hook before may write:
    /// after a wait of up to `bound` while another thread `log` runs, and at once
    /// otherwise.
    fn panicked(&self, info: &PanicHookInfo<'_>, bound: Duration) -> bool {
        let queue = &self.queue;
        let sent = queue.lines.panic(queue.line.panicked(info).into_bytes());
        let state = lock(&queue.state);
        if !state.running || state.thread == Some(thread::current().id()) {
            return true;
        }
        drop(state);
        self.written(sent, bound)
    }

    /// The wait of [`Flush::wait`] for the first `sent` lines, with no start.
    fn written(&self, sent: u64, bound: Duration) -> bool {
        let queue = &self.queue;
        let busy = |state: &mut State| {
            let live = state.running || state.writer.is_some();
            live && (state.written < sent || (queue.lines.closed() && state.running))
        };
        #[expect(
            clippy::disallowed_methods,
            reason = "a stop and a panic hook cannot await"
        )]
        let (mut state, waited) = queue
            .written
            .wait_timeout_while(lock(&queue.state), bound, busy)
            .expect("invariant: nothing panics while it holds the log lock");
        if waited.timed_out() || state.written < sent {
            return false;
        }
        let ended = (!state.running).then(|| state.ended()).flatten();
        drop(state);
        if let Some(ended) = ended {
            join(ended);
        }
        true
    }
}

/// Keeps the thread `log` running while a line can come: each shard holds an `Arc` of
/// it. The drop lets the thread end once the queue is empty. A later line waits for
/// [`Flush::resume`] or [`Flush::wait`].
#[derive(Debug)]
pub(crate) struct Hold(Arc<Queue>);

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.lines.close();
    }
}

/// What each event, each [`Flush`], and the thread `log` share.
#[derive(Debug)]
pub(super) struct Queue {
    /// Closed while no [`Hold`] is left, so the thread `log` ends when no line waits.
    lines: Lines,
    /// Taken before the lock of `lines` when a thread takes both.
    state: Mutex<State>,
    /// Signals each change of `State::written` and `State::running`.
    written: Condvar,
    threads: env::threads::Threads,
    line: Line,
}

#[derive(Debug)]
struct State {
    /// The writer while no thread `log` holds it.
    writer: Option<Writer>,
    /// A thread `log` runs or starts. When false, `writer` is there, other than after
    /// the writer panicked: then no thread starts again.
    running: bool,
    /// The thread `log` that holds `writer`.
    thread: Option<ThreadId>,
    /// The lines of the queue that the thread `log` has written or counted as dropped.
    written: u64,
    /// The thread `log` that runs or ran last.
    handle: Option<Handle>,
}

impl State {
    /// Ends the run of the thread `log`, which gives back `writer`, or `None` after the
    /// writer panicked.
    fn end(&mut self, writer: Option<Writer>) {
        self.writer = writer;
        self.running = false;
        self.thread = None;
    }

    /// The thread `log` that ran last, to join once the lock drops, other than after
    /// the writer panicked: that panic has reached the panic hook.
    fn ended(&mut self) -> Option<Handle> {
        self.writer.as_ref().and(self.handle.take())
    }
}

/// Joins `handle`, a thread `log` that has ended its run.
///
/// # Panics
///
/// When it panicked after it ended its run, which is a defect.
fn join(handle: Handle) {
    handle
        .join()
        .expect("invariant: the thread log ends with no panic");
}

struct Writer(Box<dyn Write + Send>);

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Writer")
    }
}

impl Queue {
    /// Starts a thread `log` when none runs, the writer has not panicked, and a
    /// [`Hold`] is left or a line waits. The thread takes the writer from the slot, so
    /// a start that fails keeps it.
    fn resume(self: &Arc<Self>) -> Result<(), Error> {
        let mut state = lock(&self.state);
        let waiting = self.lines.sent() > state.written;
        if state.running || state.writer.is_none() || (self.lines.closed() && !waiting)
        {
            return Ok(());
        }
        let ended = state.ended();
        let queue = Arc::clone(self);
        let drain = move || {
            let mut state = lock(&queue.state);
            let (writer, written) = (state.writer.take(), state.written);
            state.thread = Some(thread::current().id());
            drop(state);
            Drain {
                queue,
                writer,
                written,
                batch: Batch::default(),
            }
        };
        state.handle = Some(self.threads.start("log", drain)?);
        state.running = true;
        drop(state);
        if let Some(ended) = ended {
            join(ended);
        }
        Ok(())
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
        self.0.lines.push(line.to_vec());
        Ok(line.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The future of the thread `log`: it writes each line that waits, and ends once no
/// line waits and no [`Hold`] is left.
struct Drain {
    queue: Arc<Queue>,
    writer: Option<Writer>,
    /// [`State::written`], which this thread alone changes.
    written: u64,
    batch: Batch,
}

impl Drain {
    /// Writes the lines of the batch. A write that fails drops its line.
    fn write(&mut self) {
        let Writer(writer) = self
            .writer
            .as_mut()
            .expect("invariant: a drain has its writer");
        let batch = &mut self.batch;
        let sent = batch.sent();
        for line in batch.lines.drain(..) {
            drop(writer.write_all(&line));
        }
        if batch.dropped > 0 {
            let count = self.queue.line.dropped(batch.dropped);
            drop(writer.write_all(count.as_bytes()));
        }
        for line in batch.panics.drain(..) {
            drop(writer.write_all(&line));
        }
        self.written += sent;
        lock(&self.queue.state).written = self.written;
        self.queue.written.notify_all();
    }
}

impl Drop for Drain {
    /// After a panic of the writer, ends the run with no writer, so a panic hook does
    /// not wait for a thread that writes no more lines.
    fn drop(&mut self) {
        if self.writer.take().is_some() {
            lock(&self.queue.state).end(None);
            self.queue.written.notify_all();
        }
    }
}

impl Future for Drain {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let drain = self.get_mut();
        loop {
            match drain.queue.lines.take(&mut drain.batch, cx.waker()) {
                Taken::Lines => drain.write(),
                Taken::Empty => return Poll::Pending,
                Taken::Closed => {
                    let queue = Arc::clone(&drain.queue);
                    let mut state = lock(&queue.state);
                    // Under the lock, so a start sees this thread run or end.
                    match queue.lines.take(&mut drain.batch, cx.waker()) {
                        Taken::Lines => {
                            drop(state);
                            drain.write();
                        }
                        Taken::Empty => return Poll::Pending,
                        Taken::Closed => {
                            state.end(drain.writer.take());
                            drop(state);
                            queue.written.notify_all();
                            return Poll::Ready(());
                        }
                    }
                }
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .expect("invariant: nothing panics while it holds a log lock")
}
