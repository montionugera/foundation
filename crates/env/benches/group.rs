//! The cost of one poll of a task spawned on `Tasks`, through a `Group`, and through
//! a `Group` made on the `Tasks` of another. Run with `cargo bench -p env --bench
//! group`. A figure is the time of a run of polls over the polls, and each line gives
//! p10 and p50 over its runs.
//!
//! The task adds one to a counter and waits, so its `poll` can inline into the
//! wrapper of the group. The lines take turns, run by run. To compare two builds, run
//! each several times in turn on one pinned core and compare p10 and p50.

use std::cell::RefCell;
use std::hint::black_box;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use env::tasks::{Driver, Group, Task, Tasks};

const POLLS: u32 = 10_000_000;
const RUNS: usize = 31;

/// Keeps each task for the bench to poll.
#[derive(Clone, Default)]
struct Kept(Rc<RefCell<Vec<Task>>>);

impl Driver for Kept {
    fn spawn(&self, task: Task) {
        self.0.borrow_mut().push(task);
    }
}

struct Add(u64);

impl Future for Add {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        self.0 = black_box(self.0.wrapping_add(1));
        Poll::Pending
    }
}

#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn run(task: &mut Task) -> f64 {
    let mut cx = Context::from_waker(Waker::noop());
    let start = Instant::now();
    for _ in 0..POLLS {
        black_box(task.as_mut().poll(&mut cx)).is_ready();
    }
    start.elapsed().as_secs_f64() * 1e9 / f64::from(POLLS)
}

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    let kept = Kept::default();
    let tasks = Tasks::new(kept.clone());
    tasks.spawn(Add(0));
    let group = Group::new(tasks);
    group.tasks().spawn(Add(0));
    let nested = Group::new(group.tasks().clone());
    nested.tasks().spawn(Add(0));
    let mut spawned = kept.0.take();
    let mut times = [Vec::new(), Vec::new(), Vec::new()];
    for _ in 0..RUNS {
        for (task, line) in spawned.iter_mut().zip(&mut times) {
            line.push(run(task));
        }
    }
    println!("{:<28} {:>8} {:>8}", "ns per poll", "p10", "p50");
    for (name, line) in ["on Tasks", "through a Group", "through a group of a group"]
        .iter()
        .zip(&mut times)
    {
        line.sort_by(f64::total_cmp);
        println!(
            "{name:<28} {:>8.3} {:>8.3}",
            line[RUNS / 10],
            line[RUNS / 2]
        );
    }
}
