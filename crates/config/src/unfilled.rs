use ::connector::import;
use document::diagnostic::{Code, Diagnostic};
use document::value::{Kind, Value};
use document::{Document, Map};

const UNFILLED: Code = Code::new("config.unfilled");

/// `config.unfilled` at each `fill("<what>")` of `documents`, in any value at any
/// depth, in the order of their [`document::Source`], then in source order.
pub(crate) fn problems(documents: &[Document]) -> Vec<Diagnostic> {
    let mut problems = Vec::new();
    for document in documents {
        in_document(document, &mut problems);
    }
    crate::sort(&mut problems);
    problems
}

fn in_document(document: &Document, problems: &mut Vec<Diagnostic>) {
    in_map(&document.attributes, problems);
    for block in &document.blocks {
        in_document(&block.body, problems);
    }
}

fn in_map(map: &Map, problems: &mut Vec<Diagnostic>) {
    for attribute in map.iter() {
        in_value(&attribute.value, problems);
    }
}

fn in_value(value: &Value, problems: &mut Vec<Diagnostic>) {
    if let Some(what) = import::unfilled(value) {
        problems.push(Diagnostic::new(
            UNFILLED,
            value.span,
            format!("the import found no value for {what}"),
            "Write the value in place of `fill`".into(),
        ));
        return;
    }
    match &value.kind {
        Kind::List(items) => items.iter().for_each(|item| in_value(item, problems)),
        Kind::Map(map) => in_map(map, problems),
        Kind::Call(call) => call
            .arguments
            .iter()
            .for_each(|argument| in_value(argument, problems)),
        Kind::String(_)
        | Kind::Reference(_)
        | Kind::Bool(_)
        | Kind::Integer(_)
        | Kind::Float(_) => {}
    }
}
