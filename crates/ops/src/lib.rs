//! Holds the operation table and handlers, generates the CLI, MCP tools, and docs, and
//! runs each operation on the node that must run it.

use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::path::Path;

use env::paths::Paths;

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
#[cfg(test)]
mod tests;
mod used;

use error::Error;
pub use front_end::{FrontEnd, FrontEnds};
pub use node::Node;
use operation::Parsed;
pub use serve::serve;
pub use start::{Failure, Start};

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
/// `plan` and `apply` run on the node of `foundation-data`. Each reads the files it
/// names through `paths`, calls `reach` with that data directory once, and calls
/// what it gives with the body of one request. A [`Failure`] of either is the error
/// of the command, with status 1. `start` and the other operations call neither.
///
/// Blocks its thread at each read or write of a path, `input`, or `output`, and makes
/// none while a request to the node waits.
pub async fn cli<C>(
    args: impl IntoIterator<Item = OsString>,
    input: impl BufRead,
    mut output: impl Write,
    errors: impl Write,
    paths: &Paths,
    reach: impl AsyncFnMut(&Path) -> Result<C, Failure>,
) -> Run
where
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    // Read before parsing, so an argument error also honors `--json`.
    let json = args
        .iter()
        .take_while(|arg| *arg != "--")
        .map(|arg| arg.as_encoded_bytes())
        .any(|arg| arg == b"--json" || arg.starts_with(b"--json="));
    let render = |value: Value, text: String| {
        if json { format!("{value}\n") } else { text }
    };
    let done = match operation::parse(&args) {
        Ok(Parsed::Start(start)) => return Run::Start(start),
        Ok(Parsed::Run(request)) => {
            let response = request.run();
            write(&mut output, &render(response.json(), response.text()))
        }
        Ok(Parsed::Help(text)) => {
            write(&mut output, &render(json!({ "help": text }), text.clone()))
        }
        Ok(Parsed::Plan { files, out }) => {
            match remote::plan(&files, out.as_deref(), paths, reach).await {
                Ok(shown) => write(&mut output, &render(shown.json(), shown.text())),
                Err(error) => Err(Stop::Failed(error)),
            }
        }
        Ok(Parsed::Apply { plan }) => match remote::apply(&plan, paths, reach).await {
            Ok(shown) => write(&mut output, &render(shown.json(), shown.text())),
            Err(error) => Err(Stop::Failed(error)),
        },
        Ok(Parsed::Mcp) => mcp::serve(input, &mut output),
        Err(error) => Err(Stop::Failed(error)),
    };
    match done {
        Ok(()) | Err(Stop::Closed) => Run::Exit(0),
        Err(Stop::Failed(error)) => Run::Exit(report(&error, json, errors)),
    }
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
