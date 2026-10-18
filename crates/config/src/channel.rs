use document::diagnostic::{Code, Diagnostic};
use document::schema::{self, Attribute, Body, Presence};
use document::value::Value;
use document::{Block, read};
use spec::channel::{Data, Edge, Error, Kind};
use spec::data_type::DataType;
use spec::unit::Unit;
use types::name::Name;

use crate::{Definition, Found, Reported, span};

const BAD_CHANNEL_KIND: Code = Code::new("config.bad-channel-kind");
const BAD_DATA_TYPE: Code = Code::new("config.bad-data-type");
const BAD_UNIT: Code = Code::new("config.bad-unit");
const UNKNOWN_CHANNEL: Code = Code::new("config.unknown-channel");

/// The `channel` block, with one form for each kind of channel.
pub(crate) const BLOCK: schema::Block = schema::Block {
    keyword: "channel",
    label: Some("the name of the channel"),
    forms: &[INDEX, DATA],
};

const INDEX: Body = Body {
    summary: "An index channel, with `kind = \"index\"`: the time base of the data \
              channels that point at it.",
    attributes: &[
        Attribute {
            name: "kind",
            value: "\"index\"",
            presence: Presence::Required,
            summary: "Makes the channel an index.",
        },
        ERROR,
        CONTROL,
    ],
    blocks: &[],
    examples: &[EXAMPLE_INDEX],
};

const DATA: Body = Body {
    summary: "A data channel, with `kind = \"data\"` or no `kind`: the samples of \
              one value, timed by its index.",
    attributes: &[KIND_DATA, DATA_TYPE, INDEX_EDGE, QUALITY, UNIT],
    blocks: &[],
    examples: &[EXAMPLE_DATA],
};

/// What each kind of channel takes, for a block whose `kind` does not read.
pub(crate) const ANY: Body = Body {
    summary: "A channel of either kind.",
    attributes: &[
        CONTROL, DATA_TYPE, ERROR, INDEX_EDGE, KIND_DATA, QUALITY, UNIT,
    ],
    blocks: &[],
    examples: &[],
};

const ERROR: Attribute = Attribute {
    name: "error",
    value: "the name of a data channel",
    presence: Presence::Optional,
    summary: "The channel that holds the clock error bound of its timestamps.",
};

const CONTROL: Attribute = Attribute {
    name: "control",
    value: "the name of a data channel on another index",
    presence: Presence::Optional,
    summary: "The channel that the home writes control handoffs to.",
};

const KIND_DATA: Attribute = Attribute {
    name: "kind",
    value: "\"data\"",
    presence: Presence::Default("\"data\""),
    summary: "Makes the channel a data channel.",
};

const DATA_TYPE: Attribute = Attribute {
    name: "data_type",
    value: "a data type, such as \"f64\", \"f32[3]\", \"list<u8, 16>\", \
            \"string\", \"bytes\", or \"quality\"",
    presence: Presence::Required,
    summary: "The type of each sample.",
};

const INDEX_EDGE: Attribute = Attribute {
    name: "index",
    value: "the name of an index channel",
    presence: Presence::Required,
    summary: "The index channel that times its samples.",
};

const QUALITY: Attribute = Attribute {
    name: "quality",
    value: "the name of a data channel of type \"quality\"",
    presence: Presence::Optional,
    summary: "The channel that holds the quality of its samples.",
};

const UNIT: Attribute = Attribute {
    name: "unit",
    value: "a unit, such as \"kPa\"",
    presence: Presence::Optional,
    summary: "The unit of a numeric sample.",
};

const EXAMPLE_INDEX: &str = "\
channel \"edge.time\" {
  kind    = \"index\"
  error   = \"edge.time_error\"
  control = \"edge.control\"
}

channel \"edge.time_error\" {
  data_type = \"u64\"
  index     = \"edge.time\"
}

channel \"edge.control_time\" {
  kind = \"index\"
}

channel \"edge.control\" {
  data_type = \"string\"
  index     = \"edge.control_time\"
}
";

const EXAMPLE_DATA: &str = "\
channel \"edge.pressure\" {
  kind      = \"data\"
  data_type = \"f64\"
  index     = \"edge.time\"
  quality   = \"edge.pressure_quality\"
  unit      = \"kPa\"
}

channel \"edge.time\" {
  kind = \"index\"
}

channel \"edge.pressure_quality\" {
  data_type = \"quality\"
  index     = \"edge.time\"
}
";

/// The check of the attributes of one kind of channel.
type Attributes = fn(&mut Found<'_>, &Block) -> Option<Kind<Name>>;

/// Checks a `channel` block and gives its channel, with each edge as a name. After a
/// bad `kind`, it reports each attribute that no kind knows, and leaves each other
/// attribute, since its problem depends on the kind.
pub(crate) fn check(
    found: &mut Found<'_>,
    block: &Block,
    _: Option<&Name>,
) -> Option<Definition> {
    let attributes = found.attribute(block, "kind", |value| -> Result<Attributes, _> {
        match text(value, BAD_CHANNEL_KIND, "the channel kind", "\"index\"")? {
            "index" => Ok(index),
            "data" => Ok(data),
            text => Err(Diagnostic::new(
                BAD_CHANNEL_KIND,
                value.span,
                format!("{text:?} is not a kind of channel"),
                "Write \"index\" or \"data\"".into(),
            )),
        }
    });
    let Ok(attributes) = attributes else {
        drop(found.unknown(block, &ANY));
        return None;
    };
    let attributes = attributes.unwrap_or(data);
    attributes(found, block).map(Definition::Channel)
}

fn index(found: &mut Found<'_>, block: &Block) -> Option<Kind<Name>> {
    let unknown = found.unknown(block, &INDEX);
    let error = edge(found, block, Edge::Error);
    let control = edge(found, block, Edge::Control);
    let (Ok(()), Ok(error), Ok(control)) = (unknown, error, control) else {
        return None;
    };
    Some(Kind::Index { error, control })
}

fn data(found: &mut Found<'_>, block: &Block) -> Option<Kind<Name>> {
    let unknown = found.unknown(block, &DATA);
    let index = edge(found, block, Edge::Index).and_then(|index| {
        index.ok_or_else(|| {
            let fix = "Add an `index` attribute with the name of an index channel, \
                       such as \"edge.time\"";
            found.missing(block, &["index"], fix.into());
            Reported
        })
    });
    let quality = edge(found, block, Edge::Quality);
    let data_type = found.required(
        block,
        "data_type",
        |value| {
            let text = text(value, BAD_DATA_TYPE, "the data type", "\"f64\"")?;
            text.parse::<DataType>().map_err(|error| {
                Diagnostic::new(
                    BAD_DATA_TYPE,
                    value.span,
                    format!("cannot read the data type {text:?}: {error}"),
                    error.fix().into(),
                )
            })
        },
        "Add a `data_type` attribute such as \"f64\"".into(),
    );
    let unit = found.attribute(block, "unit", |value| {
        let text = text(value, BAD_UNIT, "the unit", "\"kPa\"")?;
        Unit::new(text).map_err(|error| {
            Diagnostic::new(
                BAD_UNIT,
                value.span,
                format!("cannot read the unit {text:?}: {error}"),
                error.fix().into(),
            )
        })
    });
    let (Ok(()), Ok(index), Ok(quality), Ok(data_type), Ok(unit)) =
        (unknown, index, quality, data_type, unit)
    else {
        return None;
    };
    match Data::new(index, quality, data_type, unit) {
        Ok(data) => Some(Kind::Data(data)),
        Err(ref error @ Error::Unit { ref data_type }) => {
            found.diagnostics.push(Diagnostic::new(
                BAD_UNIT,
                span(block, "unit"),
                format!("{error}: the data type is \"{data_type}\""),
                error.fix().into(),
            ));
            None
        }
    }
}

/// Reads the attribute of `edge`. A name that no `channel` block defines gives
/// `config.unknown-channel`.
fn edge(
    found: &mut Found<'_>,
    block: &Block,
    edge: Edge,
) -> Result<Option<Name>, Reported> {
    let key = attribute(edge);
    match found.attribute(block, key, read::name)? {
        Some(to) if !found.channels.contains(&to) => {
            found.diagnostics.push(Diagnostic::new(
                UNKNOWN_CHANNEL,
                span(block, key),
                format!("no `channel` block defines the {edge} `{to}`"),
                "Name a channel that a `channel` block defines".into(),
            ));
            Err(Reported)
        }
        to => Ok(to),
    }
}

/// The attribute of a `channel` block that holds `edge`.
pub(crate) const fn attribute(edge: Edge) -> &'static str {
    match edge {
        Edge::Index => "index",
        Edge::Quality => "quality",
        Edge::Error => "error",
        Edge::Control => "control",
    }
}

/// The text of a string or a reference. Another value gives `code`, with `what` in
/// the message and `example` in the fix.
fn text<'v>(
    value: &'v Value,
    code: Code,
    what: &str,
    example: &str,
) -> Result<&'v str, Diagnostic> {
    value.kind.text().ok_or_else(|| {
        Diagnostic::new(
            code,
            value.span,
            format!(
                "{what} is a string or a reference, not {}",
                value.kind.noun()
            ),
            format!("Write a string such as {example}"),
        )
    })
}
