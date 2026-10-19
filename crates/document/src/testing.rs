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
