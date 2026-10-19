//! The examples of the body of each connector kind of `node::kinds()`, as the check of
//! its kind reads them.

use connector::kind::Table;
use document::diagnostic::Diagnostic;
use document::schema::{Body, Presence};
use document::testing::{at, change, misses, needs, sites, without};
use document::value::{self, Value};
use document::{Attribute, Document, Source};

/// Each kind by name, with its body and each example of the body.
type Kinds = Vec<(&'static str, &'static Body, Vec<Document>)>;

/// The table of `node::kinds()`, with each of its kinds.
fn kinds() -> (Table, Kinds) {
    let table = node::kinds();
    let kinds = table
        .schemas()
        .map(|(name, body)| {
            let examples = body
                .examples
                .iter()
                .map(|text| {
                    config_hcl::read(Source(0), text).expect("an example is HCL")
                })
                .collect();
            (name, body, examples)
        })
        .collect();
    (table, kinds)
}

/// The problems that the check of `kind` finds in `config`.
fn problems(table: &Table, kind: &str, config: &Document) -> Vec<Diagnostic> {
    table.check(kind, None, config).err().unwrap_or_default()
}

#[test]
fn each_kind_takes_each_example_of_its_body() {
    let (table, kinds) = kinds();
    for (kind, _, examples) in &kinds {
        for example in examples {
            assert_eq!(problems(&table, kind, example), [], "{kind}");
        }
    }
}

#[test]
fn each_kind_takes_each_example_without_an_attribute_that_it_does_not_need() {
    let (table, kinds) = kinds();
    let mut removed = 0;
    for (kind, outer, examples) in &kinds {
        for example in examples {
            for (body, path) in sites(outer, example) {
                for attribute in body.attributes {
                    if needs(body, attribute.name, at(example, &path)) {
                        continue;
                    }
                    let Some(example) = without(example.clone(), &path, attribute.name)
                    else {
                        continue;
                    };
                    let problems = problems(&table, kind, &example);
                    assert_eq!(problems, [], "{kind}: {}", attribute.name);
                    removed += 1;
                }
            }
        }
    }
    assert!(removed > 0);
}

/// Each attribute with a default gives its value with no other attribute.
#[test]
fn each_kind_takes_each_example_without_each_attribute_with_a_default() {
    let (table, kinds) = kinds();
    let mut removed = 0;
    for (kind, outer, examples) in &kinds {
        for example in examples {
            for (body, path) in sites(outer, example) {
                let defaults: Vec<_> = body
                    .attributes
                    .iter()
                    .filter(|key| matches!(key.presence, Presence::Default(_)))
                    .map(|key| key.name)
                    .collect();
                let example = change(example.clone(), &path, |attributes| {
                    attributes.retain(|attribute| !defaults.contains(&&*attribute.key));
                });
                let problems = problems(&table, kind, &example);
                assert_eq!(problems, [], "{kind}: {defaults:?}");
                removed += defaults.len();
            }
        }
    }
    assert!(removed > 0);
}

#[test]
fn an_example_without_an_attribute_that_it_needs_misses_that_attribute() {
    let (table, kinds) = kinds();
    let mut removed = 0;
    for (kind, outer, examples) in &kinds {
        for example in examples {
            for (body, path) in sites(outer, example) {
                for attribute in body.attributes {
                    if !needs(body, attribute.name, at(example, &path)) {
                        continue;
                    }
                    let Some(example) = without(example.clone(), &path, attribute.name)
                    else {
                        continue;
                    };
                    let problems = problems(&table, kind, &example);
                    let missed = problems
                        .iter()
                        .any(|problem| misses(problem, attribute.name));
                    assert!(missed, "{kind}: {}: {problems:?}", attribute.name);
                    removed += 1;
                }
            }
        }
    }
    assert!(removed > 0);
}

#[test]
fn an_example_with_an_attribute_that_its_body_does_not_state_gets_one_problem() {
    let (table, kinds) = kinds();
    for (kind, outer, examples) in &kinds {
        for example in examples {
            for (_, path) in sites(outer, example) {
                let example = change(example.clone(), &path, |attributes| {
                    attributes.push(Attribute {
                        key: "colour".into(),
                        key_span: None,
                        value: Value {
                            kind: value::Kind::String("x".into()),
                            span: None,
                        },
                    });
                });
                let problems = problems(&table, kind, &example);
                let unknown: Vec<_> = problems
                    .iter()
                    .filter(|problem| {
                        problem.code.as_str() == "document.unknown-attribute"
                    })
                    .collect();
                assert_eq!(unknown.len(), 1, "{kind}: {path:?}: {problems:?}");
                let message = &unknown[0].message;
                assert!(
                    message.starts_with("`colour` is not an attribute of "),
                    "{message}"
                );
            }
        }
    }
}

/// So a kind that states no example, an attribute or block that no example holds, or an
/// attribute of an example that its body does not state fails here, and `foundation
/// docs` gives no body that a check does not take.
#[test]
fn each_attribute_and_block_of_a_body_is_in_an_example() {
    let (_, kinds) = kinds();
    let mut reached: Vec<(&Body, Vec<&Document>)> = Vec::new();
    for (kind, outer, examples) in &kinds {
        assert!(!examples.is_empty(), "{kind}");
        for example in examples {
            for (body, path) in sites(outer, example) {
                let held = at(example, &path);
                match reached.iter_mut().find(|(seen, _)| *seen == body) {
                    Some((_, helds)) => helds.push(held),
                    None => reached.push((body, vec![held])),
                }
            }
        }
    }
    for (body, helds) in reached {
        for attribute in body.attributes {
            let found = helds
                .iter()
                .any(|held| held.attributes.get(attribute.name).is_some());
            assert!(found, "{}: {}", body.summary, attribute.name);
        }
        for attribute in helds.iter().flat_map(|held| held.attributes.iter()) {
            let stated = body
                .attributes
                .iter()
                .any(|key| key.name == &*attribute.key);
            assert!(stated, "{}: {}", body.summary, attribute.key);
        }
        for inner in body.blocks {
            let found = helds.iter().any(|held| {
                held.blocks
                    .iter()
                    .any(|block| &*block.keyword == inner.keyword)
            });
            assert!(found, "{}: {}", body.summary, inner.keyword);
        }
    }
}

#[test]
fn each_name_of_an_unless_is_an_attribute_of_its_body() {
    let (_, kinds) = kinds();
    let mut names = 0;
    for (kind, outer, examples) in &kinds {
        for example in examples {
            for (body, _) in sites(outer, example) {
                for attribute in body.attributes {
                    let Presence::Unless(others) = attribute.presence else {
                        continue;
                    };
                    assert!(!others.is_empty(), "{kind}: {}", attribute.name);
                    for other in others {
                        let stated =
                            body.attributes.iter().any(|key| key.name == *other);
                        assert!(stated, "{kind}: {}: {other}", attribute.name);
                        names += 1;
                    }
                }
            }
        }
    }
    assert!(names > 0);
}

/// The `read` block of `opcua` states the count that its check takes.
#[test]
fn opcua_takes_no_read_block_as_its_body_states() {
    let (table, kinds) = kinds();
    let (_, body, _) = kinds
        .iter()
        .find(|(kind, ..)| *kind == "opcua")
        .expect("opcua");
    let [read] = body.blocks else {
        panic!("one block of opcua");
    };
    assert_eq!(
        read.forms[0].summary,
        "The node of one channel. Zero or more."
    );
    let none = "address = \"opc.tcp://plc:4840\"\nrate    = \"10Hz\"\n";
    let none = config_hcl::read(Source(0), none).expect("HCL");
    assert_eq!(problems(&table, "opcua", &none), []);
}
