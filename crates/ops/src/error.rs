use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use document::Span;
use document::diagnostic::{Code, Diagnostic};
use mesh::used::{Behind, Cause};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use types::name::Name;
use wire::hub::client::BODY_BYTES_MAX;

use crate::Failure;
use crate::front_end::File;

const ARGUMENT: Code = Code::new("ops.argument");
const UNKNOWN: Code = Code::new("ops.unknown-operation");
const INPUT: Code = Code::new("ops.input");
const OUTPUT: Code = Code::new("ops.output");
const BAD_PLAN: Code = Code::new("ops.bad-plan");
const BEHIND: Code = Code::new("ops.behind");
const STALE_PLAN: Code = Code::new("ops.stale-plan");
const APPLY: Code = Code::new("ops.apply");
const STOPPED: Code = Code::new("ops.stopped");
const BAD_REQUEST: Code = Code::new("ops.bad-request");
const DENIED: Code = Code::new("ops.denied");
const LARGE_REPLY: Code = Code::new("ops.large-reply");
/// The fix of [`ARGUMENT`].
const HELP: &str =
    "Match the arguments to `foundation --help` or `foundation <operation> --help`";

/// Why a command line did not run to its end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// An argument is missing, unknown, or has a bad value.
    Argument { message: String },
    /// No operation has this name. `closest` is the nearest name, if one is near.
    Unknown {
        name: String,
        closest: Option<String>,
    },
    /// Standard input could not be read.
    Input { message: String },
    /// Standard output could not be written.
    Output { message: String },
    /// The config files have problems: at least one.
    Config(Vec<Problem>),
    /// The plan file holds no plan that `plan` makes.
    Plan(config::plan::Error),
    /// The node does not use the newest spec, so it can neither plan nor apply.
    Behind(Box<Behind>),
    /// The spec in use is at `pointer`, not at `base`, the spec that the plan changes.
    Stale {
        base: spec::Pointer,
        pointer: spec::Pointer,
    },
    /// The region did not apply the plan.
    Apply(mesh::Error),
    /// The node did not start, or stopped with an error.
    Start(Failure),
    /// The group of the mesh stopped, so the node can neither plan nor apply.
    Stopped(mesh::Stopped),
    /// The body that a node got is no request.
    Request { message: String },
    /// No access policy gives `subject` the action `action` on `on`.
    Denied {
        subject: Name,
        action: Action,
        on: On,
    },
    /// The reply of a node has more bytes than one reply holds.
    LargeReply,
}

/// An action that an operation on a node needs: the action of its name, and `Admin`
/// on a changed key of a definition other than a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Read,
    Plan,
    Apply,
    Admin,
}

/// What a refusal of an operation on a node names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum On {
    /// A name that the request holds.
    Name(Name),
    /// A definition of the spec in use that the files do not hold.
    Spec,
    /// No name: the request names no definition.
    Nothing,
}

impl fmt::Display for On {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(f, "`{name}`"),
            Self::Spec => f.write_str(
                "a definition of the spec in use that the files do not hold",
            ),
            Self::Nothing => f.write_str("a request that names no definition"),
        }
    }
}

impl Action {
    /// The word of the action in a config file.
    const fn word(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Plan => "plan",
            Self::Apply => "apply",
            Self::Admin => "admin",
        }
    }
}

impl From<Action> for spec::access::Action {
    fn from(action: Action) -> Self {
        match action {
            Action::Read => Self::Read,
            Action::Plan => Self::Plan,
            Action::Apply => Self::Apply,
            Action::Admin => Self::Admin,
        }
    }
}

impl Error {
    /// `Config` with the problem of each of `diagnostics`, whose spans are in the files
    /// at `paths`.
    pub(crate) fn config(diagnostics: Vec<Diagnostic>, paths: &Paths) -> Self {
        Self::Config(
            diagnostics
                .into_iter()
                .map(|diagnostic| Problem::of(diagnostic, paths))
                .collect(),
        )
    }

    pub(crate) fn status(&self) -> u8 {
        match self {
            Self::Argument { .. }
            | Self::Unknown { .. }
            | Self::Config(_)
            | Self::Plan(_) => 2,
            Self::Input { .. }
            | Self::Output { .. }
            | Self::Behind(_)
            | Self::Stale { .. }
            | Self::Apply(_)
            | Self::Start(_)
            | Self::Stopped(_)
            | Self::Request { .. }
            | Self::Denied { .. }
            | Self::LargeReply => 1,
        }
    }

    /// Each problem: for each error but `Config`, one with no place and no note.
    pub(crate) fn problems(&self) -> Cow<'_, [Problem]> {
        let (code, fix) = match self {
            Self::Config(problems) => return Cow::Borrowed(problems),
            Self::Start(failure) => (failure.code, failure.fix.clone()),
            Self::Argument { .. } => (ARGUMENT, HELP.to_owned()),
            Self::Unknown {
                closest: Some(closest),
                ..
            } => (UNKNOWN, format!("Use `{closest}`, the closest name")),
            Self::Unknown { closest: None, .. } => {
                (UNKNOWN, "Use a name from `foundation docs`".to_owned())
            }
            Self::Input { .. } => (
                INPUT,
                "Give standard input a source that can be read".to_owned(),
            ),
            Self::Output { .. } => (
                OUTPUT,
                "Give standard output a destination that can be written".to_owned(),
            ),
            Self::Plan(_) => (
                BAD_PLAN,
                "Make a plan with `foundation plan`, and apply it with no edits"
                    .to_owned(),
            ),
            Self::Behind(_) => (BEHIND, "Fix the cause, then plan again".to_owned()),
            Self::Stale { .. } => (STALE_PLAN, "Plan again".to_owned()),
            Self::Apply(_) => (
                APPLY,
                "Fix the cause in the message, then plan and apply again".to_owned(),
            ),
            Self::Stopped(_) => (
                STOPPED,
                "Fix the cause in the message, then start the node and plan again"
                    .to_owned(),
            ),
            Self::Request { .. } => (
                BAD_REQUEST,
                "Use a `foundation` program of the same version as the node".to_owned(),
            ),
            Self::Denied { .. } => (
                DENIED,
                "Ask an admin of the region for an access policy that allows it"
                    .to_owned(),
            ),
            Self::LargeReply => (
                LARGE_REPLY,
                "Plan and apply the change in smaller steps".to_owned(),
            ),
        };
        Cow::Owned(vec![Problem {
            code: code.as_str().to_owned(),
            message: self.to_string(),
            fix,
            place: None,
            notes: Vec::new(),
        }])
    }

    /// `{"errors": [...]}`, with each problem.
    pub(crate) fn json(&self) -> Value {
        json!({ "errors": self.problems() })
    }

    /// Each problem for a terminal, with one empty line between two. In `Config`, each
    /// control character is written as its Rust escape, because a config producer
    /// quotes the text of a file itself. In each other error, so are a backslash and
    /// each character that does not print, because that text holds the caller's text
    /// as given.
    pub(crate) fn text(&self) -> String {
        let escape: fn(&str) -> String = match self {
            Self::Config(_) => escape_controls,
            _ => escape,
        };
        let problems: Vec<String> = self
            .problems()
            .iter()
            .map(|problem| problem.text(escape))
            .collect();
        problems.join("\n")
    }
}

/// One problem, as the JSON and the text of each error give it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Problem {
    /// What kind of problem it is, such as `ops.argument`.
    pub(crate) code: String,
    /// What is wrong.
    pub(crate) message: String,
    /// What to do about it.
    pub(crate) fix: String,
    /// Where the problem is, if it is in a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) place: Option<Place>,
    /// Other places that the problem names.
    pub(crate) notes: Vec<Note>,
}

impl Problem {
    /// The problem of `diagnostic`, whose spans are in the files at `paths`.
    pub(crate) fn of(diagnostic: Diagnostic, paths: &Paths) -> Self {
        Self {
            code: diagnostic.code.as_str().to_owned(),
            message: diagnostic.message,
            fix: diagnostic.fix,
            place: diagnostic.span.map(|span| paths.place(span)),
            notes: diagnostic
                .notes
                .into_iter()
                .map(|note| Note {
                    text: note.text,
                    place: paths.place(note.span),
                })
                .collect(),
        }
    }

    /// `error[<code>]: <message>`, its place, `fix: <fix>`, then each note and its
    /// place, with `escape` on each text.
    fn text(&self, escape: fn(&str) -> String) -> String {
        let place = self.place.as_ref().map(Place::to_string);
        let notes: Vec<String> = self
            .notes
            .iter()
            .map(|note| format!("note: {}\n{}", escape(&note.text), note.place))
            .collect();
        format!(
            "error[{}]: {}\n{}fix: {}\n{}",
            self.code,
            escape(&self.message),
            place.unwrap_or_default(),
            escape(&self.fix),
            notes.concat()
        )
    }
}

/// A place that a problem names, and what is there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Note {
    /// What is at the place.
    pub(crate) text: String,
    /// The place.
    pub(crate) place: Place,
}

/// A file, and a line and a column in it, each from 1. The column counts Unicode
/// scalar values, as rustc does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Place {
    /// The exact path, as the user or a directory listing gave it. A path that is not
    /// UTF-8 gives `ops.path-not-utf8` instead.
    pub(crate) file: Arc<str>,
    /// The line, from 1.
    pub(crate) line: u32,
    /// The column, from 1, in Unicode scalar values.
    pub(crate) column: u32,
}

/// The path of each file of a plan, by source. Each place in a file shares its path,
/// so the problems of a file do not repeat it in memory.
#[derive(Default)]
pub(crate) struct Paths(Vec<Option<Arc<str>>>);

impl Paths {
    /// The path of each of `files`, by source.
    pub(crate) fn of(files: &[File]) -> Self {
        Self(
            files
                .iter()
                .map(|file| file.path.to_str().map(Arc::from))
                .collect(),
        )
    }

    /// The start of `span`, in the file of its source.
    pub(crate) fn place(&self, span: Span) -> Place {
        let path = usize::try_from(span.source().0)
            .ok()
            .and_then(|source| self.0.get(source))
            .expect("invariant: a span is in a file of the plan");
        let start = span.start();
        Place {
            file: Arc::clone(
                path.as_ref()
                    .expect("invariant: `read` refuses a path that is not UTF-8"),
            ),
            line: start.line + 1,
            column: start.column + 1,
        }
    }
}

/// `  --> <file>:<line>:<column>`, as rustc writes a place, and a new line.
impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = escape(&self.file);
        writeln!(f, "  --> {file}:{}:{}", self.line, self.column)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument { message } => f.write_str(message),
            Self::Unknown { name, .. } => write!(f, "no operation is named `{name}`"),
            Self::Input { message } => {
                write!(f, "standard input could not be read: {message}")
            }
            Self::Output { message } => {
                write!(f, "standard output could not be written: {message}")
            }
            Self::Config(problems) => match problems.as_slice() {
                [one] => f.write_str(&one.message),
                _ => write!(f, "the config files have {} problems", problems.len()),
            },
            Self::Plan(error) => error.fmt(f),
            Self::Behind(behind) => {
                let Behind { pointer, cause } = &**behind;
                write!(f, "the node does not use the newest spec, at {pointer}: ")?;
                match cause {
                    Cause::Read(error) => write!(f, "its tree does not read: {error}"),
                    Cause::Problems(problems) => {
                        f.write_str("it has problems at this build: ")?;
                        for (i, problem) in problems.iter().enumerate() {
                            let between = if i == 0 { "" } else { "; " };
                            write!(f, "{between}{problem}")?;
                        }
                        Ok(())
                    }
                    Cause::Blob(error) => {
                        write!(f, "a call of the store failed: {error}")
                    }
                    Cause::Files(error) => write!(
                        f,
                        "the file of the pointer in use was not made durable: {error}"
                    ),
                }
            }
            Self::Stale { base, pointer } => write!(
                f,
                "the spec changed: the spec is at {pointer}, not at the base {base} \
                 of the plan"
            ),
            Self::Apply(error) => error.fmt(f),
            Self::Start(failure) => f.write_str(&failure.message),
            Self::Stopped(stopped) => stopped.fmt(f),
            Self::Request { message } => {
                write!(f, "the body is no request: {message}")
            }
            Self::Denied {
                subject,
                action,
                on,
            } => {
                let action = action.word();
                write!(
                    f,
                    "no access policy gives `{subject}` the action `{action}` on {on}"
                )
            }
            Self::LargeReply => write!(
                f,
                "the reply has more than {BODY_BYTES_MAX} bytes, the most that a reply \
                 holds"
            ),
        }
    }
}

/// `text` with each control character escaped as `char::escape_debug` does, so it
/// stays one line. For a message or a fix whose producer quoted the text of a file.
pub(crate) fn escape_controls(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_debug().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// `text` escaped as `char::escape_debug` does, with quotes kept, so it stays one line.
pub(crate) fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '"' | '\'' => c.to_string(),
            _ => c.escape_debug().to_string(),
        })
        .collect()
}
