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

#[test]
fn docs_state_each_presence_in_its_words() {
    let docs = operation::docs();
    for line in [
        "- `kind`: \"index\". Required. Makes the channel an index.\n",
        "- `kind`: \"data\". Default: \"data\". Makes the channel a data channel.\n",
        "- `unit`: a unit, such as \"kPa\". Optional. The unit of a numeric sample.\n",
        "- `disk`: a size above zero, such as \"10GiB\". Required unless the body holds \
         `pool`. The disk budget of each node. Without it, the budget of a less \
         specific policy.\n",
    ] {
        assert!(docs.contains(line), "{line}");
    }
}

#[test]
fn unless_joins_the_names_it_takes() {
    for (names, text) in [
        (&["a"][..], "Required unless the body holds `a`."),
        (&["a", "b"], "Required unless the body holds `a` and `b`."),
        (
            &["a", "b", "c"],
            "Required unless the body holds `a`, `b`, and `c`.",
        ),
    ] {
        assert_eq!(presence(Presence::Unless(names)), text);
    }
}

#[test]
fn docs_state_each_secret_source() {
    let docs = operation::docs();
    let sources = docs.find("\n# Secret sources\n").expect("a secret section");
    for call in CALLS {
        let line = format!(
            "- `{}`: takes {}. {}\n",
            call.name, call.argument, call.summary
        );
        assert!(docs[sources..].contains(&line), "{line}");
    }
    assert!(docs[sources..].contains(
        "- `env`: takes the name of a variable, as a string, such as \
         \"INFLUX_TOKEN\". The variable of the environment of the node process, as it \
         was when the process began.\n"
    ));
}
