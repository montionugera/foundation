use std::net::SocketAddr;
use std::path::PathBuf;

use document::diagnostic::Code;
use serde_json::Value;
use types::name::Name;

use crate::error;

/// The arguments of `foundation start`.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Start {
    /// `--data`, else `foundation-data` in the working directory.
    pub data: PathBuf,
    /// `--json`: each line is JSON.
    pub json: bool,
    /// `--name`: the first start on a data directory needs it, and a later start reads
    /// it from there.
    pub name: Option<Name>,
    /// `--listen`: the address of the node's one port, else port 0 of `127.0.0.1`.
    pub listen: SocketAddr,
}

/// Why a node did not start, stopped with an error, or why a command line did not
/// reach its node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What kind of failure it is, such as `node.busy`.
    pub code: Code,
    /// What went wrong.
    pub message: String,
    /// What to do about it.
    pub fix: String,
}

impl Start {
    /// The line that tells that the node `name` runs in [`Start::data`], as text or
    /// JSON, with its newline. Each gives the data directory lossily, as
    /// `Path::to_string_lossy` does. Text also escapes it as [`crate::fail`] does: the
    /// line stays one line, and a literal `\n` in the path reads apart from a newline.
    #[must_use]
    pub fn line(&self, name: &Name) -> String {
        if self.json {
            // By hand, as a `json!` object sorts its keys.
            let name = Value::from(name.as_str());
            let data = Value::from(self.data.to_string_lossy());
            format!("{{\"name\":{name},\"data\":{data}}}\n")
        } else {
            let data = error::escape(&self.data.to_string_lossy());
            format!("node {name} runs in {data}. Stop it with Ctrl-C.\n")
        }
    }
}
