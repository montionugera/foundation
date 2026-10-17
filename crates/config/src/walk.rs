use document::value::{Kind, Value};
use document::{Block, Document, Map, Span};

/// One part of a Document.
pub(crate) enum Part<'a> {
    /// A block, with its keyword and labels. Its body follows.
    Block(&'a Block),
    /// The key of an attribute, in a Document or in a map value, and its span. Its
    /// value follows.
    Key(&'a str, Option<Span>),
    /// A value. Each item, map attribute, or call argument inside it follows.
    Value(&'a Value),
}

/// Gives `visit` each part of `document` at any depth, each part before what follows
/// it. When `visit` gives `false` for a value, the walk skips what is inside that
/// value. For a block or a key, the walk ignores what `visit` gives.
pub(crate) fn each(document: &Document, visit: &mut impl FnMut(Part<'_>) -> bool) {
    in_map(&document.attributes, visit);
    for block in &document.blocks {
        visit(Part::Block(block));
        each(&block.body, visit);
    }
}

fn in_map(map: &Map, visit: &mut impl FnMut(Part<'_>) -> bool) {
    for attribute in map.iter() {
        visit(Part::Key(&attribute.key, attribute.key_span));
        in_value(&attribute.value, visit);
    }
}

fn in_value(value: &Value, visit: &mut impl FnMut(Part<'_>) -> bool) {
    if !visit(Part::Value(value)) {
        return;
    }
    match &value.kind {
        Kind::List(items) => items.iter().for_each(|item| in_value(item, visit)),
        Kind::Map(map) => in_map(map, visit),
        Kind::Call(call) => call
            .arguments
            .iter()
            .for_each(|argument| in_value(argument, visit)),
        Kind::String(_)
        | Kind::Reference(_)
        | Kind::Bool(_)
        | Kind::Integer(_)
        | Kind::Float(_) => {}
    }
}
