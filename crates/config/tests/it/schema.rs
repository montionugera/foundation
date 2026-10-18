//! The examples of each body of `config::SCHEMA`, as `check` reads them.

use config::SCHEMA;
use connector::kind::Table;
use document::diagnostic::Diagnostic;
use document::schema::{Body, Presence};
use document::value::{self, Value};
use document::{Attribute, Document, Map, Source};

fn kinds() -> Table {
    Table::new().with("influx", connector_influx::Kind::default())
}

/// Each form of each core block.
fn bodies() -> impl Iterator<Item = &'static Body> {
    SCHEMA.iter().flat_map(|block| block.forms)
}

/// Each example of `body`, as a Document.
fn examples(body: &Body) -> impl Iterator<Item = Document> {
    body.examples
        .iter()
        .map(|text| config_hcl::read(Source(0), text).expect("an example is HCL"))
}

/// The problems that `check` finds in `document`.
fn problems(document: Document) -> Vec<Diagnostic> {
    config::check(&[document], &kinds())
        .err()
        .unwrap_or_default()
}

/// `document` with the attributes of its first block, the one that the example is of,
/// changed by `change`.
fn change(
    mut document: Document,
    change: impl FnOnce(&mut Vec<Attribute>),
) -> Document {
    let block = document
        .blocks
        .first_mut()
        .expect("an example holds a block");
    let mut attributes = block.body.attributes.iter().cloned().collect();
    change(&mut attributes);
    block.body.attributes = Map::new(attributes).expect("unique keys");
    document
}

/// `document` without the attribute `key` in its first block, or `None` when the block
/// has none.
fn without(document: Document, key: &str) -> Option<Document> {
    document.blocks.first()?.body.attributes.get(key)?;
    Some(change(document, |attributes| {
        attributes.retain(|attribute| &*attribute.key != key);
    }))
}

/// Whether `example` needs `key` of `body`: its absence gives no value, or gives a
/// diagnostic.
fn needs(body: &Body, key: &str, example: &Document) -> bool {
    let first = example.blocks.first().expect("an example holds a block");
    let attribute = body
        .attributes
        .iter()
        .find(|attribute| attribute.name == key)
        .expect("a key of the body");
    match attribute.presence {
        Presence::Required => true,
        Presence::Unless(others) => !others
            .iter()
            .all(|other| first.body.attributes.get(other).is_some()),
        Presence::Default(_) | Presence::Optional => false,
    }
}

fn codes(problems: &[Diagnostic]) -> Vec<&'static str> {
    problems
        .iter()
        .map(|problem| problem.code.as_str())
        .collect()
}

#[test]
fn check_takes_each_example() {
    for body in bodies() {
        for example in examples(body) {
            assert_eq!(problems(example), [], "{}", body.summary);
        }
    }
}

#[test]
fn check_takes_each_example_without_an_attribute_that_it_does_not_need() {
    let mut removed = 0;
    for body in bodies() {
        for attribute in body.attributes {
            for example in examples(body) {
                if needs(body, attribute.name, &example) {
                    continue;
                }
                let Some(example) = without(example, attribute.name) else {
                    continue;
                };
                assert_eq!(problems(example), [], "{}", attribute.name);
                removed += 1;
            }
        }
    }
    assert!(removed > 0);
}

#[test]
fn an_example_without_an_attribute_that_it_needs_misses_an_attribute() {
    let mut removed = 0;
    for body in bodies() {
        for attribute in body.attributes {
            for example in examples(body) {
                if !needs(body, attribute.name, &example) {
                    continue;
                }
                let Some(example) = without(example, attribute.name) else {
                    continue;
                };
                let codes = codes(&problems(example));
                assert!(
                    codes.contains(&"document.missing-attribute"),
                    "{}: {codes:?}",
                    attribute.name,
                );
                removed += 1;
            }
        }
    }
    assert!(removed > 0);
}

#[test]
fn an_example_with_an_attribute_that_its_body_does_not_state_gets_one_problem() {
    for body in bodies() {
        for example in examples(body) {
            let example = change(example, |attributes| {
                attributes.push(Attribute {
                    key: "colour".into(),
                    key_span: None,
                    value: Value {
                        kind: value::Kind::String("x".into()),
                        span: None,
                    },
                });
            });
            let problems = problems(example);
            let unknown: Vec<_> = problems
                .iter()
                .filter(|problem| problem.code.as_str() == "document.unknown-attribute")
                .collect();
            assert_eq!(unknown.len(), 1, "{}: {problems:?}", body.summary);
            let message = &unknown[0].message;
            assert!(
                message.starts_with("`colour` is not an attribute of "),
                "{message}"
            );
        }
    }
}

#[test]
fn each_attribute_and_block_of_a_body_is_in_an_example() {
    for body in bodies() {
        assert!(!body.examples.is_empty(), "{}", body.summary);
        let firsts: Vec<_> = examples(body)
            .map(|example| example.blocks.into_iter().next().expect("a block"))
            .collect();
        for attribute in body.attributes {
            let found = firsts
                .iter()
                .any(|first| first.body.attributes.get(attribute.name).is_some());
            assert!(found, "{}: {}", body.summary, attribute.name);
        }
        for block in body.blocks {
            let found = firsts.iter().any(|first| {
                first
                    .body
                    .blocks
                    .iter()
                    .any(|inner| &*inner.keyword == block.keyword)
            });
            assert!(found, "{}: {}", body.summary, block.keyword);
        }
    }
}

/// Of a block with more than one form, the first block of an example holds only
/// attributes that its form states.
#[test]
fn the_first_block_of_each_example_is_of_its_block_and_form() {
    for block in SCHEMA {
        for body in block.forms {
            for example in examples(body) {
                let first = example.blocks.first().expect("a block");
                assert_eq!(&*first.keyword, block.keyword, "{}", body.summary);
                if block.forms.len() == 1 {
                    continue;
                }
                for attribute in first.body.attributes.iter() {
                    let stated = body
                        .attributes
                        .iter()
                        .any(|key| key.name == &*attribute.key);
                    assert!(stated, "{}: {}", body.summary, attribute.key);
                }
            }
        }
    }
}
