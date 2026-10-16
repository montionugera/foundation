use std::cell::Cell;
use std::ffi::CStr;
use std::num::NonZeroU32;

use proptest::prelude::*;

use super::{
    Client, READ_RESPONSE, READS, RELAY, Relay, VALUE, drop_reads, long, recount_of,
    relayed, run, serve,
};
use crate::client::browse::{Class, Description, Page, Reference, Unit};
use crate::client::service::{self, Item, Request};
use crate::ffi::{self, Service, Status};
use crate::node::{Key, Name, Node};
use crate::server::{Limits, Server};

const BAD_CONTINUATION_POINT_INVALID: Status = Status(0x804A_0000);

/// The binary encoding of a Browse response, a `NodeId` of 4 bytes.
const BROWSE_RESPONSE: [u8; 4] = [0x01, 0x00, 0x12, 0x02];

/// The binary encoding of a `BrowseNext` response.
const BROWSE_NEXT_RESPONSE: [u8; 4] = [0x01, 0x00, 0x18, 0x02];

/// The binary encoding of a `TranslateBrowsePathsToNodeIds` response.
const TRANSLATE_RESPONSE: [u8; 4] = [0x01, 0x00, 0x2D, 0x02];

/// No bound on the references of a page, other than the size of a message.
const ALL: NonZeroU32 = NonZeroU32::MAX;

const OBJECTS: Node = ns0(85);
const ORGANIZES: u32 = 35;
const HAS_COMPONENT: u32 = 47;
const HAS_PROPERTY: u32 = 46;
const HAS_SUBTYPE: u32 = 45;
const BASE_DATA_TYPE: Node = ns0(24);
const DOUBLE: Node = ns0(11);
const STRUCTURE: Node = ns0(22);
const ENUMERATION: Node = ns0(29);

const ROOT: Node = ns1(7000);

const CEFACT: &CStr = c"http://www.opcfoundation.org/UA/units/un/cefact";

const fn ns0(id: u32) -> Node {
    Node {
        namespace: 0,
        key: Key::Number(id),
    }
}

const fn ns1(id: u32) -> Node {
    Node {
        namespace: 1,
        key: Key::Number(id),
    }
}

fn per(most: u32) -> NonZeroU32 {
    NonZeroU32::new(most).expect("a count that is not 0")
}

fn named(name: &str) -> Node {
    Node {
        namespace: 1,
        key: Key::Name(Name::new(name).expect("a name that is not empty")),
    }
}

fn reference(
    node: Option<Node>,
    via: u32,
    name: &str,
    display: &str,
    class: Class,
) -> Reference {
    Reference {
        node,
        name: name.to_owned(),
        display: display.to_owned(),
        class,
        via: Some(ns0(via)),
    }
}

/// Adds the folder [`ROOT`] under `Objects`, with `count` folders `child<i>` under
/// it.
fn folders(server: &Server, count: u32) {
    server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
    for index in 0..count {
        let name = format!("child{index}\0");
        let name = CStr::from_bytes_with_nul(name.as_bytes()).expect("one NUL");
        server.add_folder(&ns1(7001 + index), &ROOT, ORGANIZES, name, name);
    }
}

/// The pages of a browse of [`ROOT`] with at most `most` references, after `setup`.
fn pages(
    most: NonZeroU32,
    setup: impl FnOnce(&Server) + Send + 'static,
) -> Result<Vec<Page>, Status> {
    run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            setup(peer.server);
            let mut client = peer.connected().await;
            let mut pages = Vec::new();
            let mut next = client.browse(&ROOT, most).await?.map(|page| {
                let next = page.next.as_ref().map(|_| ());
                pages.push(page);
                next
            });
            while next.flatten().is_some() {
                let point = pages.last_mut().and_then(|page| page.next.take());
                let page = client.browse_next(point.expect("a point")).await?;
                next = Some(page.next.as_ref().map(|_| ()));
                pages.push(page);
            }
            client.close().await;
            Ok(pages)
        })
        .await
    })
    .0
}

/// A browse gives each forward hierarchical reference of an Object, a Variable, or a
/// View, with its type, names, and class, and no reference of another kind or
/// direction. A GUID node gives no node.
#[test]
fn a_browse_gives_each_hierarchical_reference_with_its_names_and_class() {
    let pages = pages(ALL, |server| {
        server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
        server.add_folder(&ns1(7001), &ROOT, ORGANIZES, c"a", c"The A");
        server.add_variable(
            &named("s;=\u{e9}"),
            &ROOT,
            HAS_COMPONENT,
            c"speed",
            &DOUBLE,
            -1,
            3,
        );
        server.add_variable(&ns1(7003), &ROOT, HAS_PROPERTY, c"prop", &DOUBLE, -1, 1);
        server.add_view(&ns1(7002), &ROOT, c"view");
        server.add_guid(&ROOT);
        server.add_reference(&ns1(7001), ORGANIZES, &ROOT);
    })
    .expect("the browse works");
    assert_eq!(pages.len(), 1);
    let mut references = pages.into_iter().next().expect("one").references;
    references.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        references,
        [
            reference(Some(ns1(7001)), ORGANIZES, "a", "The A", Class::Object),
            reference(None, ORGANIZES, "guid", "guid", Class::Object),
            reference(
                Some(ns1(7003)),
                HAS_PROPERTY,
                "prop",
                "prop",
                Class::Variable
            ),
            reference(
                Some(named("s;=\u{e9}")),
                HAS_COMPONENT,
                "speed",
                "speed",
                Class::Variable
            ),
            reference(Some(ns1(7002)), ORGANIZES, "view", "view", Class::View),
        ]
    );
}

#[test]
fn a_browse_of_an_unknown_node_gives_none() {
    let (page, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            client.browse(&ns1(7001), ALL).await
        })
        .await
    });
    assert_eq!(page, Ok(None));
}

/// Pages of at most 2 references give the references of one page, in order.
#[test]
fn a_browse_of_two_per_page_over_5_gives_3_pages_in_order() {
    let whole = pages(ALL, |server| folders(server, 5)).expect("the browse works");
    let paged = pages(per(2), |server| folders(server, 5)).expect("the browse works");
    let sizes: Vec<usize> = paged.iter().map(|page| page.references.len()).collect();
    assert_eq!(sizes, [2, 2, 1]);
    assert!(paged.iter().all(|page| page.next.is_none()));
    let whole: Vec<Reference> = whole.into_iter().flat_map(|p| p.references).collect();
    let paged: Vec<Reference> = paged.into_iter().flat_map(|p| p.references).collect();
    assert_eq!(whole.len(), 5);
    assert_eq!(paged, whole);
}

/// A point lives in its session: another session gets
/// `BadContinuationPointInvalid`.
#[test]
fn a_point_after_its_session_closes_is_invalid() {
    let (next, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            folders(peer.server, 5);
            let mut client = peer.connected().await;
            let page = client
                .browse(&ROOT, per(2))
                .await
                .expect("the browse works");
            let point = page.and_then(|page| page.next).expect("a point");
            client.close().await;
            let mut other = peer.connected().await;
            other.browse_next(point).await
        })
        .await
    });
    assert_eq!(next, Err(BAD_CONTINUATION_POINT_INVALID));
}

/// Adds the data types of [`describe`] and a variable of each type, of the access 3
/// and the rank -1, unless the last two items say otherwise. Gives the variables.
fn variables(server: &Server) -> Vec<Node> {
    server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
    server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
    server.add_data_type(&ENUMERATION, &BASE_DATA_TYPE, c"Enumeration");
    server.add_data_type(&ns1(7101), &DOUBLE, c"Mine");
    server.add_data_type(&ns1(7102), &ns1(7101), c"MineOfMine");
    server.add_data_type(&ns1(7103), &ENUMERATION, c"Colour");
    server.add_data_type(&ns1(7105), &DOUBLE, c"LoopA");
    server.add_data_type(&ns1(7106), &ns1(7105), c"LoopB");
    let types = [
        DOUBLE,
        ns1(7102),
        ns1(7103),
        STRUCTURE,
        BASE_DATA_TYPE,
        ns1(7106),
    ];
    let mut nodes = Vec::new();
    for (index, data_type) in (0..).zip(types) {
        let node = ns1(7200 + index);
        server.add_variable(&node, &ROOT, HAS_COMPONENT, c"v", &data_type, -1, 3);
        nodes.push(node);
    }
    server.add_variable(&ns1(7250), &ROOT, HAS_COMPONENT, c"array", &DOUBLE, 1, 1);
    nodes.push(ns1(7250));
    // open62541 checks the type of a variable only at its add.
    server.delete_reference(&DOUBLE, HAS_SUBTYPE, &ns1(7105));
    server.add_reference(&ns1(7106), HAS_SUBTYPE, &ns1(7105));
    nodes
}

fn described(base: Option<u32>, rank: i32, access: u8) -> Description {
    Description {
        base,
        rank: Some(rank),
        access: Some(access),
        unit: None,
    }
}

/// The base of Double, of a subtype of a subtype of Double, of a subtype of
/// `Enumeration` (Int32), none for `Structure`, `BaseDataType`, and a loop of types,
/// with the rank and access of each.
#[test]
fn describe_gives_the_base_rank_and_access_of_each_variable() {
    let (descriptions, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let nodes = variables(peer.server);
            let mut client = peer.connected().await;
            client.describe(&nodes).await
        })
        .await
    });
    assert_eq!(
        descriptions,
        Ok(vec![
            described(Some(11), -1, 3),
            described(Some(11), -1, 3),
            described(Some(6), -1, 3),
            described(None, -1, 3),
            described(None, -1, 3),
            described(None, -1, 3),
            described(Some(11), 1, 1),
        ])
    );
}

/// Adds [`ROOT`], and under it a variable of each of `count` new data types, each
/// under its own new parent type under Double. Gives the variables.
fn deep(server: &Server, count: u32) -> Vec<Node> {
    server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
    server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
    (0..count)
        .map(|index| {
            let (parent, data_type, node) = (
                ns1(10_000 + index),
                ns1(11_000 + index),
                ns1(12_000 + index),
            );
            server.add_data_type(&parent, &DOUBLE, c"Parent");
            server.add_data_type(&data_type, &parent, c"Child");
            server.add_variable(&node, &ROOT, HAS_COMPONENT, c"v", &data_type, -1, 3);
            node
        })
        .collect()
}

/// A walk sends one Browse for each step, for the types of all the nodes of a call.
/// The client keeps the supertype of each data type that it browsed, so the next
/// describe sends no Browse, and gives the same base after the server drops a
/// supertype.
#[test]
fn describe_browses_each_step_once_and_each_data_type_once() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let nodes = deep(peer.server, 3);
            let browses = peer.server.count_browses();
            let mut client = peer.connected().await;
            let first = client.describe(&nodes).await;
            let after_first = browses.get();
            peer.server
                .delete_reference(&DOUBLE, HAS_SUBTYPE, &ns1(10_000));
            let again = client.describe(&nodes).await;
            let after_again = browses.get();
            let mut other = peer.connected().await;
            let fresh = other.describe(&nodes[..1]).await;
            let bases = [first, again, fresh].map(|out| {
                out.map(|all| all.into_iter().map(|d| d.base).collect::<Vec<_>>())
            });
            (bases, after_first, after_again)
        })
        .await
    });
    let double = Ok(vec![Some(11); 3]);
    assert_eq!(out, ([double.clone(), double, Ok(vec![None])], 2, 2));
}

/// The unit of each variable with an `EngineeringUnits` property, and none for one
/// with none. The code is `None` for a `unitId` that is not a code and for a
/// `namespaceUri` that is not that of UN/CEFACT, with the text kept.
#[test]
fn describe_gives_the_unit_of_each_variable() {
    let (units, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
            server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
            for index in 0..5 {
                let node = ns1(7200 + index);
                server.add_variable(&node, &ROOT, HAS_COMPONENT, c"v", &DOUBLE, -1, 3);
            }
            let unit = |index: u32, uri, text, code| {
                server.add_unit(
                    &ns1(7300 + index),
                    &ns1(7200 + index),
                    uri,
                    text,
                    code,
                );
            };
            unit(0, CEFACT, c"\xc2\xb0C", 0x0043_454C);
            unit(2, CEFACT, c"", -1);
            unit(3, CEFACT, c"rpm", 0x0001_0000);
            unit(4, c"urn:other", c"deg", 0x0043_454C);
            let nodes: Vec<Node> = (0..5).map(|index| ns1(7200 + index)).collect();
            let mut client = peer.connected().await;
            let out = client.describe(&nodes).await;
            out.map(|all| all.into_iter().map(|d| d.unit).collect::<Vec<_>>())
        })
        .await
    });
    assert_eq!(
        units,
        Ok(vec![
            Some(Unit {
                text: Some("\u{b0}C".to_owned()),
                code: Some("CEL".to_owned()),
            }),
            None,
            Some(Unit {
                text: None,
                code: None,
            }),
            Some(Unit {
                text: Some("rpm".to_owned()),
                code: None,
            }),
            Some(Unit {
                text: Some("deg".to_owned()),
                code: None,
            }),
        ])
    );
}

/// A server whose `MaxNodesPerRead` is 4 refuses a Read of the 3 facts of 5 nodes
/// and of their 5 units, so describe sends the facts in Reads of 1 node and the units
/// in Reads of 4, and gives the same descriptions.
#[test]
fn describe_splits_by_the_max_nodes_per_read_of_the_server() {
    let (descriptions, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.set_per_read(4);
            server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
            server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
            let nodes: Vec<Node> = (0..5).map(|index| ns1(7200 + index)).collect();
            for (index, node) in (0..).zip(&nodes) {
                server.add_variable(node, &ROOT, HAS_COMPONENT, c"v", &DOUBLE, -1, 3);
                server.add_unit(&ns1(7300 + index), node, CEFACT, c"m", 0x004D_5452);
            }
            let mut client = peer.connected().await;
            client.describe(&nodes).await
        })
        .await
    });
    let metre = || Description {
        unit: Some(Unit {
            text: Some("m".to_owned()),
            code: Some("MTR".to_owned()),
        }),
        ..described(Some(11), -1, 3)
    };
    assert_eq!(descriptions, Ok((0..5).map(|_| metre()).collect()));
}

/// A server whose `MaxNodesPerBrowse` and `MaxNodesPerTranslateBrowsePathsToNodeIds`
/// are 2 refuses a Browse or a Translate of 5 nodes, so describe sends 3 of each and
/// gives the same descriptions.
#[test]
fn describe_splits_by_the_max_nodes_per_browse_and_translate_of_the_server() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.set_per_browse(2, 2);
            let nodes = deep(server, 5);
            for (index, node) in (0..).zip(&nodes) {
                server.add_unit(&ns1(7300 + index), node, CEFACT, c"m", 0x004D_5452);
            }
            let browses = server.count_browses();
            let mut client = peer.connected().await;
            let descriptions = client.describe(&nodes).await;
            (descriptions, browses.get())
        })
        .await
    });
    let metre = || Description {
        unit: Some(Unit {
            text: Some("m".to_owned()),
            code: Some("MTR".to_owned()),
        }),
        ..described(Some(11), -1, 3)
    };
    assert_eq!(out, (Ok((0..5).map(|_| metre()).collect()), 6));
}

/// On a server with a `MaxMessageSize` of 8192 bytes, a Read of the facts of 3 nodes
/// of names of 1000 bytes does not fit, so describe halves it. A node whose request
/// alone does not fit has no facts, and the rest of the call goes on. A fact of an
/// unknown node is `None`.
#[test]
fn describe_splits_by_the_message_size_of_the_session() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_limits(Limits {
                message: 8192,
                ..Limits::default()
            });
            let mut client = peer.connected().await;
            let fits = client.describe(&[long(1000), long(1001), long(1002)]).await;
            let past = client.describe(&[VALUE, long(9000)]).await;
            client.close().await;
            (fits, past)
        })
        .await
    });
    let none = || Description::default();
    assert_eq!(
        out,
        (
            Ok(vec![none(), none(), none()]),
            Ok(vec![described(Some(11), -2, 1), none()])
        )
    );
}

/// Each call times out 5 s after it starts while the server does not run, also a
/// describe after the client read the limits of the server.
#[test]
fn each_call_times_out_when_the_server_does_not_answer() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            folders(peer.server, 5);
            let mut client = peer.connected().await;
            let point = client
                .browse(&ROOT, per(2))
                .await
                .expect("the browse works");
            let point = point.and_then(|page| page.next).expect("a point");
            client.describe(&[]).await.expect("the limits read");
            peer.gate.close();
            let start = peer.clock.now();
            let browse = client.browse(&ROOT, ALL).await.map(|_| ());
            let next = client.browse_next(point).await.map(|_| ());
            let describe = client.describe(&[VALUE]).await.map(|_| ());
            let took = peer.clock.now() - start;
            peer.gate.open();
            (browse, next, describe, took.nanos() / 5_000_000_000)
        })
        .await
    });
    let timeout = Err(Status::BAD_TIMEOUT);
    assert_eq!(out, (timeout, timeout, timeout, 3));
}

/// A call after the server closes gets the status of the closed channel, and the
/// next call the status of a client with no connection, also a describe after the
/// client read the limits of the server.
#[test]
fn a_call_after_the_server_closes_fails_with_its_status() {
    let (out, _) = run(0, async |node| {
        let mut client = serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            client.describe(&[]).await.expect("the limits read");
            client
        })
        .await;
        let browse = client.browse(&ROOT, ALL).await.map(|_| ());
        let describe = client.describe(&[VALUE]).await.map(|_| ());
        (browse, describe)
    });
    assert_eq!(
        out,
        (
            Err(Status::BAD_SECURE_CHANNEL_CLOSED),
            Err(Status::BAD_SERVER_NOT_CONNECTED)
        )
    );
}

/// While [`READS`] dropped reads stay in flight, each call fails at the end of its
/// wait and sends nothing, also a describe after the client read the limits of the
/// server.
#[test]
fn each_call_at_the_bound_of_reads_in_flight_fails_with_too_many_operations() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            client.describe(&[]).await.expect("the limits read");
            peer.gate.close();
            drop_reads(&mut client, READS).await;
            let browse = client.browse(&ROOT, ALL).await.map(|_| ());
            let describe = client.describe(&[VALUE]).await.map(|_| ());
            peer.gate.open();
            (browse, describe)
        })
        .await
    });
    let full = Err(Status::BAD_TOO_MANY_OPERATIONS);
    assert_eq!(out, (full, full));
}

/// A walk result that is not Good gives no base, and the client does not keep it, so
/// the next describe of the same client browses the type again and gives its base.
#[test]
fn describe_after_a_failed_walk_result_gives_the_base() {
    let (bases, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let nodes = deep(peer.server, 1);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let mut browses = 0;
            let rewrite = |message: &mut Vec<u8>| {
                if message.starts_with(b"MSG") && message[24..28] == BROWSE_RESPONSE {
                    browses += 1;
                    if browses == 1 {
                        let status = Status::BAD_TOO_MANY_OPERATIONS.0.to_le_bytes();
                        message[56..60].copy_from_slice(&status);
                    }
                }
            };
            relayed(&node, &Relay::default(), rewrite, async {
                let mut client = peer.connect(&url).await.expect("the client connects");
                let mut bases = Vec::new();
                for _ in 0..2 {
                    let described = client.describe(&nodes).await;
                    bases.push(described.map(|all| all[0].base));
                }
                client.close().await;
                bases
            })
            .await
        })
        .await
    });
    assert_eq!(bases, [Ok(None), Ok(Some(11))]);
}

/// What a relay does to one response of the server.
#[derive(Clone, Copy)]
enum Fault {
    /// It gives the response no results.
    Empty,
    /// It drops the response.
    Drop,
    /// It drops the response and both streams.
    Cut,
}

/// The nodes of [`faulted`]: the variable of [`deep`], whose type has two supertypes
/// and a unit, under [`ROOT`] with one folder.
fn faulty(server: &Server) -> Vec<Node> {
    let nodes = deep(server, 1);
    server.add_reference(&DOUBLE, HAS_SUBTYPE, &ns1(11_000));
    server.add_unit(&ns1(7300), &nodes[0], CEFACT, c"m", 0x004D_5452);
    server.add_folder(&ns1(7001), &ROOT, ORGANIZES, c"a", c"A");
    nodes
}

/// Gives the answer of `call` on the nodes of [`faulty`], through a relay that does
/// `fault` to the `nth` response of the type `kind` after the client reads the limits
/// of the server.
fn faulted<T: Send + 'static>(
    kind: [u8; 4],
    nth: usize,
    fault: Fault,
    call: impl AsyncFnOnce(&mut Client, Vec<Node>) -> T + Send + 'static,
) -> T {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let nodes = faulty(peer.server);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let (control, armed, seen) =
                (Relay::default(), Cell::new(false), Cell::new(0));
            let rewrite = |message: &mut Vec<u8>| {
                if !(armed.get()
                    && message.starts_with(b"MSG")
                    && message[24..28] == kind)
                {
                    return;
                }
                seen.set(seen.get() + 1);
                if seen.get() != nth {
                    return;
                }
                match fault {
                    Fault::Empty => recount_of(message, 0),
                    Fault::Drop => message.clear(),
                    Fault::Cut => {
                        message.clear();
                        control.cut.set(true);
                    }
                }
            };
            relayed(&node, &control, rewrite, async {
                let mut client = peer.connect(&url).await.expect("the client connects");
                let none = client.describe(&[]).await;
                assert_eq!(none, Ok(vec![]), "a describe of no nodes reads the limits");
                armed.set(true);
                let out = call(&mut client, nodes).await;
                client.close().await;
                out
            })
            .await
        })
        .await
    });
    out
}

/// The faults of [`faulted`] at each request of a browse and of a describe, other
/// than the read of the limits, as `fault` gives them.
fn each_request(fault: Fault) -> [Result<(), Status>; 7] {
    let describe = |kind, nth| {
        faulted(kind, nth, fault, async |client, nodes| {
            client.describe(&nodes).await.map(|_| ())
        })
    };
    [
        faulted(BROWSE_RESPONSE, 1, fault, async |client, _| {
            client.browse(&ROOT, ALL).await.map(|_| ())
        }),
        faulted(BROWSE_NEXT_RESPONSE, 1, fault, async |client, _| {
            let page = client.browse(&ROOT, per(1)).await;
            let point = page.ok().flatten().and_then(|page| page.next);
            client
                .browse_next(point.expect("a point"))
                .await
                .map(|_| ())
        }),
        describe(READ_RESPONSE, 1),
        describe(TRANSLATE_RESPONSE, 1),
        describe(READ_RESPONSE, 2),
        describe(BROWSE_RESPONSE, 1),
        describe(BROWSE_NEXT_RESPONSE, 1),
    ]
}

/// The calls of [`faulted`] work with no fault: the variable has a base, through two
/// supertypes, and a unit, and the browse of [`ROOT`] gives the folder and the
/// variable.
#[test]
fn the_calls_of_a_faulted_relay_work_with_no_fault() {
    let out = faulted([0; 4], 1, Fault::Drop, async |client, nodes| {
        let page = client.browse(&ROOT, ALL).await;
        let page = page.map(|page| page.map(|page| page.references.len()));
        let units = client.describe(&nodes).await;
        (
            page,
            units.map(|all| all.into_iter().map(|d| d.base).collect::<Vec<_>>()),
        )
    });
    assert_eq!(out, (Ok(Some(2)), Ok(vec![Some(11)])));
}

/// An answer with more or fewer results than the request asked for fails the call, at
/// each request whose count the client checks: all but the release of a point.
#[test]
fn an_answer_with_the_wrong_count_of_results_fails_the_call() {
    let wrong = Err(Status::BAD_UNEXPECTED_ERROR);
    let [browse, next, facts, translate, units, walk, release] =
        each_request(Fault::Empty);
    assert_eq!(
        [browse, next, facts, translate, units, walk, release],
        [wrong, wrong, wrong, wrong, wrong, wrong, Ok(())]
    );
}

/// A request whose answer does not come fails its call with `BadTimeout`, at each
/// request of a browse and of a describe.
#[test]
fn each_request_times_out_when_its_answer_does_not_come() {
    assert_eq!(each_request(Fault::Drop), [Err(Status::BAD_TIMEOUT); 7]);
}

/// A channel that closes before the answer of a request fails its call with the
/// status of the closed channel, at each request of a browse and of a describe.
#[test]
fn each_request_fails_when_the_channel_closes_before_its_answer() {
    assert_eq!(
        each_request(Fault::Cut),
        [Err(Status::BAD_SECURE_CHANNEL_CLOSED); 7]
    );
}

/// A server whose `MaxNodesPerRead` is 1 or 2 takes a Read of fewer than the 3 facts
/// of a node, so describe sends each fact in a Read of its own or with the next.
#[test]
fn describe_on_a_server_whose_max_nodes_per_read_is_below_3() {
    for most in [1, 2] {
        let (out, _) = run(0, async move |node| {
            serve(&node, 4, async |mut peer| {
                let server = peer.server;
                server.set_per_read(most);
                let nodes = deep(server, 2);
                server.add_unit(&ns1(7300), &nodes[0], CEFACT, c"m", 0x004D_5452);
                let mut client = peer.connected().await;
                client.describe(&nodes).await
            })
            .await
        });
        let metre = Description {
            unit: Some(Unit {
                text: Some("m".to_owned()),
                code: Some("MTR".to_owned()),
            }),
            ..described(Some(11), -1, 3)
        };
        assert_eq!(out, Ok(vec![metre, described(Some(11), -1, 3)]), "{most}");
    }
}

/// Gives the base of a variable of a type `links` subtypes under Double, and the count
/// of the Browse requests of its describe.
fn walk_of(links: u32) -> (Result<Option<u32>, Status>, usize) {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
            server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
            for link in (0..links).rev() {
                let parent = if link + 1 == links {
                    DOUBLE
                } else {
                    ns1(10_001 + link)
                };
                server.add_data_type(&ns1(10_000 + link), &parent, c"Link");
            }
            let variable = ns1(7200);
            server.add_variable(
                &variable,
                &ROOT,
                HAS_COMPONENT,
                c"v",
                &ns1(10_000),
                -1,
                3,
            );
            let browses = server.count_browses();
            let mut client = peer.connected().await;
            let described = client.describe(&[variable]).await;
            (described.map(|all| all[0].base), browses.get())
        })
        .await
    });
    out
}

/// A walk ends past 16 links: a type 16 subtypes under Double has its base, with one
/// Browse for each link, and a type 17 under has none.
#[test]
fn a_walk_gives_the_base_at_16_links_and_none_at_17() {
    assert_eq!(walk_of(16), (Ok(Some(11)), 16));
    assert_eq!(walk_of(17).0, Ok(None));
}

/// A variable of `Structure` or `BaseDataType` has no base, and its describe sends no
/// Browse.
#[test]
fn describe_of_an_abstract_type_gives_no_base_and_browses_nothing() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
            for (index, data_type) in (0..).zip([STRUCTURE, BASE_DATA_TYPE]) {
                let node = ns1(7200 + index);
                server.add_variable(
                    &node,
                    &ROOT,
                    HAS_COMPONENT,
                    c"v",
                    &data_type,
                    -1,
                    3,
                );
            }
            let browses = server.count_browses();
            let mut client = peer.connected().await;
            let described = client.describe(&[ns1(7200), ns1(7201)]).await;
            (described, browses.get())
        })
        .await
    });
    assert_eq!(
        out,
        (Ok(vec![described(None, -1, 3), described(None, -1, 3)]), 0)
    );
}

/// The client keeps at most 1024 supertypes past a call: the 1024 types of 512
/// variables stay, so their next describe sends no Browse, and the 1026 types of 513
/// go at the end of the call, so their next describe sends each Browse again.
#[test]
fn describe_keeps_at_most_1024_supertypes_past_a_call() {
    let browses = |count| {
        let (out, _) = run(0, async move |node| {
            serve(&node, 4, async |mut peer| {
                let nodes = deep(peer.server, count);
                let browses = peer.server.count_browses();
                let mut client = peer.connected().await;
                let first = client.describe(&nodes).await.map(|_| browses.get());
                let kept = client.supertypes.len();
                let again = client.describe(&nodes).await.map(|_| browses.get());
                (first, kept, again)
            })
            .await
        });
        out
    };
    let (first, kept, again) = browses(512);
    assert_eq!((kept, again), (1024, first));
    let (first, kept, again) = browses(513);
    assert_eq!((kept, again), (0, first.map(|first| first * 2)));
}

/// Gives the unit code of a variable whose unit has the `unitId` `id` under
/// UN/CEFACT, for each of `ids`.
fn codes(ids: Vec<i32>) -> Result<Vec<Option<String>>, Status> {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.add_folder(&ROOT, &OBJECTS, ORGANIZES, c"root", c"Root");
            server.add_data_type(&DOUBLE, &BASE_DATA_TYPE, c"Double");
            let mut nodes = Vec::new();
            for (index, id) in (0..).zip(ids) {
                let node = ns1(7200 + index);
                server.add_variable(&node, &ROOT, HAS_COMPONENT, c"v", &DOUBLE, -1, 3);
                server.add_unit(&ns1(20_000 + index), &node, CEFACT, c"u", id);
                nodes.push(node);
            }
            let mut client = peer.connected().await;
            let described = client.describe(&nodes).await;
            described.map(|all| {
                all.into_iter()
                    .map(|d| d.unit.and_then(|unit| unit.code))
                    .collect()
            })
        })
        .await
    });
    out
}

/// A `unitId` is a code only as 2 or 3 bytes after its zero bytes, each a capital
/// letter or a digit.
#[test]
fn a_unit_code_that_is_not_2_or_3_capital_letters_or_digits_is_none() {
    let ids = vec![
        0x5031,
        -1,
        0,
        0x43,
        0x0043_2D4C,
        0x4345_4C4D,
        0x0063_656C,
        0x0043_656C,
    ];
    let mut expected = vec![None; ids.len()];
    expected[0] = Some("P1".to_owned());
    assert_eq!(codes(ids), Ok(expected));
}

/// On a server with a `MaxMessageSize` of 8192 bytes, describe and browse split so
/// that each answer fits one message: the units of 200 variables, the walk of 200
/// types, and a browse of 400 folders.
#[test]
fn each_answer_fits_one_message_of_the_session() {
    let small = |server: &Server| {
        server.set_limits(Limits {
            message: 8192,
            ..Limits::default()
        });
    };
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            small(peer.server);
            let nodes = deep(peer.server, 200);
            for (index, node) in (0..).zip(&nodes) {
                let unit = ns1(20_000 + index);
                peer.server.add_unit(&unit, node, CEFACT, c"m", 0x004D_5452);
            }
            let mut client = peer.connected().await;
            let described = client.describe(&nodes).await;
            client.close().await;
            described.map(|all| {
                all.iter()
                    .filter(|d| d.base == Some(11) && d.unit.is_some())
                    .count()
            })
        })
        .await
    });
    assert_eq!(out, Ok(200));
    let pages = pages(ALL, move |server| {
        small(server);
        folders(server, 400);
    });
    let references = pages.map(|pages| {
        let sizes: Vec<usize> =
            pages.iter().map(|page| page.references.len()).collect();
        (sizes.len() > 1, sizes.iter().sum::<usize>())
    });
    assert_eq!(references, Ok((true, 400)));
}

/// A page asks for as many references as fit one message that the client takes,
/// with each text at 256 bytes: on a server with a `MaxMessageSize` of 8192 bytes, a
/// browse of 400 folders gives pages of that count.
#[test]
fn a_page_asks_for_the_references_that_fit_one_message() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_limits(Limits {
                message: 8192,
                ..Limits::default()
            });
            folders(peer.server, 400);
            let mut client = peer.connected().await;
            let page = client.browse(&ROOT, ALL).await;
            let fixed = service::answer_bytes(Service::BROWSE) + Item::Result.bytes();
            let fits = (client.takes - fixed) / Item::Reference.bytes();
            client.close().await;
            (
                page.map(|page| page.map(|page| page.references.len())),
                fits,
            )
        })
        .await
    });
    let (page, fits) = out;
    assert!(fits > 1 && fits < 400, "{fits}");
    assert_eq!(page, Ok(Some(fits)));
}

/// Gives the answer of a Read of the Value of [`VALUE`] to `read`.
fn read_answer(read: impl FnOnce(&service::Answer) + Send + 'static) {
    let ((), _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let answer = client.serve(&Request::values(&[(&VALUE).into()])).await;
            read(&answer.expect("the read works"));
            client.close().await;
        })
        .await;
    });
}

#[test]
#[should_panic(
    expected = "invariant: a reader of [Service(2)] reads no answer of Service(3)"
)]
fn a_reader_of_an_answer_of_another_service_panics() {
    read_answer(|answer| {
        answer.target(0);
    });
}

#[test]
#[should_panic(expected = "invariant: a reader reads result 1 of 1 results")]
fn a_reader_of_a_result_past_the_count_panics() {
    read_answer(|answer| {
        answer.value(1);
    });
}

/// A late answer of a describe that timed out changes no later call.
#[test]
fn a_late_answer_of_a_timed_out_describe_does_not_change_the_next() {
    for seed in 0..32 {
        let (out, _) = run(seed, async |node| {
            serve(&node, 4, async |mut peer| {
                let nodes = variables(peer.server);
                let mut client = peer.connected().await;
                let first = client.describe(&nodes[..1]).await.map(|all| all.len());
                peer.gate.close();
                let timed = client.describe(&nodes).await.map(|_| ());
                peer.gate.open();
                let page = client.browse(&ROOT, per(2)).await;
                let page = page.map(|page| page.map(|page| page.references.len()));
                let after = client.describe(&nodes).await;
                let again = client.describe(&nodes).await;
                (first, timed, page, after, again)
            })
            .await
        });
        let (first, timed, page, after, again) = out;
        let expected = vec![
            described(Some(11), -1, 3),
            described(Some(11), -1, 3),
            described(Some(6), -1, 3),
            described(None, -1, 3),
            described(None, -1, 3),
            described(None, -1, 3),
            described(Some(11), 1, 1),
        ];
        assert_eq!(first, Ok(1), "{seed}");
        assert_eq!(timed, Err(Status::BAD_TIMEOUT), "{seed}");
        assert_eq!(page, Ok(Some(2)), "{seed}");
        assert_eq!(after, Ok(expected), "{seed}");
        assert_eq!(again, after, "{seed}");
    }
}

/// A describe of no nodes gives none, and a describe of a node twice gives its
/// description twice, with an unknown node and limits of 1 on each service.
#[test]
fn describe_of_no_nodes_and_of_duplicates() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let server = peer.server;
            server.set_per_browse(1, 1);
            server.set_per_read(1);
            let nodes = deep(server, 2);
            server.add_unit(&ns1(7300), &nodes[0], CEFACT, c"m", 0x004D_5452);
            let mut client = peer.connected().await;
            let none = client.describe(&[]).await;
            let list = [
                nodes[0].clone(),
                nodes[1].clone(),
                nodes[0].clone(),
                ns1(9999),
            ];
            let twice = client.describe(&list).await;
            (none, twice)
        })
        .await
    });
    let metre = || Description {
        unit: Some(Unit {
            text: Some("m".to_owned()),
            code: Some("MTR".to_owned()),
        }),
        ..described(Some(11), -1, 3)
    };
    assert_eq!(
        out,
        (
            Ok(vec![]),
            Ok(vec![
                metre(),
                described(Some(11), -1, 3),
                metre(),
                Description::default()
            ])
        )
    );
}

/// Each [`Service`] names the request and answer types of its index in `shim.c`.
#[test]
fn each_service_has_its_request_and_answer_type() {
    let names = [
        Service::BROWSE,
        Service::BROWSE_NEXT,
        Service::TRANSLATE,
        Service::READ,
    ]
    .map(|service| {
        [false, true].map(|answer| {
            // SAFETY: it reads nothing.
            let raw = unsafe { ffi::shim_service_type(service, answer) };
            // SAFETY: each type of `UA_TYPES` is static.
            unsafe { &*raw }.name()
        })
    });
    assert_eq!(
        names,
        [
            ["BrowseRequest", "BrowseResponse"],
            ["BrowseNextRequest", "BrowseNextResponse"],
            [
                "TranslateBrowsePathsToNodeIdsRequest",
                "TranslateBrowsePathsToNodeIdsResponse"
            ],
            ["ReadRequest", "ReadResponse"],
        ]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn the_code_of_each_unit_id_of_capital_letters_or_digits_is_its_text(
        code in "[A-Z0-9]{2,3}",
    ) {
        let id = code.bytes().fold(0, |id, byte| id << 8 | i32::from(byte));
        prop_assert_eq!(codes(vec![id]), Ok(vec![Some(code)]));
    }
}

/// A browse whose request is the most bytes that the client sends goes to the
/// server, and one a byte larger gives `BadRequestTooLarge`.
#[test]
fn a_browse_of_the_most_bytes_goes_to_the_server() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_limits(Limits {
                message: 8192,
                ..Limits::default()
            });
            let mut client = peer.connected().await;
            let bytes = |length| {
                let node = long(length);
                Request::browse(&[(&node).into()], 0, false, 0, NonZeroU32::MIN)
                    .bytes(client.token)
            };
            let length = client.sends - bytes(1) + 1;
            assert_eq!(bytes(length), client.sends);
            let most = client.browse(&long(length), ALL).await.map(|_| ());
            let past = client.browse(&long(length + 1), ALL).await.map(|_| ());
            client.close().await;
            (most, past)
        })
        .await
    });
    assert_eq!(out, (Ok(()), Err(Status::BAD_REQUEST_TOO_LARGE)));
}

/// A browse whose request does not fit one message sends nothing and gives
/// `BadRequestTooLarge`, so the session stays and the next browse and read work.
#[test]
fn a_browse_past_one_message_keeps_the_session() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_limits(Limits {
                message: 8192,
                ..Limits::default()
            });
            folders(peer.server, 1);
            let mut client = peer.connected().await;
            let large = client.browse(&long(9000), ALL).await.map(|_| ());
            let next = client.browse(&ROOT, ALL).await;
            let next = next.map(|page| page.map(|page| page.references.len()));
            let read = client.read_one(&VALUE).await.map(|_| ());
            client.close().await;
            (large, next, read)
        })
        .await
    });
    assert_eq!(
        out,
        (Err(Status::BAD_REQUEST_TOO_LARGE), Ok(Some(1)), Ok(()))
    );
}
