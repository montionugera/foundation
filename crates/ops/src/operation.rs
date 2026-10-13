use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use types::name::Name;

use crate::Start;
use crate::error::{Among, Error};

/// The data directory of a command that names none.
pub(crate) const DATA: &str = "foundation-data";

/// The facts that the CLI, the MCP tools, and the docs show for one operation.
pub(crate) struct Spec {
    pub(crate) name: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) read_only: bool,
    pub(crate) destructive: bool,
}

/// Every operation, in the order the help and the docs list them. A test holds the
/// names equal to the variants of `Request` and `Response`.
pub(crate) const TABLE: &[Spec] = &[
    Spec {
        name: "version",
        summary: "Print the version of Foundation",
        read_only: true,
        destructive: false,
    },
    Spec {
        name: "docs",
        summary: "Print the reference for every operation, as Markdown",
        read_only: true,
        destructive: false,
    },
];

#[derive(Parser)]
#[command(
    name = "foundation",
    about = "Run Foundation operations",
    disable_help_subcommand = true
)]
struct Cli {
    /// Print the output or the error as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Run(Request),
    /// Start a node on a data directory, and run it until Ctrl-C.
    Start(StartArgs),
    /// Show the change from config files to the spec of the node of `--data`.
    Plan(PlanArgs),
    /// Apply a plan file of `foundation plan --out` on the node of `--data`.
    Apply(ApplyArgs),
    /// Show the state of each connector on the node of `--data`.
    Status(NodeArgs),
    /// Answer MCP messages on standard input, one JSON-RPC message per line, until it
    /// closes.
    Mcp,
    /// Print this help, or the help of one command.
    Help { command: Option<String> },
}

#[derive(clap::Args)]
struct StartArgs {
    /// The data directory of the node. The start makes it when it is not there, but
    /// not its parents.
    #[arg(long, default_value = DATA)]
    data: PathBuf,
    /// The name of the node. The first start on a data directory needs it, and a later
    /// start reads it from there.
    #[arg(long)]
    name: Option<Name>,
    /// The socket address of the node's one port. Port 0 takes a free port. The default
    /// takes no session from another host.
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: SocketAddr,
}

#[derive(clap::Args)]
struct PlanArgs {
    /// The config files.
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Write the plan file here, for `foundation apply`. With none, `plan` writes no
    /// file.
    #[arg(long)]
    out: Option<PathBuf>,
    #[command(flatten)]
    node: NodeArgs,
}

#[derive(clap::Args)]
struct ApplyArgs {
    /// The plan file.
    plan: PathBuf,
    #[command(flatten)]
    node: NodeArgs,
}

/// The arguments of a command that runs on a node.
#[derive(clap::Args)]
struct NodeArgs {
    /// The data directory of the node.
    #[arg(long, default_value = DATA)]
    data: PathBuf,
}

/// The input of each operation. Clap and serde both name a variant in kebab case.
#[derive(Subcommand, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Request {
    Version(Empty),
    Docs(Empty),
}

#[derive(clap::Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}

/// The output of each operation. Serde tags it with the operation's name; `json` and
/// `text` leave the tag out.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Response {
    Version(Version),
    Docs(Reference),
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub(crate) struct Version {
    version: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub(crate) struct Reference {
    markdown: String,
}

pub(crate) enum Parsed {
    Run(Request),
    Start(Start),
    Plan {
        files: Vec<PathBuf>,
        out: Option<PathBuf>,
        data: PathBuf,
    },
    Apply {
        plan: PathBuf,
        data: PathBuf,
    },
    Status {
        data: PathBuf,
    },
    Help(String),
    Mcp,
}

/// The command tree, with each operation's summary from `TABLE`.
pub(crate) fn command() -> clap::Command {
    let root = Cli::command().arg_required_else_help(false);
    TABLE.iter().fold(root, |command, spec| {
        command.mut_subcommand(spec.name, |sub| sub.about(spec.summary))
    })
}

/// Parses `args`. `json` is whether they ask for JSON, as `cli` read it.
pub(crate) fn parse(args: &[OsString], json: bool) -> Result<Parsed, Error> {
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => {
            return Ok(Parsed::Help(e.to_string()));
        }
        Err(e) => return Err(from_clap(&e)),
    };
    let cli = Cli::from_arg_matches(&matches).map_err(|e| from_clap(&e))?;
    match cli.command {
        Command::Run(request) => Ok(Parsed::Run(request)),
        Command::Start(StartArgs { data, name, listen }) => Ok(Parsed::Start(Start {
            data,
            json,
            name,
            listen,
        })),
        Command::Plan(PlanArgs { files, out, node }) => Ok(Parsed::Plan {
            files,
            out,
            data: node.data,
        }),
        Command::Apply(ApplyArgs { plan, node }) => Ok(Parsed::Apply {
            plan,
            data: node.data,
        }),
        Command::Status(NodeArgs { data }) => Ok(Parsed::Status { data }),
        Command::Mcp => Ok(Parsed::Mcp),
        Command::Help { command } => help(command.as_deref()),
    }
}

/// The help that `foundation [name] --help` prints.
fn help(name: Option<&str>) -> Result<Parsed, Error> {
    let mut root = command();
    root.build();
    let command = match name {
        None => &mut root,
        Some(name) => root
            .find_subcommand_mut(name)
            .ok_or_else(|| unknown(name, Among::Commands))?,
    };
    Ok(Parsed::Help(command.render_help().to_string()))
}

/// Reads the request of a `tools/call` of MCP for the tool `name` from its `arguments`.
pub(crate) fn read(
    name: &str,
    arguments: Map<String, Value>,
) -> Result<Request, Error> {
    if !TABLE.iter().any(|spec| spec.name == name) {
        return Err(unknown(name, Among::Tools));
    }
    let tagged = Map::from_iter([(name.to_owned(), Value::Object(arguments))]);
    serde_json::from_value(Value::Object(tagged)).map_err(|e| Error::Argument {
        message: e.to_string(),
    })
}

/// The unknown-operation error for `name`, with the closest operation name.
fn unknown(name: &str, among: Among) -> Error {
    let operations = Request::augment_subcommands(clap::Command::new("foundation"))
        .disable_help_subcommand(true)
        .disable_help_flag(true);
    let closest = operations
        .try_get_matches_from(["foundation", name])
        .err()
        .and_then(|e| context(&e, ContextKind::SuggestedSubcommand));
    Error::Unknown {
        name: name.to_owned(),
        closest,
        among,
    }
}

fn context(e: &clap::Error, kind: ContextKind) -> Option<String> {
    match e.get(kind) {
        Some(ContextValue::String(text)) => Some(text.clone()),
        Some(ContextValue::Strings(texts)) => texts.first().cloned(),
        _ => None,
    }
}

fn from_clap(e: &clap::Error) -> Error {
    if e.kind() == ErrorKind::InvalidSubcommand {
        return Error::Unknown {
            name: context(e, ContextKind::InvalidSubcommand)
                .expect("invariant: clap names an unknown subcommand"),
            closest: context(e, ContextKind::SuggestedSubcommand),
            among: Among::Commands,
        };
    }
    let what = e
        .kind()
        .as_str()
        .expect("invariant: clap describes each error kind that is not help");
    let mut message = match context(e, ContextKind::InvalidArg) {
        Some(arg) => format!("{what}: `{arg}`"),
        None => what.to_owned(),
    };
    // The error of a value's parser, such as why a name is not one.
    if let Some(source) = std::error::Error::source(e) {
        message = format!("{message}: {source}");
    }
    Error::Argument { message }
}

/// The input schema of each operation, by name.
pub(crate) fn inputs() -> Map<String, Value> {
    variants::<Request>()
}

/// The output schema of each operation, by name.
pub(crate) fn outputs() -> Map<String, Value> {
    variants::<Response>()
}

/// The schema of each variant of an externally tagged enum, by its tag.
fn variants<T: JsonSchema>() -> Map<String, Value> {
    let mut settings = SchemaSettings::draft2020_12();
    settings.inline_subschemas = true;
    let root = settings
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value();
    let variants = root["oneOf"]
        .as_array()
        .expect("invariant: an enum of variants");
    variants
        .iter()
        .flat_map(|variant| {
            variant["properties"]
                .as_object()
                .expect("invariant: each variant is tagged by its name")
                .clone()
        })
        .collect()
}

/// The reference for every operation, as Markdown.
pub(crate) fn docs() -> String {
    let yes = |flag: bool| if flag { "yes" } else { "no" };
    let sections: Vec<String> = TABLE
        .iter()
        .map(|spec| {
            format!(
                "\n## `{}`\n\n{}\n\n- Read-only: {}\n- Destructive: {}\n",
                spec.name,
                spec.summary,
                yes(spec.read_only),
                yes(spec.destructive),
            )
        })
        .collect();
    format!("# Operations\n{}", sections.concat())
}

impl Request {
    pub(crate) fn run(self) -> Response {
        match self {
            Self::Version(Empty {}) => Response::Version(Version {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }),
            Self::Docs(Empty {}) => Response::Docs(Reference { markdown: docs() }),
        }
    }
}

impl Response {
    /// The output as JSON, without the operation's name.
    pub(crate) fn json(&self) -> Value {
        match self {
            Self::Version(output) => crate::value(output),
            Self::Docs(output) => crate::value(output),
        }
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Self::Version(output) => format!("{}\n", output.version),
            Self::Docs(output) => output.markdown.clone(),
        }
    }
}
