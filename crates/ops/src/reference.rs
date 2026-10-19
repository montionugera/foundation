//! The reference of what a config file holds: each block, and each source of a secret.

use document::schema::{Attribute, Block, Body, Presence};
use secret::source::{CALLS, Call};

/// Each block of `config::SCHEMA` and each call of `secret::source::CALLS`, as
/// Markdown.
pub(crate) fn markdown() -> String {
    let blocks: String = config::SCHEMA.iter().map(|b| block(b, 2)).collect();
    let calls: String = CALLS.iter().map(call).collect();
    format!(
        "\n# Config blocks\n{blocks}\n# Secret sources\n\nA secret attribute takes \
         one of these calls, which names where its value comes from, never the \
         value.\n\n{calls}"
    )
}

/// `block` under a heading of `level`, with each of its forms.
fn block(block: &Block, level: usize) -> String {
    let label = block.label.map_or_else(
        || "No label.".to_owned(),
        |label| format!("Label: {label}."),
    );
    let forms = match block.forms {
        [form] => body(form, level + 1),
        forms => forms
            .iter()
            .enumerate()
            .map(|(n, form)| {
                let heading = "#".repeat(level + 1);
                format!("\n{heading} Form {}\n{}", n + 1, body(form, level + 2))
            })
            .collect::<Vec<_>>()
            .concat(),
    };
    format!(
        "\n{} `{}`\n\n{label}\n{forms}",
        "#".repeat(level),
        block.keyword
    )
}

/// `body`, with each block inside it under a heading of `level`.
fn body(body: &Body, level: usize) -> String {
    let attributes: String = body.attributes.iter().map(attribute).collect();
    let blocks: String = body.blocks.iter().map(|b| block(b, level)).collect();
    let examples = body
        .examples
        .iter()
        .map(|example| format!("\nExample:\n\n```hcl\n{example}```\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("\n{}\n\n{attributes}{blocks}{examples}", body.summary)
}

/// One line: the name, the value, when the check needs it, and what it sets.
fn attribute(attribute: &Attribute) -> String {
    format!(
        "- `{}`: {}. {} {}\n",
        attribute.name,
        attribute.value,
        presence(attribute.presence),
        attribute.summary
    )
}

/// When the check needs an attribute, as one sentence.
fn presence(presence: Presence) -> String {
    match presence {
        Presence::Required => "Required.".to_owned(),
        Presence::Unless(others) => {
            let names: Vec<String> = others.iter().map(|o| format!("`{o}`")).collect();
            let names = match names.as_slice() {
                [.., last] if names.len() > 1 => {
                    let rest = names[..names.len() - 1].join(", ");
                    let and = if names.len() > 2 { ", and" } else { " and" };
                    format!("{rest}{and} {last}")
                }
                _ => names.concat(),
            };
            format!("Required unless the body holds {names}.")
        }
        Presence::Default(value) => format!("Default: {value}."),
        Presence::Optional => "Optional.".to_owned(),
    }
}

/// One line: the call, its argument, and where the value comes from.
fn call(call: &Call) -> String {
    format!(
        "- `{}`: takes {}. {}\n",
        call.name, call.argument, call.summary
    )
}

#[cfg(test)]
mod tests;
