//! Where a secret's value comes from, as a config writes it.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use document::diagnostic::{Code, Diagnostic};
use document::value::{Kind, Value};
use types::name::Name;

/// A config that writes a source of a secret wrongly.
const SOURCE: Code = Code::new("secret.source");

/// Where the value of a secret comes from, as a config writes it: a call of `env`,
/// `file`, or `secret` with one string. `Display` gives the call with its argument
/// as a string, never the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// `env("NAME")`: the variable `NAME` of the environment of the node process, as
    /// it was when the node started.
    Env(String),
    /// `file("/path")`: the file at an absolute path with no `..` segment.
    File(PathBuf),
    /// `secret("full.name")`: the value of the secret of that full name in the
    /// node's store.
    Secret(Name),
}

impl Source {
    /// Reads the source that `value` writes. `secret` also takes its name as a
    /// reference, as each name in a config does.
    ///
    /// # Errors
    ///
    /// `secret.source` at the span of `value` when it is not a call of `env`,
    /// `file`, or `secret` with one valid string. The text never quotes `value`.
    pub fn read(value: &Value) -> Result<Self, Diagnostic> {
        let refuse = |message: String, fix: &str| {
            Diagnostic::new(SOURCE, value.span, message, fix.into())
        };
        let Kind::Call(call) = &value.kind else {
            return Err(refuse(
                "a secret takes where its value comes from, never the value".into(),
                FIX,
            ));
        };
        let function = &*call.function;
        if !["env", "file", "secret"].contains(&function) {
            return Err(refuse(
                "a secret comes from `env`, `file`, or `secret`, not another function"
                    .into(),
                FIX,
            ));
        }
        let [argument] = call.arguments.as_slice() else {
            return Err(refuse(
                format!(
                    "`{function}` takes one argument, not {}",
                    call.arguments.len()
                ),
                FIX,
            ));
        };
        let text = match (&argument.kind, function) {
            (Kind::String(text), _) => text,
            (Kind::Reference(name), "secret") => return Ok(Self::Secret(name.clone())),
            (kind, _) => {
                return Err(refuse(
                    format!(
                        "the argument of `{function}` is {}, not a string",
                        kind.noun()
                    ),
                    FIX,
                ));
            }
        };
        match function {
            "env" if text.is_empty() || text.contains(['=', '\0']) => Err(refuse(
                "the argument of `env` is not a variable name".into(),
                "Write a name with no `=` and no NUL, such as `env(\"INFLUX_TOKEN\")`",
            )),
            "env" => Ok(Self::Env(text.to_string())),
            "file" if !absolute(Path::new(&**text)) => Err(refuse(
                "the argument of `file` is not an absolute path with no `..`".into(),
                "Write the whole path from the root, such as \
                 `file(\"/run/secrets/influx_token\")`",
            )),
            "file" => Ok(Self::File(PathBuf::from(&**text))),
            _ => text
                .parse()
                .map(Self::Secret)
                .map_err(|error: types::name::Error| {
                    refuse(
                        "the argument of `secret` is not a valid name".into(),
                        error.fix(),
                    )
                }),
        }
    }
}

/// How to write a source.
const FIX: &str =
    "Write `env(\"NAME\")`, `file(\"/absolute/path\")`, or `secret(\"name\")`";

/// Whether `path` is absolute with no `..` segment.
fn absolute(path: &Path) -> bool {
    path.is_absolute() && !path.components().any(|c| c == Component::ParentDir)
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(f, "env({name:?})"),
            Self::File(path) => write!(f, "file({:?})", path.to_string_lossy()),
            Self::Secret(name) => write!(f, "secret(\"{name}\")"),
        }
    }
}

#[cfg(test)]
mod tests;
