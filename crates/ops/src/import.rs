//! The output of `import`: the channels that it added to a config file, and the picks
//! that it did not add.

use std::fmt::Write as _;

use config::import::{Added, Outcome, Pick};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::escape_controls;

#[cfg(test)]
mod tests;

/// What an import wrote, as `import` gives it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Output {
    /// The path of the config file that it wrote, as the user gave it.
    pub(crate) file: String,
    /// Each channel that it added, in the order of the picks.
    pub(crate) channels: Vec<Channel>,
    /// The index channel that it added, or `null` when it added none.
    pub(crate) index: Option<String>,
    /// Each channel with a value that the import did not find, which `plan` refuses
    /// until it is written.
    pub(crate) fills: Vec<Fill>,
    /// Each point that it did not add, because its default name is too long.
    pub(crate) unnamed: Vec<Unnamed>,
    /// The count of picks that the config file reads or writes already.
    pub(crate) present: usize,
    /// The count of points of the match that a connector of the kind cannot read or
    /// write, which it left out.
    pub(crate) unreadable: usize,
}

/// One channel that an import added.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Channel {
    pub(crate) name: String,
    /// The path of its point in the device.
    pub(crate) path: String,
    /// The sample type, as a `type` attribute takes it, or `null` when the device does
    /// not give it.
    #[serde(rename = "type")]
    pub(crate) data_type: Option<String>,
}

/// The values of one channel that the import did not find.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Fill {
    /// The name of the channel.
    pub(crate) name: String,
    /// What each value is, such as "the sample type".
    pub(crate) what: Vec<String>,
}

/// One point that an import did not add.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Unnamed {
    /// The path of the point in the device.
    pub(crate) path: String,
    /// What to do about it.
    pub(crate) fix: String,
}

impl Output {
    /// The output of `added`, the import of `picks` into the file at `file`, which
    /// left out `unreadable` points.
    ///
    /// # Panics
    ///
    /// When `added` holds no outcome for each pick.
    pub(crate) fn of(
        file: String,
        picks: &[Pick<'_>],
        added: &Added,
        unreadable: usize,
    ) -> Self {
        assert_eq!(
            picks.len(),
            added.picks.len(),
            "invariant: an import gives one outcome for each pick"
        );
        let mut output = Self {
            file,
            channels: Vec::new(),
            index: added.index.as_ref().map(ToString::to_string),
            fills: Vec::new(),
            unnamed: Vec::new(),
            present: 0,
            unreadable,
        };
        for (pick, outcome) in picks.iter().zip(&added.picks) {
            match outcome {
                Outcome::Added { name, fills } => {
                    output.channels.push(Channel {
                        name: name.to_string(),
                        path: pick.path.to_string(),
                        data_type: pick.facts.data_type.map(|t| t.to_string()),
                    });
                    if !fills.is_empty() {
                        output.fills.push(Fill {
                            name: name.to_string(),
                            what: fills.clone(),
                        });
                    }
                }
                Outcome::Unnamed => output.unnamed.push(Unnamed {
                    path: pick.path.to_string(),
                    fix: "Give it a name".to_owned(),
                }),
                Outcome::Present => output.present += 1,
            }
        }
        output
    }

    /// One line for the file and the count of channels, one for each channel as
    /// `<name> <path> <type>` with `?` for a type that the device does not give, then
    /// one for the index, each fill, each unnamed point, and each count that is not 0.
    pub(crate) fn text(&self) -> String {
        let mut lines = vec![format!(
            "The import added {} to {}.",
            count(self.channels.len(), "channel", "channels"),
            escape_controls(&self.file)
        )];
        for channel in &self.channels {
            lines.push(format!(
                "{} {} {}",
                channel.name,
                escape_controls(&channel.path),
                channel.data_type.as_deref().unwrap_or("?")
            ));
        }
        if let Some(index) = &self.index {
            lines.push(format!("It added the index channel {index}."));
        }
        for fill in &self.fills {
            let what: Vec<String> =
                fill.what.iter().map(|what| escape_controls(what)).collect();
            lines.push(format!(
                "Write in place of each `fill` of {}: {}.",
                fill.name,
                what.join("; ")
            ));
        }
        for unnamed in &self.unnamed {
            lines.push(format!(
                "It did not add {}, as its default name is too long. {}.",
                escape_controls(&unnamed.path),
                unnamed.fix
            ));
        }
        if self.present > 0 {
            lines.push(format!(
                "It did not add {} that the file reads or writes already.",
                count(self.present, "point", "points")
            ));
        }
        if self.unreadable > 0 {
            lines.push(format!(
                "It left out {} that the connector cannot read or write.",
                count(self.unreadable, "point", "points")
            ));
        }
        let mut text = String::new();
        for line in lines {
            writeln!(text, "{line}").expect("invariant: a String takes each write");
        }
        text
    }
}

/// `n` with `one` when it is 1, else with `many`.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
