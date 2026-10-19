//! The examples of the body of each connector kind of `node::kinds()`, as the check of
//! its kind reads them.

use connector::kind::Table;
use document::diagnostic::Diagnostic;
use document::schema::{Body, Presence};
use document::value::{self, Value};
use document::{Attribute, Document, Map, Source};

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

/// The body of `example`, which is of `body`, and each block inside it, with the body
/// of its form and its path of block indexes from the example.
fn sites(body: &'static Body, example: &Document) -> Vec<(&'static Body, Vec<usize>)> {
    let mut sites = Vec::new();
    walk(body, example, Vec::new(), &mut sites);
    sites
}

fn walk(
    body: &'static Body,
    document: &Document,
    path: Vec<usize>,
    sites: &mut Vec<(&'static Body, Vec<usize>)>,
) {
    for (index, inner) in document.blocks.iter().enumerate() {
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
        walk(form, &inner.body, at, sites);
    }
    sites.push((body, path));
}

/// The body of `document` at `path`.
fn at<'a>(document: &'a Document, path: &[usize]) -> &'a Document {
    path.iter()
        .fold(document, |body, index| &body.blocks[*index].body)
}

/// `document` with the attributes of its body at `path` changed by `change`.
fn change(
    mut document: Document,
    path: &[usize],
    change: impl FnOnce(&mut Vec<Attribute>),
) -> Document {
    let body = path
        .iter()
        .fold(&mut document, |body, index| &mut body.blocks[*index].body);
    let mut attributes = body.attributes.iter().cloned().collect();
    change(&mut attributes);
    body.attributes = Map::new(attributes).expect("unique keys");
    document
}

/// `document` without the attribute `key` in its body at `path`, or `None` when the
/// body has none.
fn without(document: Document, path: &[usize], key: &str) -> Option<Document> {
    at(&document, path).attributes.get(key)?;
    Some(change(document, path, |attributes| {
        attributes.retain(|attribute| &*attribute.key != key);
    }))
}

/// Whether `held`, of `body`, needs `key`: its absence gives a diagnostic.
fn needs(body: &Body, key: &str, held: &Document) -> bool {
    body.attributes
        .iter()
        .find(|attribute| attribute.name == key)
        .expect("a key of the body")
        .presence
        .needed(&held.attributes)
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

/// So a kind that states no example, or an attribute or block that no example holds,
/// fails here, and `foundation docs` gives no body that a check does not take.
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
