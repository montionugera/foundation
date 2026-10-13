//! The output of `foundation status --json`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The output of `foundation status --json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    connectors: Vec<Connector>,
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
}

/// The last error of a connector in `foundation status --json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Error {
    /// `config`, `device`, or `retry`.
    pub(crate) class: String,
    pub(crate) text: String,
    /// The time of the next try. `None` unless the state is `restarting`. A missing key
    /// reads as `None`, because `status` gives no `next` until #2530.
    #[serde(default)]
    pub(crate) next: Option<String>,
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
/// `restarts`, `counts`, or `error` has another shape.
#[test]
fn a_connector_that_lacks_an_optional_key_is_refused() {
    let keys = ["address", "restarts", "counts", "error"];
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
