//! A walk to a page of [`LIMIT`] points under a chain of folders with long names
//! holds the text of each name once. This binary has no test harness: the count
//! covers each thread, and a harness allocates on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use connector::browse::{
    self, Cursor, DEPTH, Device, End, Entry, Facts, LIMIT, Page, Path, Query, Scope,
    Shape, Values,
};
use connector::kind::Error;

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The bytes of each name of the chain.
const NAME: usize = 1 << 10;

/// The folders of the chain, so a walk from the root holds [`DEPTH`] folders open.
const FOLDERS: usize = DEPTH - 1;

/// A device whose root holds a chain of [`FOLDERS`] folders, each with a name of
/// [`NAME`] bytes, and whose last folder holds [`LIMIT`] points.
struct Chain;

impl Device for Chain {
    fn level(
        &mut self,
        folder: Option<&Entry>,
        _: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        let at = folder.map_or_else(Path::root, |folder| folder.path.clone());
        let depth = at.names().len();
        let entries = if depth < FOLDERS {
            vec![Entry {
                path: at.child("n".repeat(NAME)),
                tag: depth.to_string(),
                shape: Shape::Folder,
            }]
        } else {
            (0..LIMIT)
                .map(|i| Entry {
                    path: at.child(format!("p{i}")),
                    tag: format!("{depth}/{i}"),
                    shape: Shape::Point(Facts::default()),
                })
                .collect()
        };
        std::future::ready(Ok(Some(Page {
            entries,
            next: None,
        })))
    }
}

fn main() {
    let query = Query {
        start: Path::root(),
        scope: Scope::Match("**".parse().expect("a pattern")),
        values: Values::Skipped,
    };
    let before = ALLOCATOR.held();
    ALLOCATOR.reset_peak();
    let mut chain = Chain;
    let mut walk = pin!(browse::walk(&mut chain, &query));
    let Poll::Ready(found) =
        walk.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("the walk waits on a device that never waits");
    };
    let found = found.expect("found");
    let peak = ALLOCATOR.peak() - before;
    assert_eq!(
        (found.entries.len(), found.end),
        (LIMIT, End::Done),
        "the walk finds each point"
    );
    // The names of the chain, once, and a pointer to each name of each point: 1.3 MB
    // on box1. A copy of the chain's text in each point is 37 MB.
    let bound = 2 << 20;
    assert!(
        peak <= bound,
        "the walk held {peak} bytes at its peak, over {bound}"
    );
}
