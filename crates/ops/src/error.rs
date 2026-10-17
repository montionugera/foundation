use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use std::path::PathBuf;

use document::Span;
use document::diagnostic::{Code, Diagnostic};
use mesh::used::{Behind, Cause};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use types::name::Name;
use wire::hub::client::BODY_BYTES_MAX;

use crate::front_end::File;
use crate::scope::Limits;

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
const CHANNEL_READ: Code = Code::new("ops.channel-read");
const READ: Code = Code::new("ops.read");
const WRITE: Code = Code::new("ops.write");
const LARGE_REQUEST: Code = Code::new("ops.large-request");
const BAD_REPLY: Code = Code::new("ops.bad-reply");
const OUTSIDE: Code = Code::new("ops.outside");
const HIDDEN: Code = Code::new("ops.hidden");
const NO_DATA: Code = Code::new("ops.no-data");
const FOLDER_CHANGED: Code = Code::new("ops.folder-changed");
const PLAN_NAME: Code = Code::new("ops.plan-name");
const WIDE_FOLDER: Code = Code::new("ops.wide-folder");
const FOLDER: Code = Code::new("ops.folder");
const DATA: Code = Code::new("ops.data");
/// The fix of [`ARGUMENT`].
const HELP: &str =
    "Match the arguments to `foundation --help` or `foundation <operation> --help`";

/// Where the caller of an operation finds the names of the operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Among {
    /// The commands of `foundation --help`.
    Commands,
    /// The tools of `tools/list` in MCP.
    Tools,
}

/// Why a command line did not run to its end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// An argument is missing, unknown, or has a bad value.
    Argument { message: String },
    /// No operation has this name. `closest` is the nearest name, if one is near.
    Unknown {
        name: String,
        closest: Option<String>,
        among: Among,
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
    /// The command line did not run, its node did not start or stopped with an error,
    /// or the command line did not reach its node.
    Node(Failure),
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
    /// A reader of the status channels of `connector` did not open or ended, with the
    /// text of the hub.
    ChannelRead { connector: Name, message: String },
    /// A command line did not read the file at `path`.
    Read { path: PathBuf, cause: Unread },
    /// A command line did not write the plan file at `path`.
    Write { path: PathBuf, cause: Unwritten },
    /// The request of a command line has more bytes than one request holds.
    LargeRequest,
    /// The reply of a node to a command line is no reply.
    Reply { message: String },
    /// The errors that a node gave a command line, and the exit status of the reply.
    Remote { problems: Vec<Problem>, status: u8 },
    /// A path that resolves outside the folders of `foundation mcp`, with them.
    Outside { path: PathBuf, limits: Limits },
    /// A path that resolves under the data directory `data` of `foundation mcp`.
    Hidden { path: PathBuf, data: PathBuf },
    /// A new name in a directory on the way to the data directory `data` of
    /// `foundation mcp`, while nothing is at `data`.
    NoData { path: PathBuf, data: PathBuf },
    /// A path under a folder of `foundation mcp` that a program replaced after the
    /// start, with the folders.
    FolderChanged {
        path: PathBuf,
        folders: Vec<PathBuf>,
    },
    /// An `out` of a tool of `foundation mcp` that resolves to `resolved`, which does
    /// not end in `.plan`.
    PlanName { path: PathBuf, resolved: PathBuf },
    /// `foundation mcp` with no `--files`, in a working directory that is `/` or the
    /// home folder.
    WideFolder { working: PathBuf },
    /// A folder of `--files` of `foundation mcp` that it cannot use.
    Folder { path: PathBuf, cause: Unusable },
    /// The data directory of `foundation mcp`, which it did not make.
    Data {
        path: PathBuf,
        error: env::paths::Error,
    },
}

/// Why a command line did not read a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unread {
    NotFound,
    Directory,
    NotUtf8,
    Io { code: i32 },
}

/// Why `foundation mcp` cannot use a folder of `--files`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unusable {
    /// A file is at it.
    File,
    /// It did not resolve or read.
    Failed(env::paths::Error),
}

/// Why a command line did not write a plan file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unwritten {
    NotFound,
    Io { code: i32 },
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
    /// of `sources`.
    pub(crate) fn config(diagnostics: Vec<Diagnostic>, sources: &Sources) -> Self {
        Self::Config(
            diagnostics
                .into_iter()
                .map(|diagnostic| Problem::of(diagnostic, sources))
                .collect(),
        )
    }

    pub(crate) fn status(&self) -> u8 {
        match self {
            Self::Remote { status, .. } => *status,
            Self::Read {
                cause: Unread::Io { .. },
                ..
            }
            | Self::Input { .. }
            | Self::Output { .. }
            | Self::Behind(_)
            | Self::Stale { .. }
            | Self::Apply(_)
            | Self::Node(_)
            | Self::Stopped(_)
            | Self::Request { .. }
            | Self::Denied { .. }
            | Self::LargeReply
            | Self::ChannelRead { .. }
            | Self::Write { .. }
            | Self::Data { .. }
            | Self::NoData { .. }
            | Self::FolderChanged { .. }
            | Self::Reply { .. } => 1,
            Self::Argument { .. }
            | Self::Unknown { .. }
            | Self::Config(_)
            | Self::Plan(_)
            | Self::Read { .. }
            | Self::LargeRequest
            | Self::Outside { .. }
            | Self::Hidden { .. }
            | Self::PlanName { .. }
            | Self::WideFolder { .. }
            | Self::Folder { .. } => 2,
        }
    }

    /// Each problem: for each error but `Config`, one with no place and no note.
    #[expect(clippy::too_many_lines, reason = "one arm for each error")]
    pub(crate) fn problems(&self) -> Cow<'_, [Problem]> {
        let (code, fix) = match self {
            Self::Config(problems) | Self::Remote { problems, .. } => {
                return Cow::Borrowed(problems);
            }
            Self::Node(failure) => (failure.code, failure.fix.clone()),
            Self::Argument { .. } => (ARGUMENT, HELP.to_owned()),
            Self::Unknown {
                closest: Some(closest),
                ..
            } => (UNKNOWN, format!("Use `{closest}`, the closest name")),
            Self::Unknown {
                closest: None,
                among,
                ..
            } => {
                let list = match among {
                    Among::Commands => "foundation --help",
                    Among::Tools => "tools/list",
                };
                (UNKNOWN, format!("Use a name from `{list}`"))
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
            Self::ChannelRead { .. } => (
                CHANNEL_READ,
                "Fix the cause in the message, then ask for the status again"
                    .to_owned(),
            ),
            Self::Read { .. } => (
                READ,
                "Fix the cause in the message, then run the command again".to_owned(),
            ),
            Self::Write { .. } => (
                WRITE,
                "Give `--out`, or `out` of the MCP tool, a path in a directory that can \
                 be written"
                    .to_owned(),
            ),
            Self::LargeRequest => {
                (LARGE_REQUEST, "Make the config files smaller".to_owned())
            }
            Self::Reply { .. } => (
                BAD_REPLY,
                "Use a `foundation` program of the same version as the node".to_owned(),
            ),
            Self::Outside { limits, .. } => (
                OUTSIDE,
                format!(
                    "Give a path under one of those folders. A relative path is relative \
                     to the working directory {}. To reach another folder, add \
                     `--files <DIR>` to the command of the MCP server",
                    limits.working.display()
                ),
            ),
            Self::PlanName { .. } => (
                PLAN_NAME,
                "Give `out` a path that ends in `.plan` and is not a link to another \
                 file"
                    .to_owned(),
            ),
            Self::WideFolder { .. } => (
                WIDE_FOLDER,
                "Add `--files <DIR>` for each folder that the tools may read and write"
                    .to_owned(),
            ),
            Self::Hidden { data, .. } => {
                (HIDDEN, format!("Give a path outside {}", data.display()))
            }
            Self::NoData { data, .. } => (
                NO_DATA,
                format!(
                    "Start the node on {} again, or give a path in another directory",
                    data.display()
                ),
            ),
            Self::FolderChanged { .. } => {
                (FOLDER_CHANGED, "Start `foundation mcp` again".to_owned())
            }
            Self::Folder { .. } => {
                (FOLDER, "Give `--files` a folder that is there".to_owned())
            }
            Self::Data {
                path,
                error: env::paths::Error::NotFound,
            } => (
                DATA,
                format!(
                    "Make {}, or give `--data` a directory whose parent is there",
                    above(path).display()
                ),
            ),
            Self::Data { .. } => (
                DATA,
                "Give `--data` a directory that this user can make".to_owned(),
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

    /// Each problem for a terminal, with one empty line between two. In `Config` and
    /// `Remote`, each control character is written as its Rust escape, because a
    /// config producer or the node quotes the text of a file itself. In each other
    /// error, so are a backslash and each character that does not print, because that
    /// text holds the caller's text as given.
    pub(crate) fn text(&self) -> String {
        let escape: fn(&str) -> String = match self {
            Self::Config(_) | Self::Remote { .. } => escape_controls,
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

/// Why the command line did not run, why its node did not start or stopped with an
/// error, or why the command line did not reach its node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What kind of failure it is, such as `node.busy`.
    pub code: Code,
    /// What went wrong.
    pub message: String,
    /// What to do about it.
    pub fix: String,
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
    /// The problem of `diagnostic`, whose spans are in the files of `sources`.
    pub(crate) fn of(diagnostic: Diagnostic, sources: &Sources) -> Self {
        Self {
            code: diagnostic.code.as_str().to_owned(),
            message: diagnostic.message,
            fix: diagnostic.fix,
            place: diagnostic.span.map(|span| sources.place(span)),
            notes: diagnostic
                .notes
                .into_iter()
                .map(|note| Note {
                    text: note.text,
                    place: sources.place(note.span),
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
pub(crate) struct Sources(Vec<Option<Arc<str>>>);

impl Sources {
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
    #[expect(clippy::too_many_lines, reason = "one arm for each error")]
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
            Self::Node(failure) => f.write_str(&failure.message),
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
            Self::ChannelRead { connector, message } => write!(
                f,
                "the status channels of the connector `{connector}` did not read: \
                 {message}"
            ),
            Self::Read { path, cause } => {
                let path = path.display();
                match cause {
                    Unread::NotFound => write!(f, "no file is at {path}"),
                    Unread::Directory => write!(f, "{path} is a directory, not a file"),
                    Unread::NotUtf8 => write!(f, "the text of {path} is not UTF-8"),
                    Unread::Io { code } => {
                        write!(f, "the read of {path} failed with OS error {code}")
                    }
                }
            }
            Self::Write { path, cause } => {
                let path = path.display();
                match cause {
                    Unwritten::NotFound => {
                        write!(f, "the directory of the plan file {path} is not there")
                    }
                    Unwritten::Io { code } => write!(
                        f,
                        "the write of the plan file {path} failed with OS error {code}"
                    ),
                }
            }
            Self::LargeRequest => write!(
                f,
                "the request has more than {BODY_BYTES_MAX} bytes, the most that a \
                 request holds"
            ),
            Self::Reply { message } => {
                write!(f, "the reply of the node is no reply: {message}")
            }
            Self::Remote { problems, .. } => match problems.as_slice() {
                [one] => f.write_str(&one.message),
                _ => write!(f, "the node gave {} problems", problems.len()),
            },
            Self::Outside { path, limits } => write!(
                f,
                "`{}` is not under a folder of this MCP server ({})",
                path.display(),
                listed(&limits.folders)
            ),
            Self::FolderChanged { path, folders } => write!(
                f,
                "`{}` is under a folder of this MCP server ({}) that a program \
                 replaced after it started",
                path.display(),
                listed(folders)
            ),
            Self::PlanName { path, resolved } => write!(
                f,
                "the plan file `{}` resolves to {}, which does not end in `.plan`",
                path.display(),
                resolved.display()
            ),
            Self::WideFolder { working } => write!(
                f,
                "the working directory {} is the root or the home folder, which is too \
                 wide for the tools of an MCP server",
                working.display()
            ),
            Self::Hidden { path, data } => write!(
                f,
                "`{}` is in the data directory {} of this MCP server, which no tool \
                 reads or writes",
                path.display(),
                data.display()
            ),
            Self::NoData { path, data } => write!(
                f,
                "`{}` is a new name in a directory on the way to the data directory {} \
                 of this MCP server, and nothing is at {} now",
                path.display(),
                data.display(),
                data.display()
            ),
            Self::Folder { path, cause } => {
                let failed = match cause {
                    Unusable::File => "is a file".to_owned(),
                    Unusable::Failed(error) => failed(*error),
                };
                write!(f, "the folder `{}` of `--files` {failed}", path.display())
            }
            Self::Data {
                path,
                error: env::paths::Error::NotFound,
            } => write!(
                f,
                "the directory {} above the data directory `{}` is not there",
                above(path).display(),
                path.display()
            ),
            Self::Data { path, error } => write!(
                f,
                "the data directory `{}` was not made: it {}",
                path.display(),
                failed(*error)
            ),
        }
    }
}

/// What a call of `env::paths` at a path did, after the path.
fn failed(error: env::paths::Error) -> String {
    match error {
        env::paths::Error::NotFound => "is not there".to_owned(),
        env::paths::Error::Io { code } => format!("failed with OS error {code}"),
        env::paths::Error::Outside => "is outside the folders it may use".to_owned(),
        env::paths::Error::Hidden => "is in a folder that is hidden".to_owned(),
        env::paths::Error::Vacant => {
            "is a new name on the way to a hidden folder that is not there".to_owned()
        }
        env::paths::Error::Replaced => {
            "is under a folder that a program replaced".to_owned()
        }
    }
}

/// The paths of `folders`, joined with `, `.
fn listed(folders: &[PathBuf]) -> String {
    let folders: Vec<_> = folders
        .iter()
        .map(|folder| folder.display().to_string())
        .collect();
    folders.join(", ")
}

/// The directory above `path`, as given, or `.` for a path of one name.
fn above(path: &std::path::Path) -> &std::path::Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
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
