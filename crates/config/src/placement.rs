use std::collections::BTreeMap;

use document::diagnostic::{Code, Diagnostic};
use document::schema::{self, Attribute, Body};
use document::{Block, Span, read};
use spec::definition;
use spec::placement::{Error, Nodes, Policy};
use types::name::Name;

use crate::{Definition, Found, span};

const EMPTY_PLACEMENT: Code = Code::new("config.empty-placement");
const ROLE_OVERLAP: Code = Code::new("config.role-overlap");

/// The `placement` block.
pub(crate) const BLOCK: schema::Block = schema::Block {
    keyword: "placement",
    label: Some("the name of the policy"),
    forms: &[BODY],
};

const BODY: Body = Body {
    summary: "Places the connectors and indexes that it selects on nodes. It names at \
              least one node, and each node has one role.",
    attributes: &[
        Attribute {
            name: "select",
            value: "a selector",
            default: None,
            summary: "The connectors and indexes that it places.",
        },
        Attribute {
            name: "home",
            value: "the name of a node",
            default: Some("none"),
            summary: "The node that orders, buffers, and gates the selected indexes.",
        },
        Attribute {
            name: "standby",
            value: "the name of a node",
            default: Some("none"),
            summary: "The node that takes over when the home fails.",
        },
        Attribute {
            name: "copies",
            value: "the name of a node, or a list of them",
            default: Some("none"),
            summary: "The nodes that keep a copy that is never promoted.",
        },
    ],
    blocks: &[],
    examples: &[EXAMPLE],
};

const EXAMPLE: &str = "\
placement \"site_a\" {
  select  = \"site_a.*\"
  home    = \"edge\"
  standby = \"edge_b\"
  copies  = [\"cloud\"]
}
";

/// Checks a `placement` block and gives its policy.
pub(crate) fn check(
    found: &mut Found<'_>,
    block: &Block,
    _: Option<&Name>,
) -> Option<Definition> {
    let unknown = found.unknown(block, &BODY);
    let select =
        found.select(block, "connectors and indexes that it places", "site_a.*");
    let home = found.attribute(block, "home", read::name);
    let standby = found.attribute(block, "standby", read::name);
    let copies = found.attribute(block, "copies", |value| {
        read::items(value)
            .iter()
            .map(read::name)
            .collect::<Result<Vec<_>, _>>()
    });
    let (Ok(()), Ok(select), Ok(home), Ok(standby), Ok(copies)) =
        (unknown, select, home, standby, copies)
    else {
        return None;
    };
    let nodes = Nodes {
        home,
        standby,
        copies: copies.unwrap_or_default(),
    };
    match Policy::new(select, nodes.clone()) {
        Ok(policy) => Some(Definition::Spec(definition::Definition::Placement(policy))),
        Err(error) => {
            refuse(found, block, &nodes, &error);
            None
        }
    }
}

/// Each node that `policy` names, once: its home, its standby, then its copies in name
/// order.
pub(crate) fn nodes(policy: &Policy) -> impl Iterator<Item = &Name> {
    let copies = policy.copies();
    policy
        .home()
        .into_iter()
        .chain(policy.standby())
        .chain(copies)
}

/// The span of the first `home`, `standby`, or copy of `block`, a checked `placement`
/// block, that names each node.
pub(crate) fn spans(block: &Block) -> BTreeMap<Name, Option<Span>> {
    let attributes = BODY
        .attributes
        .iter()
        .filter(|key| key.name != "select")
        .filter_map(|key| block.body.attributes.get(key.name));
    let values = attributes.flat_map(|attribute| read::items(&attribute.value));
    let mut spans = BTreeMap::new();
    for value in values {
        let node = read::name(value).expect("invariant: `check` read each node");
        spans.entry(node).or_insert(value.span);
    }
    spans
}

/// Reports a policy that `Policy::new` refuses for `nodes`.
fn refuse(found: &mut Found<'_>, block: &Block, nodes: &Nodes, error: &Error) {
    let diagnostic = match error {
        Error::Empty => Diagnostic::new(
            EMPTY_PLACEMENT,
            span(block, "copies").or(block.keyword_span),
            format!(
                "the `{}` block names no home, no standby, and no copy",
                block.keyword
            ),
            "Name a `home`, a `standby`, or a node in `copies`".into(),
        ),
        Error::Overlap(node) => {
            let roles = [
                ("home", nodes.home.as_ref() == Some(node)),
                ("standby", nodes.standby.as_ref() == Some(node)),
                ("copies", nodes.copies.contains(node)),
            ];
            let last = roles
                .into_iter()
                .filter(|(_, named)| *named)
                .filter_map(|(role, _)| span(block, role))
                .max();
            Diagnostic::new(
                ROLE_OVERLAP,
                last,
                error.to_string(),
                format!("Keep {node} in one of `home`, `standby`, and `copies`"),
            )
        }
    };
    found.diagnostics.push(diagnostic);
}
