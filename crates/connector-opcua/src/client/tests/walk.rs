use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use connector::browse::{
    self, Access, Cursor, Device, End, Entry, Facts, Found, Page, Query, Scope, Shape,
    Values,
};
use connector::kind::Error;
use types::sample::{self, Scalar, Type};

use super::{RELAY, Relay, relayed, run, serve};
use crate::browse::Server as Browse;
use crate::ffi::Status;
use crate::node::ns0::{self, HAS_COMPONENT, ORGANIZES};
use crate::node::{Key, Name, Node};
use crate::server::{self, Server, Variable};
use crate::value::Value;

const BROWSE_RESPONSE: [u8; 4] = [0x01, 0x00, 0x12, 0x02];
const BROWSE_NEXT_RESPONSE: [u8; 4] = [0x01, 0x00, 0x18, 0x02];
const OBJECTS: Node = Node::ns0(ns0::OBJECTS);
const BASE_DATA_TYPE: Node = Node::ns0(ns0::BASE_DATA_TYPE);
const DOUBLE: Node = Node::ns0(ns0::DOUBLE);
const ROOT: Node = Node {
    namespace: 1,
    key: Key::Number(7000),
};

fn named(name: &str) -> Node {
    Node {
        namespace: 1,
        key: Key::Name(Name::new(name).expect("a name")),
    }
}

/// The `Browse` or `BrowseNext` response of `message`, else `None`.
fn response(message: &[u8]) -> Option<[u8; 4]> {
    let kind = message.get(24..28)?;
    let browse = kind == BROWSE_RESPONSE || kind == BROWSE_NEXT_RESPONSE;
    (message.starts_with(b"MSG") && browse).then(|| kind.try_into().expect("4 bytes"))
}

/// Gives the output of `body` with a browse of a session through a relay that gives
/// each server message to `rewrite`, after `setup`.
fn relayed_browse<T: Send + 'static>(
    setup: impl FnOnce(&Server) + Send + 'static,
    rewrite: impl FnMut(&mut Vec<u8>) + Send + 'static,
    body: impl AsyncFnOnce(&mut Browse<'_>) -> T + Send + 'static,
) -> T {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            setup(peer.server);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            relayed(&node, &Relay::default(), rewrite, async {
                let mut client = peer.connect(&url).await.expect("a connect");
                let out = body(&mut Browse::new(&mut client)).await;
                client.close().await;
                out
            })
            .await
        })
        .await
    });
    out
}

/// What a walk of `query` gives, as [`relayed_browse`] runs it.
fn walked(
    setup: impl FnOnce(&Server) + Send + 'static,
    rewrite: impl FnMut(&mut Vec<u8>) + Send + 'static,
    query: Query,
) -> Result<Found, Error> {
    relayed_browse(setup, rewrite, async move |server| {
        browse::walk(server, &query).await
    })
}

/// Puts `to` in place of each copy of `from` in `message`, and sets the size of the
/// message.
fn replace(message: &mut Vec<u8>, from: &[u8], to: &[u8]) {
    let mut at = 0;
    while at + from.len() <= message.len() {
        if &message[at..at + from.len()] == from {
            message.splice(at..at + from.len(), to.iter().copied());
            at += to.len();
        } else {
            at += 1;
        }
    }
    let size = u32::try_from(message.len()).expect("a message of a u32 of bytes");
    message[4..8].copy_from_slice(&size.to_le_bytes());
}

/// A setup with the folder `root` under the Objects folder, and a Double variable
/// under it of each node, name, and value of `variables`.
fn tree(
    variables: Vec<(Node, &'static CStr, Option<f64>)>,
) -> impl FnOnce(&Server) + Send + 'static {
    move |server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
        for (node, name, value) in variables {
            server.add_variable(&Variable {
                node,
                parent: ROOT,
                reference: HAS_COMPONENT,
                name,
                data_type: DOUBLE,
                rank: -1,
                access: server::READ,
                value: value.map(|value| server::variant(Value::F64(value)).1),
            });
        }
    }
}

fn level() -> Query {
    Query {
        start: "root".parse().expect("a path"),
        scope: Scope::Level(None),
        values: Values::Read,
    }
}

fn entry(path: &str, tag: &str, shape: Shape) -> Entry {
    Entry {
        path: path.parse().expect("a path"),
        tag: tag.to_owned(),
        shape,
    }
}

fn double() -> Shape {
    Shape::Point(Facts {
        data_type: Some(Type::Scalar(Scalar::F64)),
        unit: None,
        unit_code: None,
        access: Some(Access::Read),
    })
}

/// The node `ns=1;s=x%<F0 9F>`, a name that is not UTF-8, and the node
/// `ns=1;s=x%<U+FFFD>`, which its lossy text names, are two nodes of one folder.
#[test]
fn a_name_that_is_not_utf8_gives_an_escaped_tag_and_hides_no_other_node() {
    let setup = tree(vec![
        (named("x%bc"), c"a", Some(1.5)),
        (named("x%\u{FFFD}"), c"b", Some(2.5)),
    ]);
    let rewrite = |message: &mut Vec<u8>| {
        if response(message).is_some() {
            let to = [4, 0, 0, 0, b'x', b'%', 0xF0, 0x9F];
            replace(message, &[4, 0, 0, 0, b'x', b'%', b'b', b'c'], &to);
        }
    };
    let found = walked(setup, rewrite, level()).expect("found");
    let reason = "its node ID is a name that is empty or not UTF-8, which the kind \
                  cannot read";
    assert_eq!(
        found.entries,
        [
            entry(
                "root/a",
                "ns=1;s%=x%25%F0%9F",
                Shape::Unreadable(reason.into())
            ),
            entry("root/b", "ns=1;s=x%\u{FFFD}", double()),
        ]
    );
    assert_eq!(found.values, [None, Some(sample::Value::F64(2.5))]);
}

/// The server index 1 on the node `ns=1;s=xabc` makes it a node of another server.
#[test]
fn a_node_of_another_server_gives_its_text_as_its_tag() {
    let setup = tree(vec![(named("xabc"), c"a", Some(1.5))]);
    let rewrite = |message: &mut Vec<u8>| {
        if response(message).is_some() {
            let id = [1, 0, 4, 0, 0, 0, b'x', b'a', b'b', b'c'];
            let from = [&[3][..], &id].concat();
            let to = [&[0x43][..], &id, &[1, 0, 0, 0]].concat();
            replace(message, &from, &to);
        }
    };
    let found = walked(setup, rewrite, level()).expect("found");
    let reason = "it is a node of another server, which the kind cannot read";
    assert_eq!(
        found.entries,
        [entry(
            "root/a",
            "svr=1;ns=1;s=xabc",
            Shape::Unreadable(reason.into())
        )]
    );
    assert_eq!(found.values, [None]);
}

#[test]
fn a_variable_of_value_rank_0_is_unreadable_as_an_array() {
    let setup = |server: &Server| {
        tree(Vec::new())(server);
        server.add_variable(&Variable {
            node: named("x"),
            parent: ROOT,
            reference: HAS_COMPONENT,
            name: c"a",
            data_type: DOUBLE,
            rank: 0,
            access: server::READ,
            value: None,
        });
    };
    let found = walked(setup, |_| {}, level()).expect("found");
    let array = Shape::Unreadable("an array".into());
    assert_eq!(found.entries, [entry("root/a", "ns=1;s=x", array)]);
}

/// Each page of `root` holds one reference. The cursor of each page goes on from the
/// continuation point that its browse holds.
#[test]
fn the_cursors_of_one_browse_take_one_browse_and_a_browse_next_for_each_page() {
    let setup = |server: &Server| {
        let names = [c"a", c"b", c"c"];
        let nodes = ["a", "b", "c"].map(named);
        tree(
            nodes
                .into_iter()
                .zip(names)
                .map(|(n, m)| (n, m, None))
                .collect(),
        )(server);
        server.cap_references(1);
    };
    let counts = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let counted = Arc::clone(&counts);
    let rewrite = move |message: &mut Vec<u8>| match response(message) {
        Some(BROWSE_RESPONSE) => _ = counted[0].fetch_add(1, Ordering::Relaxed),
        Some(_) => _ = counted[1].fetch_add(1, Ordering::Relaxed),
        None => {}
    };
    let pages = relayed_browse(setup, rewrite, async |server| {
        let root = entry("root", &ROOT.to_string(), Shape::Folder);
        let mut pages: Vec<Page> = Vec::new();
        let mut cursor = None;
        loop {
            let page = server.level(Some(&root), cursor.as_ref()).await;
            let page = page.expect("a page").expect("a folder");
            cursor = page.next.clone();
            pages.push(page);
            if cursor.is_none() {
                return pages;
            }
        }
    });
    let names: Vec<(Vec<String>, Option<Cursor>)> = pages
        .into_iter()
        .map(|page| {
            let names = page.entries.into_iter().map(|child| child.name).collect();
            (names, page.next)
        })
        .collect();
    let cursor = |text: &str| Some(Cursor::new(text.to_owned()));
    assert_eq!(
        names,
        [
            (vec!["a".to_owned()], cursor("1")),
            (vec!["b".to_owned()], cursor("2")),
            (vec!["c".to_owned()], None),
        ]
    );
    let counts = counts.each_ref().map(|count| count.load(Ordering::Relaxed));
    assert_eq!(counts, [1, 2], "Browse and BrowseNext responses");
}

/// The children of `root` in a page each, each rewritten to the class `ObjectType`,
/// which the client drops, so each page of `root` has no entry and a continuation
/// point. A walk counts a page with no entries as one visit and ends `Budget` past
/// `VISITS` (10 000).
#[test]
fn pages_with_no_entries_end_at_the_budget_of_a_walk() {
    const CHILDREN: u32 = 10_050;
    let setup = |server: &Server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        for index in 0..CHILDREN {
            let name = format!("child{index}\0");
            let name = CStr::from_bytes_with_nul(name.as_bytes()).expect("one NUL");
            let node = Node {
                namespace: 1,
                key: Key::Number(20_000 + index),
            };
            server.add_folder(&node, &ROOT, ORGANIZES, name, name);
        }
        server.cap_references(1);
    };
    let rewrite = move |message: &mut Vec<u8>| {
        if response(message).is_none() {
            return;
        }
        let mut found = Vec::new();
        let mut at = 0;
        while at + 9 <= message.len() {
            let length = i32::from_le_bytes(message[at..at + 4].try_into().expect("4"));
            let start = at + 4;
            if (6..=10).contains(&length) && message[start..].starts_with(b"child") {
                let end = start + usize::try_from(length).expect("positive");
                if end <= message.len()
                    && message[start + 5..end].iter().all(u8::is_ascii_digit)
                {
                    found.push(end);
                    at = end;
                    continue;
                }
            }
            at += 1;
        }
        for pair in found.chunks(2) {
            if let [_, display] = pair {
                let class = &mut message[*display..*display + 4];
                assert_eq!(class, &1_u32.to_le_bytes(), "the class of an Object");
                class.copy_from_slice(&8_u32.to_le_bytes());
            }
        }
    };
    let query = Query {
        start: "root".parse().expect("a path"),
        scope: Scope::Match("**".parse().expect("a pattern")),
        values: Values::Skipped,
    };
    let found = walked(setup, rewrite, query).expect("found");
    assert_eq!(found.end, End::Budget);
}

/// A setup with a tree that a walk of [`everything`] holds 4 points of at once, with
/// the point of the Objects folder: the folders `root/a/c`, `root/a/d`, `root/a/e`,
/// and `root/b`, two variables in each of `b`, `c`, `d`, and `e`, and one reference in
/// each page of the server. A child folder has a lower number than its parent, so the
/// order of the keys of the points is not the order of their use.
fn nested(server: &Server) {
    let folder = |number| Node {
        namespace: 1,
        key: Key::Number(number),
    };
    server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
    server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
    let folders = [
        (6001, ROOT, c"a"),
        (6002, ROOT, c"b"),
        (6003, folder(6001), c"c"),
        (6004, folder(6001), c"d"),
        (6005, folder(6001), c"e"),
    ];
    for (number, parent, name) in folders {
        server.add_folder(&folder(number), &parent, ORGANIZES, name, name);
    }
    for (number, parent) in [(7010, 6002), (7020, 6003), (7030, 6004), (7040, 6005)] {
        for (offset, name) in [c"x", c"y"].into_iter().enumerate() {
            let offset = u32::try_from(offset).expect("a u32");
            server.add_variable(&Variable {
                node: folder(number + offset),
                parent: folder(parent),
                reference: HAS_COMPONENT,
                name,
                data_type: DOUBLE,
                rank: -1,
                access: server::READ,
                value: None,
            });
        }
    }
    server.cap_references(1);
}

fn everything() -> Query {
    Query {
        start: "root".parse().expect("a path"),
        scope: Scope::Match("**".parse().expect("a pattern")),
        values: Values::Skipped,
    }
}

/// What a walk of [`everything`] in [`nested`] gives when the server gives `most` as
/// its `MaxBrowseContinuationPoints`, the most points that the session held, and the
/// count of Browse requests.
fn held(most: u16) -> (Result<Found, Error>, usize, usize) {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            nested(peer.server);
            peer.server.report_points(most);
            let browses = peer.server.count_browses();
            let mut client = peer.connected().await;
            let found =
                browse::walk(&mut Browse::new(&mut client), &everything()).await;
            client.close().await;
            (found, browses.points(), browses.get())
        })
        .await
    });
    out
}

/// With no bound from the server, a walk holds a point for each open folder: the
/// Objects folder, `root`, `root/a`, and `root/a/c`. With a bound of 1 or 2, it holds
/// no more, and gives the same entries. At the bound of 2, it releases the point of
/// `root`, used least lately, so it browses `root` once more and `root/a` no more.
#[test]
fn a_walk_holds_no_more_points_than_the_server_holds_for_a_session() {
    let (unbound, points, browses) = held(0);
    let unbound = unbound.expect("found");
    assert_eq!(unbound.entries.len(), 8, "{:?}", unbound.entries);
    assert_eq!((points, browses), (4, 7));
    for (most, browses) in [(1, 10), (2, 8)] {
        let (found, points, browsed) = held(most);
        assert_eq!(found.expect("found").entries, unbound.entries);
        assert_eq!(
            (points, browsed),
            (usize::from(most), browses),
            "bound {most}"
        );
    }
}

/// Gives each server message to `rewrite`, and makes the result of the Browse
/// response of the count `nth` from 1 `BadNoContinuationPoints`.
fn refusing(nth: usize) -> impl FnMut(&mut Vec<u8>) + Send + 'static {
    let mut browses = 0;
    move |message: &mut Vec<u8>| {
        if response(message) != Some(BROWSE_RESPONSE) {
            return;
        }
        browses += 1;
        if browses == nth {
            // The response header of open62541 is 24 bytes, then the count of results.
            assert_eq!(message[52..56], 1_i32.to_le_bytes(), "one result");
            assert_eq!(message[56..60], [0; 4], "a Good result");
            let status = Status::BAD_NO_CONTINUATION_POINTS.0.to_le_bytes();
            message[56..60].copy_from_slice(&status);
        }
    }
}

/// The server refuses a point for the Browse of `root/a`, while the walk holds the
/// points of the Objects folder and `root`: the walk releases the point of the Objects
/// folder, browses `a` again, and gives each entry.
#[test]
fn a_browse_that_the_server_gives_no_point_releases_one_and_browses_again() {
    let (unbound, ..) = held(0);
    let found = relayed_browse(nested, refusing(3), async |server| {
        browse::walk(server, &everything()).await
    });
    assert_eq!(
        found.expect("found").entries,
        unbound.expect("found").entries
    );
}

/// The server refuses a point for the first Browse, of the Objects folder, when the
/// walk holds none to release.
#[test]
fn a_browse_that_the_server_gives_no_point_with_none_held_fails() {
    let found = relayed_browse(nested, refusing(1), async |server| {
        browse::walk(server, &everything()).await
    });
    let Err(Error::Device(source)) = found else {
        panic!("not a device error: {found:?}");
    };
    assert_eq!(
        source.to_string(),
        "the browse failed with BadNoContinuationPoints"
    );
}
