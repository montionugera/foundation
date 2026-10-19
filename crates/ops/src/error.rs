use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use std::path::PathBuf;

use connector::browse::{DEADLINE, DEPTH, LIMIT, STARS};
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
const LARGE_BROWSE: Code = Code::new("ops.large-browse");
const LARGE_STATUS: Code = Code::new("ops.large-status");
const CHANNEL_READ: Code = Code::new("ops.channel-read");
const READ: Code = Code::new("ops.read");
const NO_CONFIG: Code = Code::new("ops.no-config");
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
const BROWSE: Code = Code::new("ops.browse");
const BROWSE_NODE: Code = Code::new("ops.browse-node");
const BAD_START: Code = Code::new("ops.bad-start");
const BAD_PATTERN: Code = Code::new("ops.bad-pattern");
const NO_PICK: Code = Code::new("ops.no-pick");
const PICK_LIMIT: Code = Code::new("ops.pick-limit");
const MATCH_LIMIT: Code = Code::new("ops.match-limit");
const BAD_POINT: Code = Code::new("ops.bad-point");
const BAD_POINT_FIX: &str = "Give the names of the path joined by `/`, with a `\\` \
                             before each `/`, `*`, or `\\` in a name";
const DUPLICATE_PICK: Code = Code::new("ops.duplicate-pick");
const BAD_NAME: Code = Code::new("ops.bad-name");
const BAD_NAME_FIX: &str = "Give another name, or leave it out for the default name";
const UNREADABLE_POINT: Code = Code::new("ops.unreadable-point");
const PICK_BUDGET: Code = Code::new("ops.pick-budget");
const MATCH_BUDGET: Code = Code::new("ops.match-budget");
const LARGE_IMPORT: Code = Code::new("ops.large-import");
const FILE_CHANGED: Code = Code::new("ops.file-changed");
const MORE_PROBLEMS: Code = Code::new("ops.more-problems");
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

/// What stopped the walk of an import before it saw each entry that it looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// It reached the budget of the walk: its count of entries or of open folders.
    Budget,
    /// It ran for [`DEADLINE`].
    Deadline,
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Budget => f.write_str("at its budget"),
            Self::Deadline => write!(f, "after {DEADLINE}"),
        }
    }
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
    /// The reply of a browse has more bytes than one reply holds.
    LargeBrowse,
    /// The reply of a status has more bytes than one reply holds.
    LargeStatus,
    /// A reader of the status channels of `connector` did not open or ended, with the
    /// text of the hub.
    ChannelRead { connector: Name, message: String },
    /// A command line did not read the file at `path`.
    Read { path: PathBuf, cause: Unread },
    /// The directory `path` of a command line gives no file with one of
    /// `extensions`, as [`FrontEnds::named`](crate::FrontEnds::named) gives them.
    NoConfig { path: PathBuf, extensions: String },
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
    /// The browse of `connector` of `kind` failed, with each problem of the failure.
    /// Each message starts with the names of the connector and its kind.
    Browse {
        connector: Name,
        kind: Name,
        problems: Vec<Problem>,
    },
    /// The files place `connector` on `node`, which is not `this`, the node that got
    /// the browse.
    BrowseNode {
        connector: Name,
        node: Name,
        this: Name,
    },
    /// The start `text` of a browse is no path.
    BadStart {
        text: String,
        cause: connector::browse::Malformed,
    },
    /// The match `text` of a browse is no pattern.
    BadPattern {
        text: String,
        cause: connector::browse::Malformed,
    },
    /// An import that picks no point and gives no match.
    NoPick,
    /// An import that picks `picked` points, more than [`LIMIT`].
    PickLimit { picked: usize },
    /// A match of an import of more points than [`LIMIT`].
    MatchLimit,
    /// Each path of a point of an import that is no path, as given, with the cause,
    /// in the order of the picks.
    BadPoint {
        points: Vec<(String, connector::browse::Malformed)>,
    },
    /// Each path that an import picks more than once, in the order of the picks.
    DuplicatePick { paths: Vec<String> },
    /// Each given name of an import that is no name: the path of its point, the name
    /// as given, and the cause, in the order of the picks.
    BadName {
        names: Vec<(String, String, types::name::Error)>,
    },
    /// Each picked point that a connector of the kind cannot read or write, by path,
    /// with the reason.
    UnreadablePoint { points: Vec<(String, String)> },
    /// The walk of the picks of an import stopped at `Stop` before it found each
    /// point.
    PickBudget(Stop),
    /// The walk of a match of an import stopped at `Stop`.
    MatchBudget(Stop),
    /// The reply of an import has more bytes than one reply holds.
    LargeImport,
    /// The config file at `path` changed after an import read it.
    FileChanged { path: PathBuf },
}

/// Why a command line did not read a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unread {
    NotFound,
    Directory,
    /// The paths of the entries of a directory pass the bytes of one read.
    ManyEntries,
    NotUtf8,
    Io {
        code: i32,
    },
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

/// The codes of a body that holds a key it does not take or lacks one it needs.
const POINTED: [&str; 3] = [
    "document.unknown-attribute",
    "document.unknown-block",
    "document.missing-attribute",
];

/// Where the keys and blocks of each body are.
const DOCS: &str =
    "Run `foundation docs` for the attributes and blocks that each block takes.";

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

    /// `Browse` of `connector` of `kind` with each problem of `cause`. A problem of
    /// `Config` keeps its code, fix, place, and notes, and its spans are in the files
    /// of `sources`. `Device` and `Retry` give one problem of `ops.browse`.
    pub(crate) fn browse(
        connector: &Name,
        kind: &Name,
        cause: connector::kind::Error,
        sources: &Sources,
    ) -> Self {
        let diagnostics = match cause {
            connector::kind::Error::Config(diagnostics) => diagnostics,
            cause @ (connector::kind::Error::Device(_)
            | connector::kind::Error::Retry(_)) => vec![Diagnostic::new(
                BROWSE,
                None,
                cause.to_string(),
                "Fix the cause in the message, then browse again".to_owned(),
            )],
        };
        let problems = diagnostics
            .into_iter()
            .map(|diagnostic| {
                let mut problem = Problem::of(diagnostic, sources);
                let message = problem.message;
                problem.message = format!(
                    "the browse of the connector `{connector}` of kind `{kind}` \
                     failed: {message}"
                );
                problem
            })
            .collect();
        Self::Browse {
            connector: connector.clone(),
            kind: kind.clone(),
            problems,
        }
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
            | Self::LargeBrowse
            | Self::LargeStatus
            | Self::ChannelRead { .. }
            | Self::Write { .. }
            | Self::Data { .. }
            | Self::NoData { .. }
            | Self::FolderChanged { .. }
            | Self::Browse { .. }
            | Self::PickBudget(_)
            | Self::MatchBudget(_)
            | Self::LargeImport
            | Self::FileChanged { .. }
            | Self::Reply { .. } => 1,
            Self::Argument { .. }
            | Self::Unknown { .. }
            | Self::Config(_)
            | Self::Plan(_)
            | Self::Read { .. }
            | Self::NoConfig { .. }
            | Self::LargeRequest
            | Self::Outside { .. }
            | Self::Hidden { .. }
            | Self::PlanName { .. }
            | Self::WideFolder { .. }
            | Self::BrowseNode { .. }
            | Self::BadStart { .. }
            | Self::BadPattern { .. }
            | Self::NoPick
            | Self::PickLimit { .. }
            | Self::MatchLimit
            | Self::BadPoint { .. }
            | Self::DuplicatePick { .. }
            | Self::BadName { .. }
            | Self::UnreadablePoint { .. }
            | Self::Folder { .. } => 2,
        }
    }

    /// Each problem: for each error but `Config`, `Remote`, `Browse`, `BadPoint`,
    /// `DuplicatePick`, `BadName`, and `UnreadablePoint`, one with no place and no
    /// note.
    #[expect(clippy::too_many_lines, reason = "one arm for each error")]
    pub(crate) fn problems(&self) -> Cow<'_, [Problem]> {
        let (code, fix) = match self {
            Self::Config(problems)
            | Self::Remote { problems, .. }
            | Self::Browse { problems, .. } => {
                return Cow::Borrowed(problems);
            }
            Self::BadPoint { points } => {
                let problems = points.iter().map(|(text, cause)| {
                    let message = format!("the point `{text}` is not a path: {cause}");
                    (BAD_POINT, message, BAD_POINT_FIX)
                });
                return Cow::Owned(problems.map(Problem::plain).collect());
            }
            Self::BadName { names } => {
                let problems = names.iter().map(|(path, text, cause)| {
                    let message = format!(
                        "the name `{text}` of the point `{path}` is not a name: {cause}"
                    );
                    (BAD_NAME, message, BAD_NAME_FIX)
                });
                return Cow::Owned(problems.map(Problem::plain).collect());
            }
            Self::DuplicatePick { paths } => {
                let problems = paths.iter().map(|path| {
                    let message = format!("the import picks the point `{path}` twice");
                    (DUPLICATE_PICK, message, "Pick each point once")
                });
                return Cow::Owned(problems.map(Problem::plain).collect());
            }
            Self::UnreadablePoint { points } => {
                let problems = points.iter().map(|(path, reason)| {
                    let message = format!(
                        "a connector of the kind cannot read or write the point \
                         `{path}`: {reason}"
                    );
                    (UNREADABLE_POINT, message, "Leave the point out of the import")
                });
                return Cow::Owned(problems.map(Problem::plain).collect());
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
            Self::LargeBrowse => (
                LARGE_BROWSE,
                "Narrow the start or the match, or browse without `values`".to_owned(),
            ),
            Self::LargeStatus => (
                LARGE_STATUS,
                "Read the status channels of each connector with a client of the node"
                    .to_owned(),
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
            Self::NoConfig { path, extensions } => (
                NO_CONFIG,
                format!(
                    "Add a file whose name ends in {} and does not start with `.` \
                     to {}, or give another path",
                    extensions,
                    path.display()
                ),
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
                    "Give a path under one of those folders. A relative path starts at \
                     the working directory of this MCP server, which was {} at its \
                     start. To reach another folder, add \
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
            Self::BrowseNode { node, .. } => {
                (BROWSE_NODE, format!("Run it with `--data` of node `{node}`"))
            }
            Self::BadStart { .. } => (
                BAD_START,
                "Give folder names joined by `/`, with a `\\` before each `/`, `*`, or \
                 `\\` in a name"
                    .to_owned(),
            ),
            Self::BadPattern { .. } => (
                BAD_PATTERN,
                format!(
                    "Give names joined by `/`, at most {DEPTH} other than `**` and at \
                     most {STARS} `*`, with a `\\` before each `/`, `*`, or `\\` in a \
                     name"
                ),
            ),
            Self::NoPick => (
                NO_PICK,
                "Give the points to import, or `--match <pattern>`".to_owned(),
            ),
            Self::PickLimit { .. } => (
                PICK_LIMIT,
                format!("Pick at most {LIMIT} points, and import the others in another run"),
            ),
            Self::MatchLimit => (MATCH_LIMIT, "Narrow the start or the match".to_owned()),
            Self::PickBudget(_) => (
                PICK_BUDGET,
                "Pick the points of fewer folders, and import the others in another run"
                    .to_owned(),
            ),
            Self::MatchBudget(_) => (MATCH_BUDGET, "Narrow the start or the match".to_owned()),
            Self::LargeImport => (
                LARGE_IMPORT,
                "Import fewer points at a time, or move other blocks out of the file \
                 of the connector block"
                    .to_owned(),
            ),
            Self::FileChanged { .. } => (
                FILE_CHANGED,
                "Run the import again on the file as it is now".to_owned(),
            ),
        };
        Cow::Owned(vec![Problem::plain((code, self.to_string(), &fix))])
    }

    /// `{"errors": [...]}`, with each problem.
    pub(crate) fn json(&self) -> Value {
        json!({ "errors": self.problems() })
    }

    /// Each problem for a terminal, with one empty line between two, then [`DOCS`] when
    /// a problem has a code of [`POINTED`]. In `Config`, `Remote`, and `Browse`, each
    /// control character is written as its Rust escape, because a config producer, the
    /// node, or a connector kind quotes the text of a file or a device itself. In each
    /// other error, so are a backslash and each character that does not print, because
    /// that text holds the caller's text as given.
    pub(crate) fn text(&self) -> String {
        let escape: fn(&str) -> String = match self {
            Self::Config(_) | Self::Remote { .. } | Self::Browse { .. } => {
                escape_controls
            }
            _ => escape,
        };
        let problems = self.problems();
        let texts: Vec<String> = problems
            .iter()
            .map(|problem| problem.text(escape))
            .collect();
        let text = texts.join("\n");
        if problems.iter().any(|p| POINTED.contains(&p.code.as_str())) {
            format!("{text}\n{DOCS}\n")
        } else {
            text
        }
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
    /// The problem of `code` with `message` and `fix`, with no place and no note.
    fn plain((code, message, fix): (Code, String, &str)) -> Self {
        Self {
            code: code.as_str().to_owned(),
            message,
            fix: fix.to_owned(),
            place: None,
            notes: Vec::new(),
        }
    }

    /// `ops.more-problems`: `count` more problems do not fit in one reply.
    /// `serve::fitting` takes its encoded length to change only with the digits of
    /// `count`.
    pub(crate) fn more(count: usize) -> Self {
        Self {
            code: MORE_PROBLEMS.as_str().to_owned(),
            message: format!("{count} more problems do not fit in one reply"),
            fix: "Fix the problems above, then run it again".to_owned(),
            place: None,
            notes: Vec::new(),
        }
    }

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
            Self::LargeBrowse => write!(
                f,
                "the reply of the browse has more than {BODY_BYTES_MAX} bytes, the \
                 most that a reply holds"
            ),
            Self::LargeStatus => write!(
                f,
                "the reply of the status has more than {BODY_BYTES_MAX} bytes, the \
                 most that a reply holds"
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
                    Unread::ManyEntries => {
                        write!(f, "{path} holds more entries than one read lists")
                    }
                    Unread::NotUtf8 => write!(f, "the text of {path} is not UTF-8"),
                    Unread::Io { code } => {
                        write!(f, "the read of {path} failed with OS error {code}")
                    }
                }
            }
            Self::NoConfig { path, .. } => {
                write!(f, "{} holds no config file", path.display())
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
                 moved or replaced after it started",
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
            Self::Browse {
                connector,
                kind,
                problems,
            } => match problems.as_slice() {
                [one] => f.write_str(&one.message),
                _ => write!(
                    f,
                    "the browse of the connector `{connector}` of kind `{kind}` failed \
                     with {} problems",
                    problems.len()
                ),
            },
            Self::BrowseNode {
                connector,
                node,
                this,
            } => write!(
                f,
                "the connector `{connector}` runs on the node `{node}`, not on this \
                 node `{this}`"
            ),
            Self::BadStart { text, cause } => {
                write!(f, "the start `{text}` is not a path: {cause}")
            }
            Self::BadPattern { text, cause } => {
                write!(f, "the match `{text}` is not a pattern: {cause}")
            }
            Self::NoPick => f.write_str("the import picks no point and gives no match"),
            Self::PickLimit { picked } => write!(
                f,
                "the import picks {picked} points, more than {LIMIT}, the most for one \
                 import"
            ),
            Self::MatchLimit => {
                write!(f, "more than {LIMIT} points match, the most for one import")
            }
            Self::MatchBudget(stop) => {
                write!(f, "the import stopped {stop}, so more points can match")
            }
            Self::PickBudget(stop) => {
                write!(f, "the import stopped {stop} before it found each point")
            }
            Self::LargeImport => write!(
                f,
                "the reply of the import has more than {BODY_BYTES_MAX} bytes, the \
                 most that a reply holds"
            ),
            Self::FileChanged { path } => write!(
                f,
                "the file {} changed after the import read it, so the import wrote \
                 nothing",
                path.display()
            ),
            Self::BadPoint { .. }
            | Self::DuplicatePick { .. }
            | Self::BadName { .. }
            | Self::UnreadablePoint { .. } => match &*self.problems() {
                [one] => f.write_str(&one.message),
                problems => write!(f, "the import has {} problems", problems.len()),
            },
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
