//! The output of `foundation status --json`.

use std::collections::BTreeMap;

use serde_json::Value;

/// One connector in `foundation status --json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Connector {
    pub(crate) kind: String,
    /// `running`, `restarting`, `stopped`, or `unknown`.
    pub(crate) state: String,
    /// The `address` attribute of the config, or `None` when it has none.
    pub(crate) address: Option<String>,
    /// `None` while the state is `unknown`.
    pub(crate) restarts: Option<u64>,
    /// Each count that the kind names, such as `in` and `confirmed`. `None` while the
    /// state is `unknown`.
    pub(crate) counts: Option<BTreeMap<String, u64>>,
    /// The last error. `None` when the connector has none, or while the state is
    /// `unknown`.
    pub(crate) error: Option<Error>,
}

/// The last error of a connector in `foundation status --json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Error {
    /// `config`, `device`, or `retry`.
    pub(crate) class: String,
    pub(crate) text: String,
    /// The time of the next try. `None` unless the state is `restarting`.
    pub(crate) next: Option<String>,
}

/// The connectors of the JSON of `foundation status --json`, by name. Panics when a
/// field has another type.
pub(crate) fn connectors(json: &Value) -> BTreeMap<String, Connector> {
    let list = json["connectors"].as_array().expect("a list of connectors");
    (list.iter())
        .map(|connector| {
            let counts = connector["counts"].as_object().map(|counts| {
                (counts.iter())
                    .map(|(name, count)| {
                        (name.clone(), count.as_u64().expect("a count"))
                    })
                    .collect()
            });
            let error = &connector["error"];
            let shown = Connector {
                kind: string(&connector["kind"]).expect("a kind"),
                state: string(&connector["state"]).expect("a state"),
                address: string(&connector["address"]),
                restarts: connector["restarts"].as_u64(),
                counts,
                error: (!error.is_null()).then(|| Error {
                    class: string(&error["class"]).expect("a class"),
                    text: string(&error["text"]).expect("a text"),
                    next: string(&error["next"]),
                }),
            };
            (string(&connector["name"]).expect("a name"), shown)
        })
        .collect()
}

/// The text of `value`, or `None` when it is not a string.
fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}
