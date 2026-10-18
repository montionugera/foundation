//! A bound on the calls that hold a slot at one time, with a first come, first served
//! wait for a slot.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// Gives at most `max` slots at one time. A call over it waits, in the order the calls
/// come.
#[derive(Debug)]
pub(crate) struct Line {
    max: usize,
    /// The slots held.
    held: Cell<usize>,
    /// The key of the next call.
    keys: Cell<u64>,
    /// The calls that wait, first come first. Only the first can take a slot.
    waiting: RefCell<VecDeque<(u64, Waker)>>,
}

impl Line {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            held: Cell::new(0),
            keys: Cell::new(0),
            waiting: RefCell::new(VecDeque::new()),
        }
    }

    /// Ends when fewer than `max` slots are held and no call that came before waits,
    /// and gives a slot. The key of each slot is new in this line.
    pub(crate) fn take(&self) -> Take<'_> {
        let key = self.keys.get();
        self.keys.set(key + 1);
        Take {
            line: self,
            key,
            queued: false,
        }
    }

    /// Wakes the first call that waits, when a slot is free for it.
    fn wake_next(&self) {
        if self.held.get() == self.max {
            return;
        }
        let next = self
            .waiting
            .borrow()
            .front()
            .map(|(_, waker)| waker.clone());
        if let Some(next) = next {
            next.wake();
        }
    }
}

/// A slot of a [`Line`]. Its drop wakes the first call that waits.
#[derive(Debug)]
pub(crate) struct Slot<'a> {
    line: &'a Line,
    key: u64,
}

impl Slot<'_> {
    /// The key of the slot, which no other slot of its line has.
    pub(crate) fn key(&self) -> u64 {
        self.key
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.line.held.set(self.line.held.get() - 1);
        self.line.wake_next();
    }
}

/// A call that waits for a [`Slot`]. Its drop takes its place in the line out.
pub(crate) struct Take<'a> {
    line: &'a Line,
    key: u64,
    /// Whether the call is in [`Line::waiting`].
    queued: bool,
}

impl<'a> Future for Take<'a> {
    type Output = Slot<'a>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Slot<'a>> {
        let line = self.line;
        let mut waiting = line.waiting.borrow_mut();
        let first = waiting.front().is_none_or(|(key, _)| *key == self.key);
        if first && line.held.get() < line.max {
            if self.queued {
                // The waker drops after the borrow: its last drop can drop a task.
                let taken = waiting.pop_front();
                drop(waiting);
                drop(taken);
                self.queued = false;
            } else {
                drop(waiting);
            }
            line.held.set(line.held.get() + 1);
            line.wake_next();
            return Poll::Ready(Slot {
                line,
                key: self.key,
            });
        }
        let replaced = if self.queued {
            let (_, held) = waiting
                .iter_mut()
                .find(|(key, _)| *key == self.key)
                .expect("invariant: a queued call is in the line");
            (!held.will_wake(cx.waker()))
                .then(|| std::mem::replace(held, cx.waker().clone()))
        } else {
            waiting.push_back((self.key, cx.waker().clone()));
            None
        };
        drop(waiting);
        drop(replaced);
        self.queued = true;
        Poll::Pending
    }
}

impl Drop for Take<'_> {
    fn drop(&mut self) {
        if !self.queued {
            return;
        }
        let line = self.line;
        let mut waiting = line.waiting.borrow_mut();
        let at = waiting
            .iter()
            .position(|(key, _)| *key == self.key)
            .expect("invariant: a queued call is in the line");
        let removed = waiting.remove(at);
        drop(waiting);
        drop(removed);
        if at == 0 {
            line.wake_next();
        }
    }
}
