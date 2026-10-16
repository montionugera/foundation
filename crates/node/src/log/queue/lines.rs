//! The lines between a node's events and the thread `log`, and the wake of that thread.

use std::collections::VecDeque;
use std::mem;
use std::task::Waker;

#[cfg(loom)]
use loom::sync::{Mutex, MutexGuard};
#[cfg(not(loom))]
use std::sync::{Mutex, MutexGuard};

/// The lines that wait for the thread `log`, in the order they came. Each change and
/// each take happen under one lock, so a line or a close always wakes a take that
/// waits.
#[derive(Debug)]
pub(super) struct Lines {
    inner: Mutex<Inner>,
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
    /// The lines that went into `lines` or `panics`.
    sent: u64,
    closed: bool,
    /// The waker of the take that waits.
    waker: Option<Waker>,
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
    /// The lines of the take that came into `sent`.
    pub(super) fn sent(&self) -> u64 {
        (self.lines.len() + self.panics.len()) as u64
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
    /// [`Lines::open`].
    Closed,
}

impl Lines {
    /// Lines with no line, which hold up to `bound` lines and `bytes` bytes of lines,
    /// and closed.
    pub(super) fn new(bound: usize, bytes: usize) -> Self {
        let inner = Inner {
            lines: VecDeque::with_capacity(bound),
            bytes: 0,
            panics: Vec::new(),
            dropped: 0,
            sent: 0,
            closed: true,
            waker: None,
        };
        Self {
            inner: Mutex::new(inner),
            bound,
            bytes,
        }
    }

    /// Puts `line` after the lines that wait, or counts it as dropped when `bound`
    /// lines wait or it does not fit in `bytes`. After a drop, each line drops until
    /// the next take, so the count line of the take marks one gap.
    pub(super) fn push(&self, line: Vec<u8>) {
        let mut inner = self.lock();
        let full =
            inner.lines.len() == self.bound || line.len() > self.bytes - inner.bytes;
        if full || inner.dropped > 0 {
            inner.dropped += 1;
            wake(inner);
            return;
        }
        inner.bytes += line.len();
        inner.lines.push_back(line);
        inner.sent += 1;
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

    pub(super) fn closed(&self) -> bool {
        self.lock().closed
    }

    /// The lines that went in, other than each line that dropped.
    pub(super) fn sent(&self) -> u64 {
        self.lock().sent
    }

    /// Moves each line that waits and the count of the lines that dropped into
    /// `batch`, whose lines must be empty. When neither is there and the lines are not
    /// closed, keeps `waker` to wake at the next line, drop, or close.
    pub(super) fn take(&self, batch: &mut Batch, waker: &Waker) -> Taken {
        let mut inner = self.lock();
        if inner.lines.is_empty() && inner.panics.is_empty() && inner.dropped == 0 {
            if inner.closed {
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

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .expect("invariant: nothing panics while it holds the lines lock")
    }
}

/// Wakes the take that waits, after the lock of `inner` drops.
fn wake(mut inner: MutexGuard<'_, Inner>) {
    let waker = inner.waker.take();
    drop(inner);
    if let Some(waker) = waker {
        waker.wake();
    }
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests {
    use std::task::Waker;

    use super::{Batch, Lines, Taken};

    #[test]
    fn a_panic_line_counts_among_the_lines_sent() {
        let lines = Lines::new(1, 1);
        lines.push(b"a".to_vec());
        lines.push(b"b".to_vec());
        assert_eq!((lines.panic(b"p".to_vec()), lines.sent()), (2, 2));
        let mut batch = Batch::default();
        assert_eq!(lines.take(&mut batch, Waker::noop()), Taken::Lines);
        assert_eq!((batch.sent(), batch.dropped), (2, 1));
    }

    #[test]
    fn a_line_too_large_for_the_bytes_drops_with_each_line_until_the_take() {
        let lines = Lines::new(4, 2);
        lines.push(b"aaa".to_vec());
        lines.push(b"b".to_vec());
        let mut batch = Batch::default();
        assert_eq!(lines.take(&mut batch, Waker::noop()), Taken::Lines);
        assert_eq!((batch.sent(), batch.dropped), (0, 2));
        for line in [b"cc", b"dd"] {
            lines.push(line.to_vec());
            assert_eq!(lines.take(&mut batch, Waker::noop()), Taken::Lines);
            assert_eq!(batch.lines.drain(..).collect::<Vec<_>>(), [line.to_vec()]);
            assert_eq!(batch.dropped, 0);
        }
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

    use super::{Batch, Lines, Taken};

    /// Takes until the lines close, and gives each line and the count of each line
    /// that dropped.
    fn drain(lines: &Lines) -> (Vec<Vec<u8>>, u64) {
        let mut batch = Batch::default();
        let (mut written, mut dropped) = (Vec::new(), 0);
        block_on(poll_fn(|cx| {
            loop {
                match lines.take(&mut batch, cx.waker()) {
                    Taken::Lines => {
                        written.extend(batch.lines.drain(..));
                        dropped += batch.dropped;
                        written.append(&mut batch.panics);
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
            let lines = Arc::new(Lines::new(4, 4));
            lines.open();
            let push = |line: &'static [u8]| {
                let lines = Arc::clone(&lines);
                thread::spawn(move || lines.push(line.to_vec()))
            };
            let pushes = [push(b"a"), push(b"b")];
            let taker = {
                let lines = Arc::clone(&lines);
                thread::spawn(move || drain(&lines))
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
            let lines = Arc::new(Lines::new(1, 1));
            lines.open();
            let taker = {
                let lines = Arc::clone(&lines);
                thread::spawn(move || drain(&lines))
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
}
