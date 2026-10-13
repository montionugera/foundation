//! Holds the operation table and handlers, generates the CLI, MCP tools, and docs, and
//! runs each operation on the node that must run it.

use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::path::Path;

use env::paths::Paths;

use serde::Serialize;
use serde_json::{Value, json};

mod apply;
#[cfg(test)]
mod common;
mod error;
mod front_end;
mod mcp;
mod node;
mod operation;
mod plan;
mod remote;
mod serve;
mod start;
mod status;
#[cfg(test)]
mod tests;
mod used;

use error::Error;
pub use error::Failure;
pub use front_end::{FrontEnd, FrontEnds};
pub use node::Node;
use operation::Parsed;
pub use serve::serve;
pub use start::Start;

/// What the process does once [`cli`] returns.
#[derive(Debug, PartialEq, Eq)]
pub enum Run {
    /// Exit with this status.
    Exit(u8),
    /// Start a node with these arguments. `cli` wrote nothing for it.
    Start(Start),
}

/// Runs one command line, such as `["foundation", "version", "--json"]`, and returns
/// the exit status: 0 on success, 1 when a stream fails, and 2 for a bad argument or
/// an unknown operation. For `foundation start`, it gives the arguments to start a
/// node with.
///
/// An operation or help writes its output to `output`, and an error goes to `errors`
/// with its code and fix. With `--json`, each is one line of JSON, and help is
/// `{"help": "<text>"}`. `foundation mcp` answers each line of `input` with
/// at most one line on `output`, flushed, until `input` ends. A line longer than 1 MiB
/// gets an Invalid Request error. A closed `output` ends the run with no error,
/// because its reader has left.
///
/// `plan`, `apply`, and `status` run on the node of `--data`, else of
/// `foundation-data`. Each reads the files it names through `paths`, calls `reach` with
/// that data directory once, and calls what it gives with the body of one request. A
/// [`Failure`] of either is the error of the command, with status 1. `start` and the
/// other operations call neither.
///
/// Blocks its thread at each read or write of a path, each read of `input`, and each
/// write of `output` or `errors`, and makes none while a request to the node waits.
pub async fn cli<C>(
    args: impl IntoIterator<Item = OsString>,
    input: impl BufRead,
    mut output: impl Write,
    errors: impl Write,
    paths: &Paths,
    mut reach: impl AsyncFnMut(&Path) -> Result<C, Failure>,
) -> Run
where
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    // Read before parsing, so an argument error also honors `--json`.
    let json = json(&args);
    let done = match operation::parse(&args, json) {
        Ok(Parsed::Start(start)) => return Run::Start(start),
        Ok(Parsed::Run(request)) => {
            let response = request.run();
            write(
                &mut output,
                &render(json, &response.json(), || response.text()),
            )
        }
        Ok(Parsed::Help(text)) => write(
            &mut output,
            &render(json, &json!({ "help": text }), || text.clone()),
        ),
        Ok(Parsed::Plan { files, out, data }) => {
            match remote::plan(&files, out.as_deref(), paths, async || {
                reach(&data).await
            })
            .await
            {
                Ok(planned) => {
                    write(&mut output, &render(json, &planned, || planned.text()))
                }
                Err(error) => Err(Stop::Failed(error)),
            }
        }
        Ok(Parsed::Apply { plan, data }) => {
            match remote::apply(&plan, paths, async || reach(&data).await).await {
                Ok(applied) => {
                    write(&mut output, &render(json, &applied, || applied.text()))
                }
                Err(error) => Err(Stop::Failed(error)),
            }
        }
        Ok(Parsed::Status { data }) => {
            match remote::status(async || reach(&data).await).await {
                Ok(shown) => write(&mut output, &render(json, &shown, || shown.text())),
                Err(error) => Err(Stop::Failed(error)),
            }
        }
        Ok(Parsed::Mcp) => mcp::serve(input, &mut output),
        Err(error) => Err(Stop::Failed(error)),
    };
    match done {
        Ok(()) | Err(Stop::Closed) => Run::Exit(0),
        Err(Stop::Failed(error)) => Run::Exit(report(&error, json, errors)),
    }
}

/// Writes `failure`, why the command line `args` did not run or why its node failed,
/// to `errors` as [`cli`] writes its own errors: as JSON when `args` has `--json`
/// after the program name and before any `--`, else as text. Gives exit status 1.
pub fn fail(args: &[OsString], failure: &Failure, errors: impl Write) -> u8 {
    report(&Error::Node(failure.clone()), json(args), errors)
}

/// Whether `args` asks for JSON: a `--json` flag after the program name and before any
/// `--`.
fn json(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .map(|arg| arg.as_encoded_bytes())
        .any(|arg| arg == b"--json" || arg.starts_with(b"--json="))
}

/// Writes `error` to `errors`, as JSON when `json`, and gives its exit status.
fn report(error: &Error, json: bool, mut errors: impl Write) -> u8 {
    let text = if json {
        format!("{}\n", error.json())
    } else {
        error.text()
    };
    match errors.write_all(text.as_bytes()) {
        // With `errors` closed too, the status is the only report left.
        Ok(()) | Err(_) => error.status(),
    }
}

/// Why a run stopped before its end.
enum Stop {
    /// The reader closed `output`, so the run ends with nothing to report.
    Closed,
    Failed(Error),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

/// `output` as JSON.
pub(crate) fn value(output: &impl Serialize) -> Value {
    serde_json::to_value(output).expect("invariant: an output is plain JSON data")
}

/// `output` as one line of JSON when `json`, else `text()`.
fn render(
    json: bool,
    output: &impl Serialize,
    text: impl FnOnce() -> String,
) -> String {
    if json {
        format!("{}\n", value(output))
    } else {
        text()
    }
}

/// Writes `text` to `output` and flushes it.
fn write(output: &mut impl Write, text: &str) -> Result<(), Stop> {
    output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
        .map_err(|e| match e.kind() {
            io::ErrorKind::BrokenPipe => Stop::Closed,
            _ => Stop::Failed(Error::Output {
                message: e.to_string(),
            }),
        })
}
