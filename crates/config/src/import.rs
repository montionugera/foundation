//! The `channel` blocks that an import writes into the files, so that a kind never
//! writes one.

use ::connector::browse::Facts;
use ::connector::import::fill;
use document::value::{Kind, Value};
use document::{Attribute, Block, Document, Label, Map};
use spec::data_type::DataType;
use spec::unit::Unit;
use types::name::Name;

/// The `channel` block of the data channel `name` on the index channel `index`, with
/// the sample type and the unit of `facts`. When `facts` has no sample type, the block
/// holds [`fill`]`("the sample type")`. The unit is what [`Unit::given`] gives for the
/// unit and its code in `facts`, where an empty text or code counts as none. When it
/// gives none, the block holds `fill("the unit, which the device gives as <what>")`,
/// where `<what>` is `"<unit>"`, `the code "<code>"`, or `"<unit>" with the code
/// "<code>"`, as `facts` holds them. A sample type that holds no number gets no unit.
#[must_use]
pub fn channel(name: &Name, index: &Name, facts: &Facts) -> Block {
    let data_type = facts.data_type.map(DataType::Sample);
    let numeric = data_type.as_ref().is_none_or(DataType::numeric);
    let mut attributes = vec![
        attribute(
            "data_type",
            data_type.map_or_else(
                || fill("the sample type"),
                |data_type| string(&data_type.to_string()),
            ),
        ),
        attribute("index", string(index.as_str())),
    ];
    let text = facts.unit.as_deref().filter(|text| !text.is_empty());
    let code = facts.unit_code.as_deref().filter(|code| !code.is_empty());
    let given = |what: String| {
        Some(fill(&format!("the unit, which the device gives as {what}")))
    };
    let unit = match (Unit::given(text, code), text, code) {
        (Some(unit), ..) => Some(string(unit.as_str())),
        (None, None, None) => None,
        (None, Some(text), None) => given(format!("{text:?}")),
        (None, None, Some(code)) => given(format!("the code {code:?}")),
        (None, Some(text), Some(code)) => {
            given(format!("{text:?} with the code {code:?}"))
        }
    };
    if let Some(unit) = unit.filter(|_| numeric) {
        attributes.push(attribute("unit", unit));
    }
    block(name, attributes)
}

/// The `channel` block of the index channel `name`.
#[must_use]
pub fn index(name: &Name) -> Block {
    block(name, vec![attribute("kind", string("index"))])
}

fn block(name: &Name, attributes: Vec<Attribute>) -> Block {
    Block {
        keyword: "channel".into(),
        keyword_span: None,
        labels: vec![Label {
            text: name.as_str().into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(attributes)
                .expect("invariant: each key is written once"),
            blocks: Vec::new(),
        },
        span: None,
    }
}

fn attribute(key: &str, value: Value) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value,
    }
}

fn string(text: &str) -> Value {
    Value {
        kind: Kind::String(text.into()),
        span: None,
    }
}

#[cfg(test)]
mod tests;
