//! A walk to a page of [`LIMIT`] points under a chain of folders with long names
//! holds the text of each name once, and a walk from a deep start holds no more than
//! one from the root. This binary has no test harness: the count covers each thread,
//! and a harness allocates on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use connector::browse::{
    self, Child, Cursor, DEPTH, Device, End, Entry, Facts, LIMIT, Page, Path, Query,
    Scope, Shape, Values,
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
            vec![Child {
                name: "n".repeat(NAME),
                tag: depth.to_string(),
                shape: Shape::Folder,
            }]
        } else {
            (0..LIMIT)
                .map(|i| Child {
                    name: format!("p{i}"),
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

/// The folders above the start of [`Wide`], more than [`DEPTH`].
const START: usize = 200;

/// A device whose root holds a chain of `start` folders, each with the name `s`, then
/// a chain of [`DEPTH`] folders, each of which holds the folder `f` and then points
/// to a page of [`LIMIT`] entries. The last holds [`LIMIT`] points.
struct Wide {
    start: usize,
}

impl Device for Wide {
    fn level(
        &mut self,
        folder: Option<&Entry>,
        _: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>> {
        let depth = folder.map_or(0, |folder| folder.path.names().len());
        let point = |i: usize| Child {
            name: format!("p{i}"),
            tag: format!("{depth}/{i}"),
            shape: Shape::Point(Facts::default()),
        };
        let folder = |name: &str| Child {
            name: name.into(),
            tag: depth.to_string(),
            shape: Shape::Folder,
        };
        let entries = if depth < self.start {
            vec![folder("s")]
        } else if depth < self.start + DEPTH - 1 {
            std::iter::once(folder("f"))
                .chain((1..LIMIT).map(point))
                .collect()
        } else {
            (0..LIMIT).map(point).collect()
        };
        std::future::ready(Ok(Some(Page {
            entries,
            next: None,
        })))
    }
}

/// What `device` gives for `query`, and the most bytes that the walk held.
fn walk(device: &mut impl Device, query: &Query) -> (browse::Found, usize) {
    let before = ALLOCATOR.held();
    ALLOCATOR.reset_peak();
    let mut walk = pin!(browse::walk(device, query));
    let Poll::Ready(found) =
        walk.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("the walk waits on a device that never waits");
    };
    (found.expect("found"), ALLOCATOR.peak() - before)
}

fn main() {
    let query = Query {
        start: Path::root(),
        scope: Scope::Match("**".parse().expect("a pattern")),
        values: Values::Skipped,
    };
    let (found, peak) = walk(&mut Chain, &query);
    assert_eq!(found.entries.len(), LIMIT, "the walk finds each point");
    assert_eq!(found.end, End::Done, "the walk finds each point");
    drop(found);
    // The names of the chain, once, and a link for each point: 0.2 MB on box1. A copy
    // of the chain's text in each point is 36 MB.
    let bound = 1 << 20;
    assert!(
        peak <= bound,
        "the walk held {peak} bytes at its peak, over {bound}"
    );

    let (found, root) = walk(&mut Wide { start: 0 }, &query);
    assert_eq!(
        found.end,
        End::Limit,
        "the walk from the root ends at the limit"
    );
    drop(found);
    let deep = Query {
        start: vec!["s"; START].join("/").parse().expect("a path"),
        ..query
    };
    // The links of the start, once: 8 KB more than from the root on ip-172-31-11-54. A
    // copy of the start's names in each entry of the open pages is 102 MB more.
    let (found, peak) = walk(&mut Wide { start: START }, &deep);
    assert_eq!(
        found.end,
        End::Limit,
        "the walk from the start ends at the limit"
    );
    drop(found);
    let bound = root + (1 << 16);
    assert!(
        peak <= bound,
        "the walk from the start held {peak} bytes at its peak, over {bound}"
    );
}
