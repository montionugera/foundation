//! The output of `foundation status --json`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The output of `foundation status --json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    connectors: Vec<Connector>,
    roots: Option<Roots>,
}

/// The failures of the read of the OS roots in `foundation status --json`.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Roots {
    pub(crate) source: String,
    pub(crate) failures: Vec<String>,
    pub(crate) unlisted: u64,
}

/// One connector in `foundation status --json`. Each key must be present, also with a
/// null value.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connector {
    pub(crate) name: String,
    pub(crate) kind: String,
    /// `running`, `restarting`, `stopped`, `ending`, or `unknown`.
    pub(crate) state: String,
    /// The `address` attribute of the config, or `None` when it has none.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) address: Option<String>,
    /// `None` while the state is `unknown`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) restarts: Option<u64>,
    /// Each count that the kind names, such as `in` and `confirmed`. `None` while the
    /// state is `unknown`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) counts: Option<BTreeMap<String, u64>>,
    /// The last error. `None` when the connector has none, or while the state is
    /// `unknown`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) error: Option<Error>,
    /// The note that the running kind shows. `None` when it shows none, or while the
    /// state is `unknown`.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) note: Option<String>,
}

/// The last error of a connector in `foundation status --json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Error {
    /// `config`, `device`, or `retry`.
    pub(crate) class: String,
    pub(crate) text: String,
}

/// The connectors of `json`, the output of `foundation status --json`, by name.
/// Panics when `json` has another shape.
pub(crate) fn connectors(json: &[u8]) -> BTreeMap<String, Connector> {
    let output: Output = serde_json::from_slice(json).expect("the status JSON");
    (output.connectors.into_iter())
        .map(|connector| (connector.name.clone(), connector))
        .collect()
}

/// `ops` writes each key of a connector, so a connector that lacks one of `address`,
/// `restarts`, `counts`, `error`, or `note` has another shape.
#[test]
fn a_connector_that_lacks_a_nullable_key_is_refused() {
    let keys = ["address", "restarts", "counts", "error", "note"];
    for missing in keys {
        let present = keys.iter().filter(|key| **key != missing);
        let fields: Vec<_> = present.map(|key| format!(r#""{key}":null"#)).collect();
        let json = format!(
            r#"{{"connectors":[{{"name":"a","kind":"influx","state":"unknown",{}}}]}}"#,
            fields.join(","),
        );
        let error = serde_json::from_str::<Output>(&json)
            .err()
            .expect("a refusal");
        let column = json.len() - 2;
        assert_eq!(
            error.to_string(),
            format!("missing field `{missing}` at line 1 column {column}"),
        );
    }
}

/// `ops` writes no `next` until #2530, so an error with that key has another shape.
#[test]
fn an_error_with_next_is_refused() {
    let json = concat!(
        r#"{"connectors":[{"name":"a","kind":"influx","state":"stopped","#,
        r#""address":null,"restarts":0,"counts":{},"#,
        r#""error":{"class":"config","text":"t","next":null},"note":null}]}"#,
    );
    let error = serde_json::from_str::<Output>(json)
        .err()
        .expect("a refusal");
    let column = json.find(r#""next""#).expect("the key") + 6;
    assert_eq!(
        error.to_string(),
        format!(
            "unknown field `next`, expected `class` or `text` at line 1 column {column}"
        ),
    );
}

/// A key that `ops` does not write, on a connector or beside `connectors`, is another
/// shape.
#[test]
fn an_unknown_key_is_refused() {
    let connector = concat!(
        r#"{"name":"a","kind":"influx","state":"unknown","address":null,"#,
        r#""restarts":null,"counts":null,"error":null,"note":null"#,
    );
    let cases = [
        (
            format!(r#"{{"connectors":[{connector},"extra":0}}]}}"#),
            "expected one of `name`, `kind`, `state`, `address`, `restarts`, \
             `counts`, `error`, `note`",
        ),
        (
            format!(r#"{{"connectors":[{connector}}}],"extra":0}}"#),
            "expected `connectors` or `roots`",
        ),
        (
            concat!(
                r#"{"connectors":[],"roots":"#,
                r#"{"source":"","failures":[],"unlisted":0,"extra":0}}"#,
            )
            .to_owned(),
            "expected one of `source`, `failures`, `unlisted`",
        ),
    ];
    for (json, expected) in cases {
        let error = serde_json::from_str::<Output>(&json)
            .err()
            .expect("a refusal");
        let column = json.find(r#""extra""#).expect("the key") + 7;
        assert_eq!(
            error.to_string(),
            format!("unknown field `extra`, {expected} at line 1 column {column}"),
        );
    }
}

/// The failures of the OS roots in `json`, the output of `foundation status --json`, or
/// `None` when it has none. Panics when `json` has another shape.
pub(crate) fn roots(json: &[u8]) -> Option<Roots> {
    let output: Output = serde_json::from_slice(json).expect("the status JSON");
    output.roots
}
