use document::schema::{self, Attribute, Body};
use document::{Block, read};
use spec::definition;
use spec::retention::Policy;
use types::name::Name;

use crate::{Definition, Found};

/// The `retention` block.
pub(crate) const BLOCK: schema::Block = schema::Block {
    keyword: "retention",
    label: Some("the name of the policy"),
    forms: &[BODY],
};

const BODY: Body = Body {
    summary: "Caps how long a hold keeps a sample of each index that it selects, so \
              the node may trim it when it needs disk.",
    attributes: &[
        Attribute {
            name: "select",
            value: "a selector",
            default: None,
            summary: "The indexes that it caps.",
        },
        Attribute {
            name: "keep",
            value: "a span of zero or more, such as \"3d\"",
            default: None,
            summary: "The most time that a hold keeps a sample after its store time.",
        },
    ],
    blocks: &[],
    examples: &[EXAMPLE],
};

const EXAMPLE: &str = "\
retention \"site_a\" {
  select = \"site_a.**\"
  keep   = \"3d\"
}
";

/// Checks a `retention` block and gives its policy.
pub(crate) fn check(
    found: &mut Found<'_>,
    block: &Block,
    _: Option<&Name>,
) -> Option<Definition> {
    let unknown = found.unknown(block, &BODY);
    let select = found.select(block, "indexes that it caps", "site_a.**");
    let fix = "Add a `keep` attribute with a span such as \"3d\"";
    let keep = found.required(block, "keep", read::span, fix.into());
    let (Ok(()), Ok(select), Ok(keep)) = (unknown, select, keep) else {
        return None;
    };
    let policy =
        Policy::new(select, keep).expect("invariant: read::span reads zero or more");
    Some(Definition::Spec(definition::Definition::Retention(policy)))
}
