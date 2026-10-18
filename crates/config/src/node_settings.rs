use document::diagnostic::{Code, Diagnostic};
use document::schema::{self, Attribute, Body, Presence};
use document::{Block, read};
use spec::definition;
use spec::node_settings::{Error, Policy};
use types::name::Name;

use crate::{Definition, Found};

const ZERO_SIZE: Code = Code::new("config.zero-size");
/// The `node_settings` block.
pub(crate) const BLOCK: schema::Block = schema::Block {
    keyword: "node_settings",
    label: Some("the name of the policy"),
    forms: &[BODY],
};

const BODY: Body = Body {
    summary: "Sets the disk and pool budgets of the nodes that it selects. It sets at \
              least one.",
    attributes: &[
        Attribute {
            name: "select",
            value: "a selector",
            presence: Presence::Required,
            summary: "The nodes that it sets.",
        },
        Attribute {
            name: "disk",
            value: "a size above zero, such as \"10GiB\"",
            presence: Presence::Default("the budget of a less specific policy"),
            summary: "The disk budget of each node.",
        },
        Attribute {
            name: "pool",
            value: "a size above zero, such as \"512MiB\"",
            presence: Presence::Default("the budget of a less specific policy"),
            summary: "The pool budget of each node.",
        },
    ],
    blocks: &[],
    examples: &[EXAMPLE],
};

const EXAMPLE: &str = "\
node_settings \"site_a\" {
  select = \"site_a.*\"
  disk   = \"10GiB\"
  pool   = \"512MiB\"
}
";

/// Checks a `node_settings` block and gives its policy. An unknown attribute stops the
/// budget check, because it may be a budget under a wrong key.
pub(crate) fn check(
    found: &mut Found<'_>,
    block: &Block,
    _: Option<&Name>,
) -> Option<Definition> {
    let unknown = found.unknown(block, &BODY);
    let select = found.select(block, "nodes that it sets", "site_a.*");
    let disk = found.attribute(block, "disk", read::size);
    let pool = found.attribute(block, "pool", read::size);
    let (Ok(()), Ok(select), Ok(disk), Ok(pool)) = (unknown, select, disk, pool) else {
        return None;
    };
    match Policy::new(select, disk, pool) {
        Ok(policy) => Some(Definition::Spec(definition::Definition::NodeSettings(
            policy,
        ))),
        Err(error) => {
            refuse(found, block, error);
            None
        }
    }
}

/// Reports a policy that `Policy::new` refuses.
fn refuse(found: &mut Found<'_>, block: &Block, error: Error) {
    let zero = |key: &str| {
        let at = block.body.attributes.get(key);
        Diagnostic::new(
            ZERO_SIZE,
            at.and_then(|attribute| attribute.value.span),
            format!("the `{key}` budget is zero"),
            format!("Write a size above zero, or remove `{key}`"),
        )
    };
    match error {
        Error::ZeroDisk => found.diagnostics.push(zero("disk")),
        Error::ZeroPool => found.diagnostics.push(zero("pool")),
        Error::NoBudget => {
            let fix = "Add a `disk` attribute, a `pool` attribute, or both, with a \
                       size such as \"10GiB\"";
            found.missing(block, &["disk", "pool"], fix.into());
        }
    }
}
