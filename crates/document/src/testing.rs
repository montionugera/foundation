//! The walk of an example over the body that states it, for the tests of each crate
//! that states a body.

use crate::diagnostic::Diagnostic;
use crate::schema::Body;
use crate::{Attribute, Document, Map};

/// `example`, which is of `body`, and each block inside it, each with the body of its
/// form and its path of block indexes from `example`.
///
/// # Panics
///
/// When a block of `example` is not a block of its body, or a block inside a body has
/// more than one form.
#[must_use]
pub fn sites(
    body: &'static Body,
    example: &Document,
) -> Vec<(&'static Body, Vec<usize>)> {
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

/// The body of the block of `document` at `path`, or `document` when `path` is empty.
///
/// # Panics
///
/// When `path` names no block.
#[must_use]
pub fn at<'a>(document: &'a Document, path: &[usize]) -> &'a Document {
    path.iter().fold(document, |body, index| {
        &body.blocks.get(*index).expect("a block at the path").body
    })
}

/// The body of the block of `document` at `path`, or `document` when `path` is empty,
/// to change.
///
/// # Panics
///
/// When `path` names no block.
pub fn at_mut<'a>(document: &'a mut Document, path: &[usize]) -> &'a mut Document {
    path.iter().fold(document, |body, index| {
        &mut body
            .blocks
            .get_mut(*index)
            .expect("a block at the path")
            .body
    })
}

/// `document` with the attributes of its body at `path` changed by `change`.
///
/// # Panics
///
/// When `path` names no block, or `change` leaves two attributes with one key.
#[must_use]
pub fn change(
    mut document: Document,
    path: &[usize],
    change: impl FnOnce(&mut Vec<Attribute>),
) -> Document {
    let body = at_mut(&mut document, path);
    let mut attributes = body.attributes.iter().cloned().collect();
    change(&mut attributes);
    body.attributes = Map::new(attributes).expect("unique keys");
    document
}

/// `document` without the attribute `key` in its body at `path`, or `None` when that
/// body has none.
///
/// # Panics
///
/// When `path` names no block.
#[must_use]
pub fn without(document: Document, path: &[usize], key: &str) -> Option<Document> {
    at(&document, path).attributes.get(key)?;
    Some(change(document, path, |attributes| {
        attributes.retain(|attribute| &*attribute.key != key);
    }))
}

/// Whether `held`, of `body`, needs `key`: its absence gives a diagnostic.
///
/// # Panics
///
/// When `body` states no attribute `key`.
#[must_use]
pub fn needs(body: &Body, key: &str, held: &Document) -> bool {
    body.attributes
        .iter()
        .find(|attribute| attribute.name == key)
        .expect("a key of the body")
        .presence
        .needed(&held.attributes)
}

/// Whether `problem` is a missing attribute that names `key`.
#[must_use]
pub fn misses(problem: &Diagnostic, key: &str) -> bool {
    problem.code.as_str() == "document.missing-attribute"
        && problem
            .message
            .split_once(" has no ")
            .is_some_and(|(_, keys)| keys.contains(&format!("`{key}`")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;
    use crate::diagnostic::Code;
    use crate::read;
    use crate::schema::{self, Presence};
    use crate::value::{Kind, Value};

    const fn attribute(name: &'static str, presence: Presence) -> schema::Attribute {
        schema::Attribute {
            name,
            value: "an integer",
            presence,
            summary: "A number.",
        }
    }

    const fn holding(
        summary: &'static str,
        attributes: &'static [schema::Attribute],
        blocks: &'static [schema::Block],
    ) -> Body {
        Body {
            summary,
            attributes,
            blocks,
            examples: &[],
        }
    }

    const OTHER: Body = holding("Other.", &[attribute("d", Presence::Optional)], &[]);

    const INNER: Body = holding(
        "Inner.",
        &[attribute("c", Presence::Required)],
        &[schema::Block {
            keyword: "other",
            label: None,
            forms: &[OTHER],
        }],
    );

    const OUTER: Body = holding(
        "Outer.",
        &[
            attribute("a", Presence::Required),
            attribute("b", Presence::Default("1")),
            attribute("u", Presence::Unless(&["a"])),
        ],
        &[
            schema::Block {
                keyword: "inner",
                label: None,
                forms: &[INNER],
            },
            schema::Block {
                keyword: "other",
                label: None,
                forms: &[OTHER],
            },
        ],
    );

    fn document(keys: &[&str], blocks: Vec<Block>) -> Document {
        let attributes = keys
            .iter()
            .map(|key| Attribute {
                key: (*key).into(),
                key_span: None,
                value: Value {
                    kind: Kind::Integer(1),
                    span: None,
                },
            })
            .collect();
        Document {
            attributes: Map::new(attributes).expect("unique keys"),
            blocks,
        }
    }

    fn block(keyword: &str, body: Document) -> Block {
        Block {
            keyword: keyword.into(),
            keyword_span: None,
            labels: Vec::new(),
            body,
            span: None,
        }
    }

    /// `outer { a b inner { c other { d } } other { d } }`, with `other` last so
    /// that a site that `outer` gives `other` differs from one that it gives `inner`.
    fn example() -> Document {
        let other = || block("other", document(&["d"], Vec::new()));
        let inner = block("inner", document(&["c"], vec![other()]));
        document(&["a", "b"], vec![inner, other()])
    }

    #[test]
    fn gives_each_site_with_its_body_and_path() {
        assert_eq!(
            sites(&OUTER, &example()),
            [
                (&OTHER, vec![0, 0]),
                (&INNER, vec![0]),
                (&OTHER, vec![1]),
                (&OUTER, vec![]),
            ]
        );
    }

    #[test]
    fn gives_the_body_at_a_path() {
        let mut example = example();
        let deep = document(&["d"], Vec::new());
        assert_eq!(*at(&example, &[0, 0]), deep);
        assert_eq!(*at(&example, &[]), example);
        assert_eq!(*at_mut(&mut example, &[0, 0]), deep);
    }

    #[test]
    fn changes_the_attributes_of_the_body_at_a_path() {
        let changed = change(example(), &[1], |attributes| {
            attributes.retain(|attribute| &*attribute.key != "d");
        });
        let mut expected = example();
        expected.blocks[1].body = Document::default();
        assert_eq!(changed, expected);
    }

    #[test]
    fn removes_one_attribute_of_the_body_at_a_path() {
        let mut expected = example();
        expected.attributes = document(&["b"], Vec::new()).attributes;
        assert_eq!(without(example(), &[], "a"), Some(expected));
        assert_eq!(without(example(), &[1], "a"), None);
    }

    #[test]
    fn needs_an_attribute_as_its_presence_states() {
        let held = document(&["a"], Vec::new());
        assert!(needs(&OUTER, "a", &held));
        assert!(!needs(&OUTER, "b", &held));
        assert!(!needs(&OUTER, "u", &held));
        assert!(needs(&OUTER, "u", &Document::default()));
    }

    #[test]
    fn misses_only_a_key_that_a_missing_attribute_names() {
        let problem = read::missing("the block", None, &["a", "u"], "Add one".into());
        assert!(misses(&problem, "a"));
        assert!(misses(&problem, "u"));
        assert!(!misses(&problem, "b"));
        let other = Diagnostic::new(
            Code::new("document.unknown-attribute"),
            None,
            "the block has no `a`".into(),
            "Remove it".into(),
        );
        assert!(!misses(&other, "a"));
    }
}
