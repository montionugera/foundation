//! The time of one `browse::walk` of a pattern over a flat device that gives pages of
//! [`LIMIT`] points. Run with `cargo bench -p connector --bench browse`.
//!
//! - `deep`: `**/a` 64 times, over 9 999 points `p<i>`.
//! - `long`: `*`, 800 `a`, then `b`, over 2 000 points of 1 000 `a`.
//! - `pieces`: `*a` 500 000 times (1 MB), over 9 999 points `p<i>`.

use std::hint::black_box;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use connector::browse::{
    self, Child, Cursor, Device, Entry, Facts, LIMIT, Page, Path, Query, Scope, Shape,
    Values,
};
use connector::kind::Error;

const WARMUP: usize = 2;
const ROUNDS: usize = 20;

/// A device whose root holds one point of each name, in pages of [`LIMIT`], with the
/// offset of the next page as its cursor.
struct Flat {
    names: Vec<String>,
}

impl Device for Flat {
    fn level(
        &mut self,
        _: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        let start = cursor.map_or(0, |cursor| {
            cursor.as_str().parse().expect("a cursor of this device")
        });
        let end = (start + LIMIT).min(self.names.len());
        let entries = self.names[start..end]
            .iter()
            .map(|name| Child {
                name: name.clone(),
                tag: name.clone(),
                shape: Shape::Point(Facts::default()),
            })
            .collect();
        let next = (end < self.names.len()).then(|| Cursor::new(end.to_string()));
        std::future::ready(Ok(Some(Page { entries, next })))
    }
}

fn main() {
    let points: Vec<String> = (0..9_999).map(|i| format!("p{i}")).collect();
    let deep = vec!["**/a"; 64].join("/");
    let long = format!("*{}b", "a".repeat(800));
    let lines = [
        time("deep", &deep, points.clone()),
        time("long", &long, vec!["a".repeat(1_000); 2_000]),
        time("pieces", &"*a".repeat(500_000), points),
    ];
    print(&lines);
}

/// The µs of each timed walk of `pattern` over the points `names`.
#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn time(
    name: &'static str,
    pattern: &str,
    names: Vec<String>,
) -> (&'static str, Vec<u64>) {
    let query = Query {
        start: Path::root(),
        scope: Scope::Match(pattern.parse().expect("a pattern")),
        values: Values::Skipped,
    };
    let mut device = Flat { names };
    let mut micros = Vec::new();
    for round in 0..WARMUP + ROUNDS {
        let start = Instant::now();
        let mut walk = pin!(browse::walk(&mut device, &query));
        let Poll::Ready(found) =
            walk.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("the walk waits on a device that never waits");
        };
        black_box(found.expect("found"));
        let took = Instant::now().duration_since(start).as_micros();
        if round >= WARMUP {
            micros.push(u64::try_from(took).expect("a walk takes under 2^64 µs"));
        }
    }
    (name, micros)
}

/// Prints p10, p50, and p90 of the µs of each line.
#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn print(lines: &[(&'static str, Vec<u64>)]) {
    println!("µs per walk over {ROUNDS} walks");
    println!("{:<8} {:>9} {:>9} {:>9}", "line", "p10", "p50", "p90");
    for (name, micros) in lines {
        let mut micros = micros.clone();
        micros.sort_unstable();
        let at = |percent: usize| micros[micros.len() * percent / 100];
        println!("{name:<8} {:>9} {:>9} {:>9}", at(10), at(50), at(90));
    }
}
