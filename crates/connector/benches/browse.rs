//! The time of one `browse::walk` of a pattern over a device. Run with
//! `cargo bench -p connector --bench browse`.
//!
//! - `deep`: `**/a` 64 times, over a chain of 62 folders `a`, each folder with 149
//!   points `a`.
//! - `long`: `*`, 800 `a`, then `b`, over 2 000 points of 1 000 `a`.
//! - `pieces`: `*a` 256 times, over 9 999 points `p<i>`.
//! - `stars`: `**/` and a glob 63 times, then `x`, with 256 `*` in the globs, over
//!   a chain of 63 folders, each named 2 000 `a`, and points of that name in the last.

use std::hint::black_box;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use connector::browse::{
    self, Child, Cursor, DEPTH, Device, Entry, Facts, LIMIT, Page, Path, Query, STARS,
    Scope, Shape, Values,
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

/// A device whose each folder holds 149 points `a`, after the folder `a` when its
/// depth is under [`Chain::DEPTH`]. The tag of a folder is its depth.
struct Chain;

impl Chain {
    const DEPTH: usize = 62;
}

impl Device for Chain {
    fn level(
        &mut self,
        folder: Option<&Entry>,
        _: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        let depth: usize =
            folder.map_or(0, |folder| folder.tag.parse().expect("a depth"));
        let folder = (depth < Self::DEPTH).then(|| Child {
            name: "a".into(),
            tag: (depth + 1).to_string(),
            shape: Shape::Folder,
        });
        let points = (0..149).map(|i| Child {
            name: "a".into(),
            tag: format!("{depth}.{i}"),
            shape: Shape::Point(Facts::default()),
        });
        let entries = folder.into_iter().chain(points).collect();
        std::future::ready(Ok(Some(Page {
            entries,
            next: None,
        })))
    }
}

/// A device whose root starts a chain of [`Long::FOLDERS`] folders, each named
/// [`Long::name`], whose last holds points of that name with no end, in pages of
/// [`LIMIT`]. The tag of a folder is its depth.
struct Long;

impl Long {
    const FOLDERS: usize = DEPTH - 1;

    fn name() -> String {
        "a".repeat(2_000)
    }
}

impl Device for Long {
    fn level(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        let depth: usize =
            folder.map_or(0, |folder| folder.tag.parse().expect("a depth"));
        let page = if depth < Self::FOLDERS {
            Page {
                entries: vec![Child {
                    name: Self::name(),
                    tag: (depth + 1).to_string(),
                    shape: Shape::Folder,
                }],
                next: None,
            }
        } else {
            let start = cursor.map_or(0, |cursor| {
                cursor.as_str().parse().expect("a cursor of this device")
            });
            let end = start + LIMIT;
            Page {
                entries: (start..end)
                    .map(|i| Child {
                        name: Self::name(),
                        tag: format!("p{i}"),
                        shape: Shape::Point(Facts::default()),
                    })
                    .collect(),
                next: Some(Cursor::new(end.to_string())),
            }
        };
        std::future::ready(Ok(Some(page)))
    }
}

fn main() {
    let points: Vec<String> = (0..9_999).map(|i| format!("p{i}")).collect();
    let deep = vec!["**/a"; 64].join("/");
    let long = format!("*{}b", "a".repeat(800));
    let globs = (0..Long::FOLDERS).map(|at| {
        let stars = STARS / Long::FOLDERS + usize::from(at < STARS % Long::FOLDERS);
        format!("**/{}", "*a".repeat(stars))
    });
    let stars = format!("{}/x", globs.collect::<Vec<_>>().join("/"));
    let lines = [
        time("deep", &deep, &mut Chain),
        time(
            "long",
            &long,
            &mut Flat {
                names: vec!["a".repeat(1_000); 2_000],
            },
        ),
        time("pieces", &"*a".repeat(STARS), &mut Flat { names: points }),
        time("stars", &stars, &mut Long),
    ];
    print(&lines);
}

/// The µs of each timed walk of `pattern` over `device`.
#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn time(
    name: &'static str,
    pattern: &str,
    device: &mut impl Device,
) -> (&'static str, Vec<u64>) {
    let query = Query {
        start: Path::root(),
        scope: Scope::Match(pattern.parse().expect("a pattern")),
        values: Values::Skipped,
    };
    let mut micros = Vec::new();
    for round in 0..WARMUP + ROUNDS {
        let start = Instant::now();
        let mut walk = pin!(browse::walk(device, &query));
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
