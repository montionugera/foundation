//! The output of `browse`: the entries of a device that a browse found, and why it
//! stopped.

use std::fmt::Write as _;

use connector::browse::{self, Found, LIMIT};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Number;
use types::sample;

use crate::error::escape_controls;

#[cfg(test)]
mod tests;

/// What a browse found, as `browse` gives it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Output {
    /// The entries, in device order.
    pub(crate) entries: Vec<Entry>,
    /// Why the browse stopped.
    pub(crate) end: End,
}

/// One entry of the device.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Entry {
    /// Where it is: its names from the root, joined by `/`, with a `\` before each
    /// `/`, `*`, and `\` in a name. `--start` takes the path of a folder as it is.
    pub(crate) path: String,
    /// Its address in the device, in the form of the kind, such as an OPC UA node ID.
    /// Two entries with the same tag are one entry of the device.
    pub(crate) tag: String,
    #[serde(flatten)]
    pub(crate) shape: Shape,
}

/// What an entry is.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub(crate) enum Shape {
    /// It holds other entries.
    Folder,
    /// A point that a connector of the kind can read or write.
    Point(Point),
    /// A point that a connector of the kind cannot read or write.
    Unreadable {
        /// Why, such as "an array".
        reason: String,
    },
}

/// What the device knows of a point. Each fact is `null` when the device cannot know
/// it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Point {
    /// The sample type, as a `type` attribute takes it, such as `f64`.
    #[serde(rename = "type")]
    pub(crate) data_type: Option<String>,
    /// The unit, as the device gives it.
    pub(crate) unit: Option<String>,
    /// The UN/CEFACT Recommendation 20 common code of the unit, such as `CEL`.
    pub(crate) unit_code: Option<String>,
    pub(crate) access: Option<Access>,
    /// The value, only with `values`: `null` when the device gave none, or a value
    /// that is not one fixed-width scalar.
    #[expect(
        clippy::option_option,
        reason = "the outer `None` is no `value` field, the inner one `null`"
    )]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "given"
    )]
    pub(crate) value: Option<Option<Value>>,
}

/// Whether a connector can read a point, write it, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Access {
    Read,
    Write,
    ReadWrite,
}

/// One value of a point. A bool and each integer of 32 bits or fewer are a JSON bool
/// and number, an `i64` or `u64` its decimal text, a finite float a number, and a float
/// that is not finite the text `NaN`, `Infinity`, or `-Infinity`.
/// A stamp is RFC 3339 text in UTC with nine fraction digits, a span its text such as
/// `250us`, and a UUID its hyphenated text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum Value {
    Bool(bool),
    Number(Number),
    Text(String),
}

/// Why a browse stopped.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum End {
    /// It gave each entry that it looks for.
    Done,
    /// The next page of the level starts at this cursor, which `cursor` takes.
    Next(String),
    /// More than 500 points match, the most for one browse.
    Limit,
    /// It stopped at its budget of work, so more may match.
    Budget,
}

/// A value that is there, also when it is `null`.
#[expect(clippy::option_option, reason = "the type of `Point::value`")]
fn given<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Value>>, D::Error> {
    Option::<Value>::deserialize(deserializer).map(Some)
}

impl Output {
    /// The output of `found`.
    pub(crate) fn of(found: Found) -> Self {
        let mut values = found.values.into_iter();
        let entries = found
            .entries
            .into_iter()
            .map(|entry| {
                // With values, `found` holds one for each entry.
                let value = values.next();
                let shape = match entry.shape {
                    browse::Shape::Folder => Shape::Folder,
                    browse::Shape::Unreadable(reason) => Shape::Unreadable { reason },
                    browse::Shape::Point(facts) => Shape::Point(Point {
                        data_type: facts.data_type.map(|t| t.to_string()),
                        unit: facts.unit,
                        unit_code: facts.unit_code,
                        access: facts.access.map(Access::of),
                        value: value.map(|value| value.map(Value::of)),
                    }),
                };
                Entry {
                    path: entry.path.to_string(),
                    tag: entry.tag,
                    shape,
                }
            })
            .collect();
        let end = match found.end {
            browse::End::Done => End::Done,
            browse::End::Next(cursor) => End::Next(cursor.as_str().to_owned()),
            browse::End::Limit => End::Limit,
            browse::End::Budget => End::Budget,
        };
        Self { entries, end }
    }

    /// One line for each entry, then one line for the end: a folder as `<path>/`, a
    /// point as `<path> <type> <unit> <access>`, then its value with `values`, and an
    /// unreadable point as `<path> unreadable: <reason>`. A fact that the device
    /// cannot know is `?`. The unit is its text, else its code.
    pub(crate) fn text(&self) -> String {
        let mut text = String::new();
        if self.entries.is_empty() {
            text.push_str("The browse found no entries.\n");
        }
        for entry in &self.entries {
            let path = escape_controls(&entry.path);
            let line = match &entry.shape {
                Shape::Folder => format!("{path}/"),
                Shape::Unreadable { reason } => {
                    format!("{path} unreadable: {}", escape_controls(reason))
                }
                Shape::Point(point) => point.line(&path),
            };
            writeln!(text, "{line}").expect("invariant: a String takes each write");
        }
        let end = match &self.end {
            End::Done => return text,
            End::Next(cursor) => format!("more: --cursor {}", escape_controls(cursor)),
            End::Limit => format!(
                "More than {LIMIT} points match, the most for one browse; narrow the \
                 pattern or the start"
            ),
            End::Budget => "The browse stopped at its budget, so more may match; \
                            narrow the pattern or the start"
                .to_owned(),
        };
        writeln!(text, "{end}").expect("invariant: a String takes each write");
        text
    }
}

impl Point {
    /// `<path> <type> <unit> <access>`, then the value when it is there.
    fn line(&self, path: &str) -> String {
        let known =
            |fact: Option<&str>| fact.map_or_else(|| "?".to_owned(), escape_controls);
        let unit = self.unit.as_deref().or(self.unit_code.as_deref());
        let access = self.access.map(|access| match access {
            Access::Read => "read",
            Access::Write => "write",
            Access::ReadWrite => "read-write",
        });
        let mut line = format!(
            "{path} {} {} {}",
            known(self.data_type.as_deref()),
            known(unit),
            known(access)
        );
        if let Some(value) = &self.value {
            let value = value.as_ref().map(Value::text);
            line = format!("{line} {}", known(value.as_deref()));
        }
        line
    }
}

impl Access {
    fn of(access: browse::Access) -> Self {
        match access {
            browse::Access::Read => Self::Read,
            browse::Access::Write => Self::Write,
            browse::Access::ReadWrite => Self::ReadWrite,
        }
    }
}

impl Value {
    /// The JSON of `value`.
    pub(crate) fn of(value: sample::Value) -> Self {
        match value {
            sample::Value::Bool(value) => Self::Bool(value),
            sample::Value::I8(value) => Self::Number(value.into()),
            sample::Value::I16(value) => Self::Number(value.into()),
            sample::Value::I32(value) => Self::Number(value.into()),
            sample::Value::I64(value) => Self::Text(value.to_string()),
            sample::Value::U8(value) => Self::Number(value.into()),
            sample::Value::U16(value) => Self::Number(value.into()),
            sample::Value::U32(value) => Self::Number(value.into()),
            sample::Value::U64(value) => Self::Text(value.to_string()),
            sample::Value::F32(value) if value.is_finite() => {
                // The shortest text that reads back as this `f32`, not as its `f64`.
                let text = serde_json::to_string(&value)
                    .expect("invariant: a finite float is JSON");
                Self::Number(
                    text.parse()
                        .expect("invariant: the JSON of a finite float is a number"),
                )
            }
            sample::Value::F32(value) => Self::not_finite(value.into()),
            sample::Value::F64(value) => Number::from_f64(value)
                .map_or_else(|| Self::not_finite(value), Self::Number),
            sample::Value::Stamp(stamp) => Self::Text(stamp.to_string()),
            sample::Value::Span(span) => Self::Text(span.to_string()),
            sample::Value::Uuid(bytes) => {
                let n = u128::from_be_bytes(bytes);
                Self::Text(format!(
                    "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                    n >> 96,
                    (n >> 80) & 0xffff,
                    (n >> 64) & 0xffff,
                    (n >> 48) & 0xffff,
                    n & 0xffff_ffff_ffff
                ))
            }
        }
    }

    /// `NaN`, `Infinity`, or `-Infinity`.
    fn not_finite(value: f64) -> Self {
        let text = if value.is_nan() {
            "NaN"
        } else if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        };
        Self::Text(text.to_owned())
    }

    /// The text of the value, as its JSON gives it with no quotes.
    fn text(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::Text(value) => value.clone(),
        }
    }
}
