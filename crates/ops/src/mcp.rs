use std::io::{self, BufRead, Read, Write};

use env::paths::Paths;
use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::operation::{self, TABLE};
use crate::remote::Link;
use crate::{Failure, Stop};

/// The one protocol version this server speaks. A client that asks for another gets
/// this one, and may then disconnect.
const VERSION: &str = "2025-06-18";

const PARSE: (i64, &str) = (-32700, "Parse error");
const REQUEST: (i64, &str) = (-32600, "Invalid Request");
const METHOD: (i64, &str) = (-32601, "Method not found");
const PARAMS: (i64, &str) = (-32602, "Invalid params");

/// The longest line, with its newline, that `serve` reads into memory.
pub(crate) const LIMIT: usize = 1 << 20;

/// Answers each line of `input` with at most one line on `output`, until `input` ends
/// or the reader closes `output`. A tool call reads and writes its files through
/// `paths`, and sends its request to the node through `link`.
pub(crate) async fn serve<R, C>(
    mut input: impl BufRead,
    output: &mut impl Write,
    paths: &Paths,
    link: &mut Link<R, C>,
) -> Result<(), Stop>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let failed = |e: io::Error| Error::Input {
        message: e.to_string(),
    };
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut input)
            .take(LIMIT as u64)
            .read_until(b'\n', &mut line)
            .map_err(failed)?;
        let reply = if read == 0 {
            return Ok(());
        } else if read == LIMIT && !line.ends_with(b"\n") {
            input.skip_until(b'\n').map_err(failed)?;
            Some(reply(&Value::Null, Err(fault(REQUEST))).to_string())
        } else {
            // A line that is not UTF-8 is not JSON either, so "" gets the parse error.
            respond(std::str::from_utf8(&line).unwrap_or(""), paths, link).await
        };
        if let Some(reply) = reply {
            crate::write(output, &format!("{reply}\n"))?;
        }
    }
}

/// Answers one MCP message, a JSON-RPC 2.0 request or notification. Returns the reply
/// as one line of JSON, or `None` for a notification or a response.
///
/// A request id must be a string or a 64-bit integer, so the reply carries it
/// unchanged. An unknown tool and a bad argument give a JSON-RPC error whose `data` is
/// `{"errors": [...]}`, each item a problem with `code`, `message`, `fix`, `place` when
/// the problem is in a file, and `notes`. A failed operation gives a result with
/// `isError` true and that JSON as its one text.
pub(crate) async fn respond<R, C>(
    message: &str,
    paths: &Paths,
    link: &mut Link<R, C>,
) -> Option<String>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let reply = match serde_json::from_str::<Value>(message) {
        Ok(Value::Object(message)) => answer(message, paths, link).await?,
        Ok(_) => reply(&Value::Null, Err(fault(REQUEST))),
        Err(_) => reply(&Value::Null, Err(fault(PARSE))),
    };
    Some(reply.to_string())
}

async fn answer<R, C>(
    mut message: Map<String, Value>,
    paths: &Paths,
    link: &mut Link<R, C>,
) -> Option<Value>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    if !message.contains_key("method")
        && (message.contains_key("result") || message.contains_key("error"))
    {
        return None;
    }
    let id = match message.remove("id") {
        None => None,
        Some(id) if id.is_string() || id.is_i64() || id.is_u64() => Some(id),
        Some(_) => return Some(reply(&Value::Null, Err(fault(REQUEST)))),
    };
    let method = match message.remove("method") {
        Some(Value::String(method))
            if message.get("jsonrpc") == Some(&json!("2.0")) =>
        {
            method
        }
        _ => return Some(reply(&id.unwrap_or(Value::Null), Err(fault(REQUEST)))),
    };
    // Checked after the request itself, so an invalid notification gets a reply.
    let id = id?;
    let Some(method) = Method::of(&method) else {
        return Some(reply(&id, Err(fault(METHOD))));
    };
    let params = match message.remove("params") {
        None => Map::new(),
        Some(Value::Object(params)) => params,
        Some(_) => return Some(reply(&id, Err(fault(PARAMS)))),
    };
    let result = match method {
        Method::Initialize => Ok(initialize()),
        Method::Ping => Ok(json!({})),
        Method::List => Ok(tools()),
        Method::Call => call(params, paths, link).await,
    };
    Some(reply(&id, result))
}

/// The methods that the server answers.
enum Method {
    Initialize,
    Ping,
    List,
    Call,
}

impl Method {
    fn of(name: &str) -> Option<Self> {
        match name {
            "initialize" => Some(Self::Initialize),
            "ping" => Some(Self::Ping),
            "tools/list" => Some(Self::List),
            "tools/call" => Some(Self::Call),
            _ => None,
        }
    }
}

fn initialize() -> Value {
    json!({
        "protocolVersion": VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "foundation", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tools() -> Value {
    let inputs = operation::inputs();
    let outputs = operation::outputs();
    let tools: Vec<Value> = TABLE
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.summary,
                "inputSchema": inputs[spec.name],
                "outputSchema": outputs[spec.name],
                "annotations": {
                    "readOnlyHint": spec.read_only,
                    "destructiveHint": spec.destructive,
                },
            })
        })
        .collect();
    json!({ "tools": tools })
}

async fn call<R, C>(
    mut params: Map<String, Value>,
    paths: &Paths,
    link: &mut Link<R, C>,
) -> Result<Value, Value>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let Some(Value::String(name)) = params.remove("name") else {
        return Err(fault(PARAMS));
    };
    let arguments = match params.remove("arguments") {
        None => Map::new(),
        Some(Value::Object(arguments)) => arguments,
        Some(_) => return Err(fault(PARAMS)),
    };
    match operation::read(&name, arguments) {
        Ok(request) => Ok(match request.run(paths, link).await {
            Ok(response) => {
                let content = response.json();
                json!({
                    "content": [{ "type": "text", "text": content.to_string() }],
                    "structuredContent": content,
                })
            }
            Err(error) => json!({
                "content": [{ "type": "text", "text": error.json().to_string() }],
                "isError": true,
            }),
        }),
        Err(error) => Err(json!({
            "code": PARAMS.0,
            "message": error.to_string(),
            "data": error.json(),
        })),
    }
}

fn fault((code, message): (i64, &str)) -> Value {
    json!({ "code": code, "message": message })
}

fn reply(id: &Value, result: Result<Value, Value>) -> Value {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    }
}
