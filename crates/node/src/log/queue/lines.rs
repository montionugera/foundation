//! The lines between a node's events and the thread `log`, the run of that thread,
//! and its wake.

use std::collections::VecDeque;
use std::io::Write;
use std::mem;
use std::task::Waker;
use std::thread::{self, ThreadId};
use std::time::Duration;

#[cfg(loom)]
use loom::sync::{Condvar, Mutex, MutexGuard};
#[cfg(not(loom))]
use std::sync::{Condvar, Mutex, MutexGuard};

/// The lines that wait for the thread `log`, in the order they came, and the run of
/// that thread. Each change, each take, and each start and end of a run happen under
/// one lock, so a line or a close always wakes a take that waits, and a start always
/// sees a run that has ended.
#[derive(Debug)]
pub(super) struct Lines {
    inner: Mutex<Inner>,
    /// Signals each change of `Inner::written` and `Inner::running`.
    written: Condvar,
    /// The lines that can wait. A line that finds this many drops.
    bound: usize,
    /// The bytes that the lines that wait can hold. A line that does not fit drops.
    bytes: usize,
}

#[derive(Debug)]
struct Inner {
    lines: VecDeque<Vec<u8>>,
    /// The bytes of `lines`.
    bytes: usize,
    /// The panic lines, which never drop.
    panics: Vec<Vec<u8>>,
    /// The lines that dropped since the last take.
    dropped: u64,
    /// The lines that came, dropped ones too.
    sent: u64,
    closed: bool,
    /// The waker of the take that waits.
    waker: Option<Waker>,
    /// The writer while no thread `log` holds it.
    writer: Option<Writer>,
    /// A thread `log` runs or starts. When false, `writer` is there, other than after
    /// the writer panicked: then no thread starts again.
    running: bool,
    /// The thread `log` that holds `writer`.
    thread: Option<ThreadId>,
    /// The lines that the thread `log` has written or counted as dropped.
    written: u64,
}

impl Inner {
    /// Ends the run of the thread `log`, which gives back `writer`, or `None` after the
    /// writer panicked.
    fn end(&mut self, writer: Option<Writer>) {
        self.writer = writer;
        self.running = false;
        self.thread = None;
    }
}

/// The writer of a node's log.
pub(super) struct Writer(pub(super) Box<dyn Write + Send>);

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Writer")
    }
}

/// The lines of one take, in the order to write them: `lines`, then the count line
/// of `dropped` when it is not 0, then `panics`. Each line of `lines` came before
/// each line that dropped.
#[derive(Debug, Default)]
pub(super) struct Batch {
    pub(super) lines: VecDeque<Vec<u8>>,
    pub(super) dropped: u64,
    pub(super) panics: Vec<Vec<u8>>,
}

impl Batch {
    /// The lines of the take that came into `sent`, dropped ones too.
    pub(super) fn sent(&self) -> u64 {
        (self.lines.len() + self.panics.len()) as u64 + self.dropped
    }
}

/// What a take gives.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Taken {
    /// The batch holds lines or a count of lines that dropped.
    Lines,
    /// No line waits and none dropped. The next line, drop, or close wakes the waker
    /// of the take.
    Empty,
    /// No line waits, none dropped, and [`Lines::close`] came after the last
    /// [`Lines::open`]. The run of the thread `log` has ended.
    Closed,
}

impl Lines {
    /// Lines with no line, which hold up to `bound` lines and `bytes` bytes of lines,
    /// closed, and with `writer` for the first thread `log`.
    pub(super) fn new(bound: usize, bytes: usize, writer: Writer) -> Self {
        let inner = Inner {
            lines: VecDeque::with_capacity(bound),
            bytes: 0,
            panics: Vec::new(),
            dropped: 0,
            sent: 0,
            closed: true,
            waker: None,
            writer: Some(writer),
            running: false,
            thread: None,
            written: 0,
        };
        Self {
            inner: Mutex::new(inner),
            written: Condvar::new(),
            bound,
            bytes,
        }
    }

    /// An empty batch for [`Lines::take`], with room for `bound` lines, so neither
    /// queue of lines grows after it.
    pub(super) fn batch(&self) -> Batch {
        Batch {
            lines: VecDeque::with_capacity(self.bound),
            ..Batch::default()
        }
    }

    /// Puts `line` after the lines that wait, or counts it as dropped when `bound`
    /// lines wait or it does not fit in `bytes`. After a drop, each line drops until
    /// the next take, so the count line of the take marks one gap.
    pub(super) fn push(&self, line: Vec<u8>) {
        let mut inner = self.lock();
        let full =
            inner.lines.len() == self.bound || line.len() > self.bytes - inner.bytes;
        inner.sent += 1;
        if full || inner.dropped > 0 {
            inner.dropped += 1;
            wake(inner);
            return;
        }
        inner.bytes += line.len();
        inner.lines.push_back(line);
        wake(inner);
    }

    /// Puts the panic line `line` after each line that waits; it never drops. Gives
    /// the lines sent with it.
    pub(super) fn panic(&self, line: Vec<u8>) -> u64 {
        let mut inner = self.lock();
        inner.panics.push(line);
        inner.sent += 1;
        let sent = inner.sent;
        wake(inner);
        sent
    }

    /// Lets a take wait for a line after the queue is empty.
    pub(super) fn open(&self) {
        self.lock().closed = false;
    }

    /// Lets a take end once no line waits.
    pub(super) fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        wake(inner);
    }

    /// The lines that came, dropped ones too.
    pub(super) fn sent(&self) -> u64 {
        self.lock().sent
    }

    /// Starts a run of the thread `log` when none runs, the writer has not panicked,
    /// and the lines are open or a line waits. Gives `false` when it starts none.
    /// After `true`, the new thread calls [`Lines::begin`], or [`Lines::failed`]
    /// follows.
    pub(super) fn start(&self) -> bool {
        let mut inner = self.lock();
        let waiting = inner.sent > inner.written;
        if inner.running || inner.writer.is_none() || (inner.closed && !waiting) {
            return false;
        }
        inner.running = true;
        true
    }

    /// Ends the run of a [`Lines::start`] whose thread did not start. The writer stays.
    pub(super) fn failed(&self) {
        self.lock().running = false;
        self.written.notify_all();
    }

    /// Gives the writer to the calling thread, the thread `log` of the run.
    pub(super) fn begin(&self) -> Option<Writer> {
        let mut inner = self.lock();
        inner.thread = Some(thread::current().id());
        inner.writer.take()
    }

    /// Moves each line that waits and the count of the lines that dropped into
    /// `batch`, whose lines must be empty. When neither is there and the lines are not
    /// closed, keeps `waker` to wake at the next line, drop, or close. When the lines
    /// are closed, ends the run in the same step, which gives back `writer`.
    pub(super) fn take(
        &self,
        batch: &mut Batch,
        waker: &Waker,
        writer: &mut Option<Writer>,
    ) -> Taken {
        let mut inner = self.lock();
        if inner.lines.is_empty() && inner.panics.is_empty() && inner.dropped == 0 {
            if inner.closed {
                inner.end(writer.take());
                drop(inner);
                self.written.notify_all();
                return Taken::Closed;
            }
            inner.waker = Some(waker.clone());
            return Taken::Empty;
        }
        mem::swap(&mut inner.lines, &mut batch.lines);
        inner.bytes = 0;
        mem::swap(&mut inner.panics, &mut batch.panics);
        batch.dropped = mem::take(&mut inner.dropped);
        Taken::Lines
    }

    /// Counts `sent` more lines as written or dropped, and wakes each wait.
    pub(super) fn wrote(&self, sent: u64) {
        self.lock().written += sent;
        self.written.notify_all();
    }

    /// Ends the run with no writer, after the writer panicked: no thread starts again,
    /// and no wait waits for one.
    pub(super) fn end(&self) {
        self.lock().end(None);
        self.written.notify_all();
    }

    /// Whether a thread `log` other than the calling thread runs or starts.
    pub(super) fn runs_elsewhere(&self) -> bool {
        let inner = self.lock();
        inner.running && inner.thread != Some(thread::current().id())
    }

    /// Whether no thread `log` runs and the writer has not panicked.
    pub(super) fn idle(&self) -> bool {
        let inner = self.lock();
        !inner.running && inner.writer.is_some()
    }

    /// Blocks until the first `sent` lines are written and, while the lines are
    /// closed, the run has ended, or until `bound` passes or the writer panicked.
    /// Gives whether both happened first. Blocks on real time.
    pub(super) fn wait(&self, sent: u64, bound: Duration) -> bool {
        let busy = |inner: &mut Inner| {
            let live = inner.running || inner.writer.is_some();
            live && (inner.written < sent || (inner.closed && inner.running))
        };
        let (inner, late) = wait_while(&self.written, self.lock(), bound, busy);
        !late && inner.written >= sent
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .expect("invariant: nothing panics while it holds the lines lock")
    }
}

/// Waits on `written` while `busy`, up to `bound`. Gives whether `bound` passed.
#[cfg(not(loom))]
fn wait_while<'a>(
    written: &Condvar,
    inner: MutexGuard<'a, Inner>,
    bound: Duration,
    busy: impl FnMut(&mut Inner) -> bool,
) -> (MutexGuard<'a, Inner>, bool) {
    #[expect(
        clippy::disallowed_methods,
        reason = "a stop and a panic hook cannot await"
    )]
    let (inner, waited) = written
        .wait_timeout_while(inner, bound, busy)
        .expect("invariant: nothing panics while it holds the lines lock");
    (inner, waited.timed_out())
}

/// As `Condvar::wait_timeout_while` of `std`, with no bound: loom has no time.
#[cfg(loom)]
fn wait_while<'a>(
    written: &Condvar,
    mut inner: MutexGuard<'a, Inner>,
    _bound: Duration,
    mut busy: impl FnMut(&mut Inner) -> bool,
) -> (MutexGuard<'a, Inner>, bool) {
    while busy(&mut inner) {
        inner = written
            .wait(inner)
            .expect("invariant: nothing panics while it holds the lines lock");
    }
    (inner, false)
}

/// Wakes the take that waits, after the lock of `inner` drops.
fn wake(mut inner: MutexGuard<'_, Inner>) {
    let waker = inner.waker.take();
    drop(inner);
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// A writer that drops each line.
#[cfg(test)]
fn sink() -> Writer {
    Writer(Box::new(std::io::sink()))
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests {
    use std::task::Waker;

    use super::{Batch, Lines, Taken, sink};

    #[test]
    fn a_panic_line_counts_among_the_lines_sent() {
        let lines = Lines::new(1, 1, sink());
        lines.push(b"a".to_vec());
        lines.push(b"b".to_vec());
        assert_eq!((lines.panic(b"p".to_vec()), lines.sent()), (3, 3));
        let mut batch = Batch::default();
        assert_eq!(
            lines.take(&mut batch, Waker::noop(), &mut None),
            Taken::Lines
        );
        assert_eq!((batch.sent(), batch.dropped), (3, 1));
    }

    #[test]
    fn a_line_too_large_for_the_bytes_drops_with_each_line_until_the_take() {
        let lines = Lines::new(4, 2, sink());
        lines.push(b"aaa".to_vec());
        lines.push(b"b".to_vec());
        let mut batch = Batch::default();
        assert_eq!(
            lines.take(&mut batch, Waker::noop(), &mut None),
            Taken::Lines
        );
        assert_eq!((batch.sent(), batch.dropped), (2, 2));
        for line in [b"cc", b"dd"] {
            lines.push(line.to_vec());
            assert_eq!(
                lines.take(&mut batch, Waker::noop(), &mut None),
                Taken::Lines
            );
            assert_eq!(batch.lines.drain(..).collect::<Vec<_>>(), [line.to_vec()]);
            assert_eq!(batch.dropped, 0);
        }
    }

    #[test]
    fn a_batch_has_room_for_the_bound() {
        assert!(Lines::new(4, 4, sink()).batch().lines.capacity() >= 4);
    }

    #[test]
    fn a_run_whose_thread_did_not_start_leaves_the_lines_idle_for_the_next_start() {
        let lines = Lines::new(4, 4, sink());
        lines.open();
        assert!(lines.start());
        assert!(!lines.idle());
        lines.failed();
        assert!(lines.idle());
        assert!(lines.start());
    }
}

#[cfg(test)]
#[cfg(loom)]
mod model {
    use std::future::poll_fn;
    use std::task::Poll;

    use loom::future::block_on;
    use loom::sync::Arc;
    use loom::thread;

    use super::{Batch, Lines, Taken, Writer, sink};

    /// Takes with `writer` until the lines close, as the thread `log` does, and gives
    /// each line and the count of each line that dropped.
    fn drain(lines: &Lines, writer: &mut Option<Writer>) -> (Vec<Vec<u8>>, u64) {
        let mut batch = Batch::default();
        let (mut written, mut dropped) = (Vec::new(), 0);
        block_on(poll_fn(|cx| {
            loop {
                match lines.take(&mut batch, cx.waker(), writer) {
                    Taken::Lines => {
                        let sent = batch.sent();
                        written.extend(batch.lines.drain(..));
                        dropped += batch.dropped;
                        written.append(&mut batch.panics);
                        lines.wrote(sent);
                    }
                    Taken::Empty => return Poll::Pending,
                    Taken::Closed => return Poll::Ready(()),
                }
            }
        }));
        (written, dropped)
    }

    /// Lines from two threads and a close after them each reach a take that waits on
    /// a third.
    #[test]
    fn each_line_before_the_close_reaches_a_take_that_waits() {
        loom::model(|| {
            let lines = Arc::new(Lines::new(4, 4, sink()));
            lines.open();
            let push = |line: &'static [u8]| {
                let lines = Arc::clone(&lines);
                thread::spawn(move || lines.push(line.to_vec()))
            };
            let pushes = [push(b"a"), push(b"b")];
            let taker = {
                let lines = Arc::clone(&lines);
                thread::spawn(move || drain(&lines, &mut None))
            };
            for push in pushes {
                push.join().unwrap();
            }
            lines.close();
            let (mut written, dropped) = taker.join().unwrap();
            written.sort();
            assert_eq!((written, dropped), (vec![b"a".to_vec(), b"b".to_vec()], 0));
        });
    }

    /// A line that finds the bound drops and counts, and a panic line that finds it
    /// comes after each line before it.
    #[test]
    fn a_full_queue_drops_a_line_but_never_a_panic_line() {
        loom::model(|| {
            let lines = Arc::new(Lines::new(1, 1, sink()));
            lines.open();
            let taker = {
                let lines = Arc::clone(&lines);
                thread::spawn(move || drain(&lines, &mut None))
            };
            lines.push(b"a".to_vec());
            lines.push(b"b".to_vec());
            assert_eq!(lines.panic(b"p".to_vec()), lines.sent());
            lines.close();
            let (written, dropped) = taker.join().unwrap();
            assert_eq!(written.len() as u64 + dropped, 3);
            assert_eq!(written.first(), Some(&b"a".to_vec()));
            assert_eq!(written.last(), Some(&b"p".to_vec()));
        });
    }

    /// A line that comes as the run of the thread `log` ends gets a run that writes
    /// it: the run that ends takes it, or the start after the line finds that run
    /// ended.
    #[test]
    fn a_line_as_the_run_ends_gets_a_run_that_writes_it() {
        loom::model(|| {
            let lines = Arc::new(Lines::new(4, 4, sink()));
            lines.open();
            assert!(lines.start());
            let run = {
                let lines = Arc::clone(&lines);
                thread::spawn(move || drain(&lines, &mut lines.begin()).0)
            };
            let late = {
                let lines = Arc::clone(&lines);
                thread::spawn(move || {
                    lines.push(b"a".to_vec());
                    if lines.start() {
                        drain(&lines, &mut lines.begin()).0
                    } else {
                        Vec::new()
                    }
                })
            };
            lines.close();
            let mut written = run.join().unwrap();
            written.extend(late.join().unwrap());
            assert_eq!(written, [b"a".to_vec()]);
            assert!(lines.idle());
        });
    }
}
