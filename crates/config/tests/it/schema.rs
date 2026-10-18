//! The examples of each body of `config::SCHEMA`, as `check` reads them.

use std::collections::BTreeSet;

use config::SCHEMA;
use connector::kind::Table;
use document::diagnostic::Diagnostic;
use document::schema::{Body, Presence};
use document::value::{self, Value};
use document::{Attribute, Block, Document, Map, Source};

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

/// The first block of `example`, which is of `body`, and each block inside it, with
/// the body of its form and its path of block indexes from the document.
fn sites(body: &'static Body, example: &Document) -> Vec<(&'static Body, Vec<usize>)> {
    let first = example.blocks.first().expect("an example holds a block");
    let mut sites = Vec::new();
    walk(body, first, vec![0], &mut sites);
    sites
}

fn walk(
    body: &'static Body,
    block: &Block,
    path: Vec<usize>,
    sites: &mut Vec<(&'static Body, Vec<usize>)>,
) {
    for (index, inner) in block.body.blocks.iter().enumerate() {
        let of = body
            .blocks
            .iter()
            .find(|of| of.keyword == &*inner.keyword)
            .expect("a block of the body");
        let [form] = of.forms else {
            panic!("a block inside a body has one form: {}", of.keyword);
        };
        let mut at = path.clone();
        at.push(index);
        walk(form, inner, at, sites);
    }
    sites.push((body, path));
}

/// The block of `document` at `path`.
fn at<'a>(document: &'a Document, path: &[usize]) -> &'a Block {
    let (first, rest) = path.split_first().expect("a path holds an index");
    rest.iter().fold(&document.blocks[*first], |block, index| {
        &block.body.blocks[*index]
    })
}

/// `document` with the attributes of its block at `path` changed by `change`.
fn change(
    mut document: Document,
    path: &[usize],
    change: impl FnOnce(&mut Vec<Attribute>),
) -> Document {
    let (first, rest) = path.split_first().expect("a path holds an index");
    let block = rest
        .iter()
        .fold(&mut document.blocks[*first], |block, index| {
            &mut block.body.blocks[*index]
        });
    let mut attributes = block.body.attributes.iter().cloned().collect();
    change(&mut attributes);
    block.body.attributes = Map::new(attributes).expect("unique keys");
    document
}

/// `document` without the attribute `key` in its block at `path`, or `None` when the
/// block has none.
fn without(document: Document, path: &[usize], key: &str) -> Option<Document> {
    at(&document, path).body.attributes.get(key)?;
    Some(change(document, path, |attributes| {
        attributes.retain(|attribute| &*attribute.key != key);
    }))
}

/// Whether `block` of `body` needs `key`: its absence gives no value, or gives a
/// diagnostic.
fn needs(body: &Body, key: &str, block: &Block) -> bool {
    presence(body, key).needed(&block.body.attributes)
}

fn presence(body: &Body, key: &str) -> Presence {
    body.attributes
        .iter()
        .find(|attribute| attribute.name == key)
        .expect("a key of the body")
        .presence
}

/// Whether `problem` is a missing attribute that names `key`.
fn misses(problem: &Diagnostic, key: &str) -> bool {
    problem.code.as_str() == "document.missing-attribute"
        && problem
            .message
            .split_once(" has no ")
            .is_some_and(|(_, keys)| keys.contains(&format!("`{key}`")))
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
    for outer in bodies() {
        for example in examples(outer) {
            for (body, path) in sites(outer, &example) {
                for attribute in body.attributes {
                    if needs(body, attribute.name, at(&example, &path)) {
                        continue;
                    }
                    let Some(example) = without(example.clone(), &path, attribute.name)
                    else {
                        continue;
                    };
                    assert_eq!(problems(example), [], "{}", attribute.name);
                    removed += 1;
                }
            }
        }
    }
    assert!(removed > 0);
}

/// Each attribute with a default gives its value with no other attribute.
#[test]
fn check_takes_each_example_without_each_attribute_with_a_default() {
    let mut removed = 0;
    for outer in bodies() {
        for example in examples(outer) {
            for (body, path) in sites(outer, &example) {
                let defaults: Vec<_> = body
                    .attributes
                    .iter()
                    .filter(|key| matches!(key.presence, Presence::Default(_)))
                    .map(|key| key.name)
                    .collect();
                let example = change(example.clone(), &path, |attributes| {
                    attributes.retain(|attribute| !defaults.contains(&&*attribute.key));
                });
                assert_eq!(problems(example), [], "{}: {defaults:?}", body.summary);
                removed += defaults.len();
            }
        }
    }
    assert!(removed > 0);
}

#[test]
fn each_name_of_an_unless_is_an_attribute_of_its_body() {
    let mut names = 0;
    for outer in bodies() {
        for example in examples(outer) {
            for (body, _) in sites(outer, &example) {
                for attribute in body.attributes {
                    let Presence::Unless(others) = attribute.presence else {
                        continue;
                    };
                    assert!(!others.is_empty(), "{}", attribute.name);
                    for other in others {
                        let stated =
                            body.attributes.iter().any(|key| key.name == *other);
                        assert!(stated, "{}: {other}", attribute.name);
                        names += 1;
                    }
                }
            }
        }
    }
    assert!(names > 0);
}

/// The `kind` of a block with more than one form picks its form, so it is the case of
/// the next test.
#[test]
fn an_example_without_an_attribute_that_it_needs_misses_that_attribute() {
    let mut removed = 0;
    for block in SCHEMA {
        for outer in block.forms {
            for example in examples(outer) {
                for (body, path) in sites(outer, &example) {
                    let picks = path.len() == 1 && block.forms.len() > 1;
                    for attribute in body.attributes {
                        if !needs(body, attribute.name, at(&example, &path))
                            || picks && attribute.name == "kind"
                        {
                            continue;
                        }
                        let Some(example) =
                            without(example.clone(), &path, attribute.name)
                        else {
                            continue;
                        };
                        let problems = problems(example);
                        let missed = problems
                            .iter()
                            .any(|problem| misses(problem, attribute.name));
                        assert!(missed, "{}: {problems:?}", attribute.name);
                        removed += 1;
                    }
                }
            }
        }
    }
    assert!(removed > 0);
}

/// A block without the `kind` that its form needs reads as the form whose `kind` has a
/// default: it misses each attribute that that form needs, and each of its other
/// attributes is unknown, in any order.
#[test]
fn an_example_without_its_kind_reads_as_the_form_with_a_default_kind() {
    let mut removed = 0;
    for block in SCHEMA.iter().filter(|block| block.forms.len() > 1) {
        let picked = block
            .forms
            .iter()
            .find(|form| matches!(presence(form, "kind"), Presence::Default(_)))
            .expect("a form whose kind has a default");
        for body in block.forms {
            if presence(body, "kind") != Presence::Required {
                continue;
            }
            for example in examples(body) {
                let example = without(example, &[0], "kind").expect("a kind");
                let first = at(&example, &[0]);
                let missing = picked
                    .attributes
                    .iter()
                    .filter(|key| needs(picked, key.name, first))
                    .filter(|key| first.body.attributes.get(key.name).is_none())
                    .map(|key| {
                        let message = format!(
                            "the `{}` block has no `{}`",
                            block.keyword, key.name
                        );
                        ("document.missing-attribute", message)
                    });
                let unknown = first
                    .body
                    .attributes
                    .iter()
                    .filter(|held| {
                        picked.attributes.iter().all(|key| key.name != &*held.key)
                    })
                    .map(|held| {
                        let message = format!(
                            "`{}` is not an attribute of the `{}` block",
                            held.key, block.keyword
                        );
                        ("document.unknown-attribute", message)
                    });
                let expected: BTreeSet<_> = missing.chain(unknown).collect();
                let found: BTreeSet<_> = problems(example)
                    .into_iter()
                    .map(|problem| (problem.code.as_str(), problem.message))
                    .collect();
                assert_eq!(found, expected, "{}", body.summary);
                removed += 1;
            }
        }
    }
    assert!(removed > 0);
}

#[test]
fn an_example_with_an_attribute_that_its_body_does_not_state_gets_one_problem() {
    for outer in bodies() {
        for example in examples(outer) {
            for (body, path) in sites(outer, &example) {
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
                let problems = problems(example);
                let unknown: Vec<_> = problems
                    .iter()
                    .filter(|problem| {
                        problem.code.as_str() == "document.unknown-attribute"
                    })
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
}

/// Each body that an example reaches: each core body, and each body of a block inside
/// one.
#[test]
fn each_attribute_and_block_of_a_body_is_in_an_example() {
    let mut reached: Vec<(&Body, Vec<&Block>)> = Vec::new();
    let examples: Vec<_> = bodies()
        .map(|body| {
            assert!(!body.examples.is_empty(), "{}", body.summary);
            (body, examples(body).collect::<Vec<_>>())
        })
        .collect();
    for (outer, examples) in &examples {
        for example in examples {
            for (body, path) in sites(outer, example) {
                let block = at(example, &path);
                match reached.iter_mut().find(|(seen, _)| *seen == body) {
                    Some((_, blocks)) => blocks.push(block),
                    None => reached.push((body, vec![block])),
                }
            }
        }
    }
    for (body, blocks) in reached {
        for attribute in body.attributes {
            let found = blocks
                .iter()
                .any(|block| block.body.attributes.get(attribute.name).is_some());
            assert!(found, "{}: {}", body.summary, attribute.name);
        }
        for inner in body.blocks {
            let found = blocks.iter().any(|block| {
                block
                    .body
                    .blocks
                    .iter()
                    .any(|held| &*held.keyword == inner.keyword)
            });
            assert!(found, "{}: {}", body.summary, inner.keyword);
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
