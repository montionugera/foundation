//! Where a secret's value comes from, as a config writes it.

use std::fmt;

use document::diagnostic::{Code, Diagnostic};
use document::value::{Kind, Value};
use env::files::Absolute;
use types::name::Name;

/// A config that writes a source of a secret wrongly.
const SOURCE: Code = Code::new("secret.source");

/// Where the value of a secret comes from, as a config writes it: a call of `env`,
/// `file`, or `secret` with one string, or for `secret` also a reference. `Display`
/// gives the call with its argument as a string, never the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// `env("NAME")`: the variable `NAME` of the environment of the node process, as
    /// it was when the node started.
    Env(String),
    /// `file("/path")`: the file at an [`Absolute`] path.
    File(Absolute),
    /// `secret("full.name")`: the value of the secret of that full name in the
    /// node's store.
    Secret(Name),
}

/// A call that gives the value of a secret. `Display` gives it as a config writes it,
/// such as `env("INFLUX_TOKEN")`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    name: &'static str,
    /// An argument that [`Source::read`] takes.
    argument: &'static str,
    summary: &'static str,
    function: Function,
}

impl Call {
    /// One sentence on where the value comes from.
    #[must_use]
    pub const fn summary(&self) -> &'static str {
        self.summary
    }
}

/// Each call that [`Source::read`] takes, in the order that the docs give them.
pub const CALLS: &[Call] = &[
    Call {
        name: "env",
        argument: "INFLUX_TOKEN",
        summary: "The variable of that name in the environment of the node process, \
                  as it was when the process began.",
        function: Function::Env,
    },
    Call {
        name: "file",
        argument: "/run/secrets/influx_token",
        summary: "The file at that absolute path with no `..`, on the node that runs \
                  the connector.",
        function: Function::File,
    },
    Call {
        name: "secret",
        argument: "influx.token",
        summary: "The secret of that full name in the node's store. The name can also \
                  be a reference.",
        function: Function::Secret,
    },
];

impl Source {
    /// Reads the source that `value` writes. `secret` also takes its name as a
    /// reference, as each name in a config does.
    ///
    /// # Errors
    ///
    /// `secret.source` at the span of `value` when it is not a call of `env`,
    /// `file`, or `secret` with one valid string, or for `secret` also a reference.
    /// The text never quotes `value`.
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
        let Some(known) = CALLS.iter().find(|known| known.name == function) else {
            return Err(refuse(
                "a secret comes from `env`, `file`, or `secret`, not another function"
                    .into(),
                FIX,
            ));
        };
        let [argument] = call.arguments.as_slice() else {
            return Err(refuse(
                format!(
                    "`{function}` takes one argument, not {}",
                    call.arguments.len()
                ),
                FIX,
            ));
        };
        let text = match (&argument.kind, known.function) {
            (Kind::String(text), _) => text,
            (Kind::Reference(name), Function::Secret) => {
                return Ok(Self::Secret(name.clone()));
            }
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
        match known.function {
            Function::Env if text.is_empty() || text.contains(['=', '\0']) => {
                Err(refuse(
                    "the argument of `env` is not a variable name".into(),
                    "Write a name with no `=` and no NUL, such as `env(\"INFLUX_TOKEN\")`",
                ))
            }
            Function::Env => Ok(Self::Env(text.to_string())),
            Function::File => Absolute::new(text).map(Self::File).ok_or_else(|| {
                let message = "the argument of `file` is not an absolute path with no \
                               `..` and no NUL";
                refuse(message.into(), FILE_FIX)
            }),
            Function::Secret => text.parse().map(Self::Secret).map_err(
                |error: types::name::Error| {
                    let message = match error {
                        types::name::Error::Segment { .. } => {
                            "a segment of the argument of `secret` is not valid"
                        }
                        types::name::Error::Wildcard { .. } => {
                            "the argument of `secret` holds a wildcard, which a name \
                             never does"
                        }
                        _ => "the argument of `secret` is not a valid name",
                    };
                    refuse(message.into(), error.fix())
                },
            ),
        }
    }
}

/// The function of a [`Call`], which [`Source::read`] matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Function {
    Env,
    File,
    Secret,
}

impl fmt::Display for Call {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_call(f, self.name, self.argument)
    }
}

/// `name` called with `argument` as a string, as a config writes it.
fn write_call(f: &mut fmt::Formatter<'_>, name: &str, argument: &str) -> fmt::Result {
    write!(f, "{name}({argument:?})")
}

/// How to write the path of `file`.
const FILE_FIX: &str =
    "Write the whole path from the root, such as `file(\"/run/secrets/influx_token\")`";

/// How to write a source.
const FIX: &str =
    "Write `env(\"NAME\")`, `file(\"/absolute/path\")`, or `secret(\"name\")`";

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write_call(f, "env", name),
            Self::File(path) => {
                write_call(f, "file", &path.as_path().to_string_lossy())
            }
            Self::Secret(name) => write_call(f, "secret", &name.to_string()),
        }
    }
}

#[cfg(test)]
mod tests;
