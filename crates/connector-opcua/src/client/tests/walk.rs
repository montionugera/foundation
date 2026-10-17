use std::ffi::{CStr, CString};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use connector::browse::{
    self, Access, Cursor, Device, End, Entry, Facts, Found, Page, Query, Scope, Shape,
    Values,
};
use connector::kind::Error;
use types::sample::{self, Scalar, Type};

use super::{RELAY, Relay, relayed, run, serve};
use crate::browse::{BYTES, Server as Browse};
use crate::ffi::Status;
use crate::node::ns0::{self, HAS_COMPONENT, ORGANIZES};
use crate::node::{self, Key, Name, Node};
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

/// The relay moves the node `ns=1;s=x%bc` to namespace 0 with a name that is not
/// UTF-8, whose tag has no `ns=` and is no node.
#[test]
fn a_name_of_namespace_0_that_is_not_utf8_gives_a_tag_that_is_no_node() {
    let setup = tree(vec![(named("x%bc"), c"a", Some(1.5))]);
    let rewrite = |message: &mut Vec<u8>| {
        if response(message).is_some() {
            let from = [3, 1, 0, 4, 0, 0, 0, b'x', b'%', b'b', b'c'];
            let to = [3, 0, 0, 4, 0, 0, 0, b'x', b'%', 0xF0, 0x9F];
            replace(message, &from, &to);
        }
    };
    let found = walked(setup, rewrite, level()).expect("found");
    let reason = "its node ID is a name that is empty or not UTF-8, which the kind \
                  cannot read";
    let tag = "s%=x%25%F0%9F";
    let unreadable = Shape::Unreadable(reason.into());
    assert_eq!(found.entries, [entry("root/a", tag, unreadable)]);
    assert_eq!(tag.parse::<Node>(), Err(node::Error::Syntax));
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

/// The references of `root` in a page each, the second rewritten to the class
/// `ObjectType`, which the client drops, so the second page of the server has no
/// references. A new browse of each cursor, as a `Scope::Level` browse of the kind
/// does, reaches the third reference.
#[test]
fn a_new_browse_of_each_cursor_goes_past_a_server_page_with_no_references() {
    let setup = |server: &Server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        for (index, name) in [c"keep0", c"drop1", c"keep2"].into_iter().enumerate() {
            let node = Node {
                namespace: 1,
                key: Key::Number(20_000 + u32::try_from(index).expect("small")),
            };
            server.add_folder(&node, &ROOT, ORGANIZES, name, name);
        }
        server.cap_references(1);
    };
    let rewrite = |message: &mut Vec<u8>| {
        if response(message).is_none() {
            return;
        }
        let pattern = [&5_i32.to_le_bytes()[..], b"drop1"].concat();
        let ends: Vec<usize> = message
            .windows(pattern.len())
            .enumerate()
            .filter(|(_, window)| *window == pattern.as_slice())
            .map(|(at, _)| at + pattern.len())
            .collect();
        for pair in ends.chunks(2) {
            if let [_, display] = pair {
                let class = &mut message[*display..*display + 4];
                assert_eq!(class, &1_u32.to_le_bytes(), "the class of an Object");
                class.copy_from_slice(&8_u32.to_le_bytes());
            }
        }
    };
    let (pages, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            setup(peer.server);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            relayed(&node, &Relay::default(), rewrite, async {
                let mut client = peer.connect(&url).await.expect("a connect");
                let root = entry("root", &ROOT.to_string(), Shape::Folder);
                let mut pages: Vec<(Vec<String>, Option<Cursor>)> = Vec::new();
                let mut cursor: Option<Cursor> = None;
                for _ in 0..6 {
                    let mut browse = Browse::new(&mut client);
                    let page = browse.level(Some(&root), cursor.as_ref()).await;
                    let page = page.expect("a page").expect("a folder");
                    let names = page.entries.into_iter().map(|child| child.name);
                    cursor = page.next.clone();
                    pages.push((names.collect(), page.next));
                    if cursor.is_none() {
                        break;
                    }
                }
                client.close().await;
                pages
            })
            .await
        })
        .await
    });
    let names: Vec<String> = pages.iter().flat_map(|(n, _)| n.clone()).collect();
    assert_eq!(names, ["keep0", "keep2"], "pages: {pages:?}");
    let last = pages.last().map(|(_, next)| next.clone());
    assert_eq!(last, Some(None), "pages: {pages:?}");
}

/// `count` folders `n00000` and on under `root`, in a page each.
fn numbered(count: u32) -> impl FnOnce(&Server) + Send + 'static {
    move |server: &Server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        for index in 0..count {
            let node = Node {
                namespace: 1,
                key: Key::Number(20_000 + index),
            };
            let name = CString::new(format!("n{index:05}")).expect("no NUL");
            server.add_folder(&node, &ROOT, ORGANIZES, &name, &name);
        }
        server.cap_references(1);
    }
}

/// Gives each Browse and `BrowseNext` response to `change` with the end of the display
/// name of each of its references of [`numbered`], and the place of that reference in
/// the order of the server.
fn placed(
    mut change: impl FnMut(&mut Vec<u8>, usize, usize) + Send + 'static,
) -> impl FnMut(&mut Vec<u8>) + Send + 'static {
    let mut order = std::collections::BTreeMap::new();
    move |message: &mut Vec<u8>| {
        if response(message).is_none() {
            return;
        }
        // Each name is the length 6, `n`, and 5 digits, as the browse name and then the
        // display name of its reference, which the class follows.
        let ends: Vec<(usize, usize)> = message
            .windows(10)
            .enumerate()
            .filter(|(_, window)| {
                window[..5] == [6, 0, 0, 0, b'n']
                    && window[5..].iter().all(u8::is_ascii_digit)
            })
            .map(|(at, window)| {
                let digits = std::str::from_utf8(&window[5..]).expect("digits");
                (at + 10, digits.parse().expect("a number"))
            })
            .collect();
        for pair in ends.chunks(2).rev() {
            if let [_, (display, index)] = pair {
                let next = order.len();
                let place = *order.entry(*index).or_insert(next);
                change(message, *display, place);
            }
        }
    }
}

/// What a new browse of `root` gives for each cursor from `cursor`, at most `pages`
/// times, through a relay that gives each server message to `rewrite`.
fn new_browses(
    setup: impl FnOnce(&Server) + Send + 'static,
    rewrite: impl FnMut(&mut Vec<u8>) + Send + 'static,
    cursor: Option<Cursor>,
    pages: usize,
) -> Result<Vec<(usize, Option<Cursor>)>, Error> {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            setup(peer.server);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            relayed(&node, &Relay::default(), rewrite, async {
                let mut client = peer.connect(&url).await.expect("a connect");
                let root = entry("root", &ROOT.to_string(), Shape::Folder);
                let (mut out, mut cursor) = (Vec::new(), cursor);
                for _ in 0..pages {
                    let mut browse = Browse::new(&mut client);
                    let page = browse.level(Some(&root), cursor.as_ref()).await;
                    let page = match page {
                        Ok(page) => page.expect("a folder"),
                        Err(error) => {
                            client.close().await;
                            return Err(error);
                        }
                    };
                    cursor = page.next.clone();
                    out.push((page.entries.len(), page.next));
                    if cursor.is_none() {
                        break;
                    }
                }
                client.close().await;
                Ok(out)
            })
            .await
        })
        .await
    });
    out
}

/// 10 003 references under `root`, of which the server gives the second to the
/// 10 002nd, `VISITS` + 1 of them, as `ObjectType`, which the browse gives no entry
/// for. A new browse of each cursor counts them, so it goes past them.
#[test]
fn a_new_browse_of_each_cursor_goes_past_more_than_visits_references_of_other_classes()
{
    let rewrite = placed(|message, display, place| {
        if (1..=10_001).contains(&place) {
            let class = &mut message[display..display + 4];
            assert_eq!(class, &1_u32.to_le_bytes(), "the class of an Object");
            class.copy_from_slice(&8_u32.to_le_bytes());
        }
    });
    let pages = new_browses(numbered(10_003), rewrite, None, 4);
    let pages = pages.expect("the pages");
    assert_eq!(
        pages,
        [
            (1, Some(Cursor::new("1".to_owned()))),
            (0, Some(Cursor::new("2".to_owned()))),
            (0, Some(Cursor::new("3".to_owned()))),
            (0, Some(Cursor::new("4".to_owned()))),
        ]
    );
}

/// Two references named `same` under `root`, in a page each, of which the server gives
/// the first as `ObjectType`. A new browse of the cursor `1` gives the second its plain
/// name, as the browse that gave the cursor does.
#[test]
fn a_reference_of_another_class_before_a_cursor_takes_no_name() {
    let setup = |server: &Server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        add_folders(
            server,
            &ROOT,
            vec![(20_000, "same".to_owned()), (20_001, "same".to_owned())],
        );
        server.cap_references(1);
    };
    let mut first = true;
    let rewrite = move |message: &mut Vec<u8>| {
        let pattern = [&4_i32.to_le_bytes()[..], b"same"].concat();
        let at = message
            .windows(8)
            .position(|window| window == pattern.as_slice());
        if response(message).is_none() || !first {
            return;
        }
        // The display name `d` of `add_folders`, with an empty locale, then the class.
        let class = at.expect("a reference") + 8 + 10;
        assert_eq!(message[class..class + 4], 1_u32.to_le_bytes(), "an Object");
        message[class..class + 4].copy_from_slice(&8_u32.to_le_bytes());
        first = false;
    };
    let names = relayed_browse(setup, rewrite, async |server| {
        let root = entry("root", &ROOT.to_string(), Shape::Folder);
        let cursor = Cursor::new("1".to_owned());
        let page = server.level(Some(&root), Some(&cursor)).await;
        let page = page.expect("a page").expect("a folder");
        page.entries
            .into_iter()
            .map(|child| child.name)
            .collect::<Vec<_>>()
    });
    assert_eq!(names, ["same"]);
}

/// Makes the page of `message`, of one reference, a page with no references.
fn emptied(message: &mut Vec<u8>) {
    // The results start at 52: their count, then the status, the point, and the count
    // of the references of the first. The count of their diagnostics ends the message.
    assert_eq!(message[52..56], 1_i32.to_le_bytes(), "one result");
    let point = i32::from_le_bytes(message[60..64].try_into().expect("4 bytes"));
    let at = 64 + usize::try_from(point.max(0)).expect("a size");
    assert_eq!(message[at..at + 4], 1_i32.to_le_bytes(), "one reference");
    message[at..at + 4].copy_from_slice(&0_i32.to_le_bytes());
    let end = message.len() - 4;
    message.drain(at + 4..end);
    let size = u32::try_from(message.len()).expect("a size");
    message[4..8].copy_from_slice(&size.to_le_bytes());
}

/// What a new browse of the cursor `cursor` gives over 10 003 references in a page
/// each, of which the relay empties the pages of the second to the one at `last` in the
/// order of the server.
fn past_empty_pages(
    cursor: &str,
    last: usize,
) -> Result<Vec<(usize, Option<Cursor>)>, Error> {
    let rewrite = placed(move |message, _, place| {
        if (1..=last).contains(&place) {
            emptied(message);
        }
    });
    let cursor = Some(Cursor::new(cursor.to_owned()));
    new_browses(numbered(10_003), rewrite, cursor, 1)
}

/// The cursor `2` skips one reference, reads past `VISITS` (10 000) pages with no
/// references, skips one more, and gives the last.
#[test]
fn a_new_browse_of_a_cursor_reads_past_visits_pages_with_no_references() {
    let pages = past_empty_pages("2", 10_000).expect("the pages");
    assert_eq!(pages, [(1, None)]);
}

/// The cursor `1` skips one reference, and the next `VISITS` + 1 pages have no
/// references, so its page would have the cursor `1` again.
#[test]
fn a_new_browse_of_a_cursor_past_more_than_visits_pages_with_no_references_fails() {
    let pages = past_empty_pages("1", 10_001);
    let Err(Error::Device(source)) = pages else {
        panic!("not a device error: {pages:?}");
    };
    assert_eq!(
        source.to_string(),
        "the browse reads past 10000 pages with no references"
    );
}

/// Adds a folder under `parent` for each number and browse name of `children`, with
/// the display name `d`.
fn add_folders(server: &Server, parent: &Node, children: Vec<(u32, String)>) {
    for (number, name) in children {
        let node = Node {
            namespace: 1,
            key: Key::Number(number),
        };
        let name = CString::new(name).expect("no NUL");
        server.add_folder(&node, parent, ORGANIZES, &name, c"d");
    }
}

/// The folders `a` and `b` under `root`, 16 references in each page of the server,
/// and children of 4096 bytes of tag and name each: 32 in `a`, and 4080 in `b` with
/// `extra` more bytes in the name of the first. The browse reads the first page of `a`
/// `opens` times and holds its point, so it keeps 64 KiB for it, and the pages of `b`
/// keep `BYTES - 64 KiB + extra` at its last page.
fn budget(extra: usize, opens: usize) -> Result<usize, Error> {
    const A: Node = Node {
        namespace: 1,
        key: Key::Number(7001),
    };
    const B: Node = Node {
        namespace: 1,
        key: Key::Number(7002),
    };
    // The tag of each child, such as `ns=1;i=20000`, has 12 bytes.
    let name =
        |number: u32, extra: usize| format!("{number:0>width$}", width = 4084 + extra);
    let setup = move |server: &Server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        server.add_folder(&A, &ROOT, ORGANIZES, c"a", c"a");
        server.add_folder(&B, &ROOT, ORGANIZES, c"b", c"b");
        add_folders(
            server,
            &A,
            (20_000..20_032).map(|n| (n, name(n, 0))).collect(),
        );
        let b =
            (30_000..34_080).map(|n| (n, name(n, if n == 30_000 { extra } else { 0 })));
        add_folders(server, &B, b.collect());
        server.cap_references(16);
    };
    relayed_browse(
        setup,
        |_| {},
        async move |server| {
            let a = entry("root/a", &A.to_string(), Shape::Folder);
            for _ in 0..opens {
                let first = server.level(Some(&a), None).await?.expect("a folder");
                assert_eq!(first.next, Some(Cursor::new("16".to_owned())));
            }
            let b = entry("root/b", &B.to_string(), Shape::Folder);
            let (mut entries, mut cursor) = (0, None);
            loop {
                let page = server.level(Some(&b), cursor.as_ref()).await?;
                let page = page.expect("a folder");
                entries += page.entries.len();
                cursor = page.next;
                if cursor.is_none() {
                    return Ok(entries);
                }
            }
        },
    )
}

#[test]
fn a_browse_keeps_the_tags_and_names_of_its_budget() {
    assert_eq!(BYTES, 16 * 4096 + 4080 * 4096);
    assert_eq!(budget(0, 1).expect("the pages of `b`"), 4080);
}

#[test]
fn a_browse_that_keeps_more_than_its_budget_fails() {
    let found = budget(1, 1);
    let Err(Error::Device(source)) = found else {
        panic!("not a device error: {found:?}");
    };
    assert_eq!(
        source.to_string(),
        "the tags and names of the browse pass 16 MiB"
    );
}

/// The second page of `a` from its start gives the cursor of the first, so the browse
/// keeps the 64 KiB of `a` once, and one more byte passes the budget.
#[test]
fn a_folder_open_twice_keeps_its_budget_once() {
    assert_eq!(budget(0, 2).expect("the pages of `b`"), 4080);
    let found = budget(1, 2);
    let Err(Error::Device(source)) = found else {
        panic!("not a device error: {found:?}");
    };
    assert_eq!(
        source.to_string(),
        "the tags and names of the browse pass 16 MiB"
    );
}

/// The relay frees the point of the first cursor past `SKIP` (100 000) when the browse
/// goes on from it, and that browse fails, and does not read the folder again from its
/// start.
#[test]
fn a_cursor_past_the_skip_bound_whose_point_the_server_freed_fails() {
    let setup = |server: &Server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        let children = (0..100_600).map(|index| (200_000 + index, format!("c{index}")));
        add_folders(server, &ROOT, children.collect());
    };
    let freed = Arc::new(AtomicBool::new(false));
    let free = Arc::clone(&freed);
    let rewrite = move |message: &mut Vec<u8>| {
        if response(message) == Some(BROWSE_NEXT_RESPONSE)
            && free.swap(false, Ordering::Relaxed)
        {
            let invalid = Status::BAD_CONTINUATION_POINT_INVALID.0.to_le_bytes();
            message[56..60].copy_from_slice(&invalid);
        }
    };
    let page = relayed_browse(setup, rewrite, async move |server| {
        let root = entry("root", &ROOT.to_string(), Shape::Folder);
        let mut cursor: Option<Cursor> = None;
        loop {
            let page = server.level(Some(&root), cursor.as_ref()).await;
            let page = page.expect("a page").expect("a folder");
            cursor = page.next;
            let count: usize = cursor
                .as_ref()
                .expect("a cursor")
                .as_str()
                .parse()
                .expect("a count");
            if count > 100_000 {
                freed.store(true, Ordering::Relaxed);
                let page = server.level(Some(&root), cursor.as_ref()).await;
                return page;
            }
        }
    });
    let Err(Error::Device(source)) = page else {
        panic!("not a device error: {page:?}");
    };
    assert_eq!(
        source.to_string(),
        "the browse failed with BadContinuationPointInvalid"
    );
}
