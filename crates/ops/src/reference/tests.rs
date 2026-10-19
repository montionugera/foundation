use secret::CALLS;

use super::*;
use crate::operation;

/// Each body of `block` and of each block inside it.
fn bodies(block: &'static Block) -> Vec<&'static Body> {
    block
        .forms
        .iter()
        .flat_map(|form| {
            std::iter::once(form).chain(form.blocks.iter().flat_map(bodies))
        })
        .collect()
}

#[test]
fn docs_state_each_block_form_attribute_and_example() {
    let docs = operation::docs();
    for block in config::SCHEMA {
        let heading = format!("\n## `{}`\n\n", block.keyword);
        assert!(docs.contains(&heading), "{heading}");
        let label = block.label.expect("each core block has a label");
        assert!(docs.contains(&format!("Label: {label}.\n")), "{label}");
        for body in bodies(block) {
            assert!(docs.contains(&format!("\n{}\n\n", body.summary)));
            for attribute in body.attributes {
                let line = format!(
                    "- `{}`: {}. {} {}\n",
                    attribute.name,
                    attribute.value,
                    presence(attribute.presence),
                    attribute.summary
                );
                assert!(docs.contains(&line), "{line}");
            }
            for example in body.examples {
                let fence = format!("```hcl\n{example}```\n");
                assert!(docs.contains(&fence), "{example}");
            }
        }
    }
}

#[test]
fn docs_give_the_forms_of_a_block_in_order() {
    let docs = operation::docs();
    let channel = docs.find("\n## `channel`\n").expect("a channel section");
    let index = docs[channel..].find("\n### Form 1\n\nAn index channel");
    let data = docs[channel..].find("\n### Form 2\n\nA data channel");
    assert!(index.is_some() && index < data, "{docs}");
}

/// A body with no attributes and the blocks `blocks`.
const fn holding(summary: &'static str, blocks: &'static [Block]) -> Body {
    Body {
        summary,
        attributes: &[],
        blocks,
        examples: &[],
    }
}

const INNER: Block = Block {
    keyword: "inner",
    label: None,
    forms: &[holding("Inner.", &[])],
};

const ONE: Block = Block {
    keyword: "one",
    label: None,
    forms: &[holding("One.", &[INNER])],
};

const TWO: Block = Block {
    keyword: "two",
    label: Some("a name"),
    forms: &[holding("First.", &[INNER]), holding("Second.", &[])],
};

#[test]
fn a_block_inside_a_body_gets_the_next_heading() {
    let text = markdown(&[ONE, TWO], &[]);
    assert!(
        text.starts_with(
            "\n# Config blocks\n\n## `one`\n\nNo label.\n\nOne.\n\n\n### \
             `inner`\n\nNo label.\n\nInner.\n\n\n## `two`\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("\n### Form 1\n\nFirst.\n\n\n#### `inner`\n"),
        "{text}"
    );
    assert!(text.contains("\n### Form 2\n\nSecond.\n"), "{text}");
}

#[test]
fn docs_state_each_presence_in_its_words() {
    let docs = operation::docs();
    for line in [
        "- `kind`: \"index\". Required. Makes the channel an index.\n",
        "- `kind`: \"data\". Default: \"data\". Makes the channel a data channel.\n",
        "- `unit`: a unit, such as \"kPa\". Optional. The unit of a numeric sample.\n",
        "- `disk`: a size above zero, such as \"10GiB\". Required unless the body \
         holds `pool`. The disk budget of each node. Without it, the budget of a less \
         specific policy.\n",
    ] {
        assert!(docs.contains(line), "{line}");
    }
}

/// An attribute `name` that each of `others` can take the place of.
const fn unless(name: &'static str, others: &'static [&'static str]) -> Attribute {
    Attribute {
        name,
        value: "a value",
        presence: Presence::Unless(others),
        summary: "It.",
    }
}

const UNLESS: Block = Block {
    keyword: "unless",
    label: None,
    forms: &[Body {
        summary: "Unless.",
        attributes: &[
            unless("one", &["a"]),
            unless("two", &["a", "b"]),
            unless("three", &["a", "b", "c"]),
        ],
        blocks: &[],
        examples: &[],
    }],
};

#[test]
fn unless_joins_the_names_it_takes() {
    let text = markdown(&[UNLESS], &[]);
    for line in [
        "- `one`: a value. Required unless the body holds `a`. It.\n",
        "- `two`: a value. Required unless the body holds `a` and `b`. It.\n",
        "- `three`: a value. Required unless the body holds `a`, `b`, and `c`. It.\n",
    ] {
        assert!(text.contains(line), "{text}");
    }
}

#[test]
fn docs_state_each_secret_source() {
    let docs = operation::docs();
    assert!(docs.contains("\n# Config blocks\n\n## `"), "{docs}");
    let sources = docs.find("\n# Secret sources\n").expect("a secret section");
    assert!(docs[sources..].starts_with(
        "\n# Secret sources\n\nA secret attribute takes one of these calls, which \
         names where its value comes from, never the value.\n\n"
    ));
    for call in CALLS {
        let line = format!("- `{call}`: {}\n", call.summary());
        assert!(docs[sources..].contains(&line), "{line}");
    }
    assert!(docs[sources..].contains(
        "- `env(\"INFLUX_TOKEN\")`: The variable of that name in the environment of \
         the node process, as it was when the process began.\n"
    ));
}
