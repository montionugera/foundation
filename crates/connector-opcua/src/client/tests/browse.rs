use std::ffi::CStr;

use proptest::prelude::*;
use types::hash::{Map, Set};

use super::{
    Client, READS, RELAY, Relay, VALUE, drop_reads, long, recount_of, relayed, run,
    serve,
};
use crate::client::browse::{Class, Description, Page, Reference, Unit, base, code_of};
use crate::ffi::Status;
use crate::node::{Key, Name, Node};
use crate::server::{Limits, Server};

const BAD_CONTINUATION_POINT_INVALID: Status = Status(0x804A_0000);

/// The binary encoding of a Browse response, a `NodeId` of 4 bytes.
const BROWSE_RESPONSE: [u8; 4] = [0x01, 0x00, 0x12, 0x02];

/// The binary encoding of a `TranslateBrowsePathsToNodeIds` response.
const TRANSLATE_RESPONSE: [u8; 4] = [0x01, 0x00, 0x2D, 0x02];

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

fn named(name: &str) -> Node {
    Node {
        namespace: 1,
        key: Key::Name(Name::new(name).expect("a name that is not empty")),
    }
}

fn reference(
    node: Option<Node>,
    kind: u32,
    name: &str,
    display: &str,
    class: Class,
) -> Reference {
    Reference {
        node,
        name: name.to_owned(),
        display: display.to_owned(),
        class,
        kind: Some(ns0(kind)),
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
    most: u32,
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
    let pages = pages(0, |server| {
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
            client.browse(&ns1(7001), 0).await
        })
        .await
    });
    assert_eq!(page, Ok(None));
}

/// Pages of at most 2 references give the references of one page, in order.
#[test]
fn a_browse_of_two_per_page_over_5_gives_3_pages_in_order() {
    let whole = pages(0, |server| folders(server, 5)).expect("the browse works");
    let paged = pages(2, |server| folders(server, 5)).expect("the browse works");
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
            let page = client.browse(&ROOT, 2).await.expect("the browse works");
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
            let (parent, data_type, node) =
                (ns1(7100 + index), ns1(7150 + index), ns1(7200 + index));
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
                .delete_reference(&DOUBLE, HAS_SUBTYPE, &ns1(7100));
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
/// does not fit fails before it sends. A fact of an unknown node is `None`.
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
            let after = client.describe(&[long(1000)]).await;
            client.close().await;
            (fits, past, after)
        })
        .await
    });
    let none = || Description::default();
    assert_eq!(
        out,
        (
            Ok(vec![none(), none(), none()]),
            Err(Status::BAD_REQUEST_TOO_LARGE),
            Ok(vec![none()])
        )
    );
}

/// Each call times out 5 s after it starts while the server does not run.
#[test]
fn each_call_times_out_when_the_server_does_not_answer() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            folders(peer.server, 5);
            let mut client = peer.connected().await;
            let point = client.browse(&ROOT, 2).await.expect("the browse works");
            let point = point.and_then(|page| page.next).expect("a point");
            peer.gate.close();
            let start = peer.clock.now();
            let browse = client.browse(&ROOT, 0).await.map(|_| ());
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
/// next call the status of a client with no connection.
#[test]
fn a_call_after_the_server_closes_fails_with_its_status() {
    let (out, _) = run(0, async |node| {
        let mut client = serve(&node, 4, async |mut peer| peer.connected().await).await;
        let browse = client.browse(&ROOT, 0).await.map(|_| ());
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

/// While [`READS`] dropped reads stay in flight, each call fails
/// at the end of its wait and sends nothing.
#[test]
fn each_call_at_the_bound_of_reads_in_flight_fails_with_too_many_operations() {
    let (out, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            peer.gate.close();
            drop_reads(&mut client, READS).await;
            let browse = client.browse(&ROOT, 0).await.map(|_| ());
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

/// Gives the answer of `call` through a relay that gives each response of the type
/// `kind` no results.
fn emptied<T: Send + 'static>(
    kind: [u8; 4],
    call: impl AsyncFnOnce(&mut Client) -> T + Send + 'static,
) -> T {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            folders(peer.server, 2);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let rewrite = |message: &mut Vec<u8>| {
                if message.starts_with(b"MSG") && message[24..28] == kind {
                    recount_of(message, 0);
                }
            };
            relayed(&node, &Relay::default(), rewrite, async {
                let mut client = peer.connect(&url).await.expect("the client connects");
                let out = call(&mut client).await;
                client.close().await;
                out
            })
            .await
        })
        .await
    });
    out
}

/// An answer with more or fewer results than the request asked for fails the call.
#[test]
fn an_answer_with_the_wrong_count_of_results_fails_the_call() {
    let browse = emptied(BROWSE_RESPONSE, async |client| {
        client.browse(&ROOT, 0).await.map(|_| ())
    });
    let describe = emptied(TRANSLATE_RESPONSE, async |client| {
        client.describe(&[VALUE]).await.map(|_| ())
    });
    let wrong = Err(Status::BAD_UNEXPECTED_ERROR);
    assert_eq!((browse, describe), (wrong, wrong));
}

#[test]
fn a_walk_gives_the_first_unbrowsed_type_and_ends_past_16_links() {
    let (mut supertypes, none) = (Map::default(), Set::default());
    assert_eq!(base(&supertypes, &none, &ns1(1)), Err(ns1(1)));
    supertypes.insert(ns1(1), Some(ns1(2)));
    assert_eq!(base(&supertypes, &none, &ns1(1)), Err(ns1(2)));
    let failed = Set::from_iter([ns1(2)]);
    assert_eq!(base(&supertypes, &failed, &ns1(1)), Ok(None));
    supertypes.insert(ns1(2), None);
    assert_eq!(base(&supertypes, &none, &ns1(1)), Ok(None));
    for id in 1..=16 {
        supertypes.insert(ns1(100 + id), Some(ns1(101 + id)));
    }
    supertypes.insert(ns1(117), Some(ns0(11)));
    assert_eq!(base(&supertypes, &none, &ns1(102)), Ok(Some(11)));
    assert_eq!(base(&supertypes, &none, &ns1(101)), Ok(None));
}

#[test]
fn a_walk_gives_each_built_in_type_and_none_for_each_abstract_type() {
    let (supertypes, none) = (Map::default(), Set::default());
    let bases: Vec<Option<u32>> = (1..=30)
        .map(|id| base(&supertypes, &none, &ns0(id)).unwrap_or(Some(0)))
        .collect();
    let expected: Vec<Option<u32>> = (1..=30)
        .map(|id| match id {
            22 | 24 | 26..=28 => None,
            29 => Some(6),
            30 => Some(0),
            id => Some(id),
        })
        .collect();
    assert_eq!(bases, expected);
    assert_eq!(base(&supertypes, &none, &ns1(11)), Err(ns1(11)));
}

#[test]
fn a_unit_code_that_is_not_2_or_3_capital_letters_or_digits_is_none() {
    assert_eq!(code_of(0x5031), Some("P1".to_owned()));
    for id in [
        -1,
        0,
        0x0043_2D4C,
        0x4345_4C4D,
        0x43,
        0x0063_656C,
        0x0043_656C,
    ] {
        assert_eq!(code_of(id), None, "{id:#x}");
    }
}

proptest! {
    #[test]
    fn the_code_of_each_unit_id_of_capital_letters_or_digits_is_its_text(
        code in "[A-Z0-9]{2,3}",
    ) {
        let id = code.bytes().fold(0, |id, byte| id << 8 | i32::from(byte));
        prop_assert_eq!(code_of(id), Some(code));
    }
}
