//! The requests and replies of a remote program, and how the node runs each request.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::path::PathBuf;

use access::Rules;
use connector::browse;
use schemars::JsonSchema;
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, DeserializeOwned, Deserializer, IgnoredAny, MapAccess};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spec::definition::{Definition, Kind};
use types::name::{Name, Prefix};
use wire::hub::client::BODY_BYTES_MAX;

use crate::error::{Action, Error, On, Problem, Stop};
use crate::front_end::{self, File};
use crate::operation::Empty;
use crate::plan::{Documents, Output};
use crate::{Node, apply, status, used};

#[cfg(test)]
mod tests;

/// Runs the request `body` of `subject` on `node`, and gives the body of the reply:
/// the output of the operation, or its error with code, message, and fix. `body` is
/// what [`cli`] sends. Call it only for a subject whose key the hub verified.
///
/// The reply is at most `wire::hub::client::BODY_BYTES_MAX` bytes, and `serve` stops
/// writing a reply at that bound. No `body` makes it panic. The reply holds
/// `ops.bad-request` for a body that is no request of `plan`, `apply`, `status`,
/// `browse`, or `import`, `ops.denied` for an action that no access policy gives
/// `subject` and for a request that names no definition. An error reply over the bound
/// keeps the problems that fit, then `ops.more-problems` for the rest.
/// `ops.large-reply` takes the place of an output over the bound and of an error whose
/// first problem does not fit, `ops.large-browse` takes it for `browse`,
/// `ops.large-status` for `status`, and `ops.large-import` for `import`. `apply` also
/// needs `apply` on the channel at each edge of each channel that the plan adds or
/// changes, and `admin` on the key of each definition other than a channel that the
/// plan adds, changes, or removes. It gives a home only to an index on which `subject`
/// has `apply`, and drops each other home with no error. `status` shows only each
/// connector on whose label and status channels `subject` has `read`. When it shows
/// none, also when the node places none, it needs `read` on the name of the node, and
/// gives `ops.denied` with no grant. It gives the failures of the read of the OS roots
/// of the node only to a subject with `read` on the name of the node. It waits at most
/// 1 s from the request. A connector is `unknown` when no status as the spec defines it
/// comes in that time, and `status` gives `ops.channel-read` when the reader of a
/// connector fails for another cause. `browse` needs `apply` and `admin` on the name of
/// the connector, and browses only a connector that the files place on this node.
/// `import` needs what `browse` needs, and imports only a connector that the files
/// place on this node.
///
/// [`cli`]: crate::cli
pub async fn serve(node: &Node, subject: &Name, body: &[u8]) -> Vec<u8> {
    let reply = match read(body) {
        // The cause is cut, so no reply of a body that is no request reaches
        // `LargeReply`.
        Err(error) => failed(&error, &Error::LargeReply),
        Ok((request, rest)) => {
            let large = request.large();
            match run(node, subject, request, rest).await {
                Ok(ran) => ran.body().unwrap_or_else(|Large| failed(&large, &large)),
                Err(error) => failed(&error, &large),
            }
        }
    };
    reply.0
}

/// A body that would pass the bound.
#[derive(Debug)]
struct Large;

/// The output of an operation that ran.
enum Ran {
    /// The output of `plan`, then its plan file.
    Plan(Output, config::plan::Plan),
    Apply(apply::Effect),
    Status(status::Output),
    Browse(crate::browse::Output),
    /// The output of `import`, then the new text of its file.
    Import(crate::import::Output, String),
}

impl Ran {
    /// The reply that holds this output. Stops at the bound.
    ///
    /// # Errors
    ///
    /// `Large` when the reply passes the bound.
    fn body(&self) -> Result<Body, Large> {
        match self {
            Self::Plan(output, plan) => {
                let mut reply = Body::done(output)?;
                reply.add(&plan.encode())?;
                Ok(reply)
            }
            Self::Apply(effect) => Body::done(effect),
            Self::Status(output) => Body::done(output),
            Self::Browse(output) => Body::done(output),
            Self::Import(output, text) => {
                let mut reply = Body::done(output)?;
                reply.add(text.as_bytes())?;
                Ok(reply)
            }
        }
    }
}

/// The bytes of a request or a reply: at most `BODY_BYTES_MAX`.
#[derive(Default)]
struct Body(Vec<u8>);

impl Body {
    /// The body of `head` as one line of JSON. Stops at the bound. `fitting` counts
    /// the layout of this line.
    ///
    /// # Errors
    ///
    /// `Large` when the line passes the bound.
    fn line(head: &impl Serialize) -> Result<Self, Large> {
        let mut body = Self::default();
        match serde_json::to_writer(&mut body, head) {
            Ok(()) => {}
            Err(error) if error.is_io() => return Err(Large),
            Err(error) => panic!("invariant: a body is plain JSON data: {error}"),
        }
        body.add(b"\n")?;
        Ok(body)
    }

    /// The body of a reply that holds `output`. Stops at the bound.
    ///
    /// # Errors
    ///
    /// `Large` when the line passes the bound.
    fn done(output: &impl Serialize) -> Result<Self, Large> {
        Self::line(&Answer::<_, ()>::Done { output })
    }

    /// The body of a reply that holds `errors` with `status`. Stops at the bound.
    ///
    /// # Errors
    ///
    /// `Large` when the line passes the bound.
    fn failed(errors: impl Serialize, status: u8) -> Result<Self, Large> {
        Self::line(&Answer::<(), _>::Failed { errors, status })
    }

    /// Adds `bytes` to the end.
    ///
    /// # Errors
    ///
    /// `Large`, with no byte added, when the body would pass the bound.
    fn add(&mut self, bytes: &[u8]) -> Result<(), Large> {
        if fits(self.0.len().saturating_add(bytes.len())) {
            self.0.extend_from_slice(bytes);
            Ok(())
        } else {
            Err(Large)
        }
    }
}

impl Write for Body {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.add(bytes)
            .map(|()| bytes.len())
            .map_err(|Large| io::ErrorKind::FileTooLarge.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The first line of a request: one operation and its input. The bytes of a plan
/// file follow the line of `apply`, as they are.
enum Request {
    Plan(Plan),
    Apply,
    Status,
    Browse(Browse),
    Import(Import),
    /// An operation of another name.
    Other(String),
}

impl Request {
    /// The error of a reply of this request that passes the bound.
    fn large(&self) -> Error {
        match self {
            Self::Plan(_) | Self::Apply | Self::Other(_) => Error::LargeReply,
            Self::Status => Error::LargeStatus,
            Self::Browse(_) => Error::LargeBrowse,
            Self::Import(_) => Error::LargeImport,
        }
    }
}

/// The input of `plan`: each config file, with its path as the user gave it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    files: Vec<File>,
}

/// The input of `browse`: the name of a connector, each config file with its path as
/// the user gave it, and the query.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Browse {
    pub(crate) connector: String,
    pub(crate) files: Vec<File>,
    pub(crate) start: Option<String>,
    #[serde(rename = "match")]
    pub(crate) pattern: Option<String>,
    pub(crate) cursor: Option<String>,
    #[serde(default)]
    pub(crate) values: bool,
}

/// The input of `import`: the name of a connector, each config file with its path as
/// the user gave it, the start of the walk, and the points to pick or the match.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Import {
    pub(crate) connector: String,
    pub(crate) files: Vec<File>,
    pub(crate) start: Option<String>,
    #[serde(default)]
    pub(crate) points: Vec<Point>,
    #[serde(rename = "match")]
    pub(crate) pattern: Option<String>,
}

/// One point that an import picks.
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Point {
    /// Its names from the root, as `browse` gives its path.
    pub(crate) path: String,
    /// The name of its channel. The default is `<connector>.<path from the start>`.
    #[serde(default)]
    pub(crate) name: Option<String>,
}

/// A `T` that takes only an object: a derived struct also takes a sequence.
struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Visitor<T> {
            type Value = Object<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Object<T>, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Object)
            }
        }

        deserializer.deserialize_map(Visitor(PhantomData))
    }
}

/// The fields of a [`Point`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PointFields {
    path: String,
    #[serde(default)]
    name: Option<String>,
}

impl<'de> Deserialize<'de> for Point {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let Object(PointFields { path, name }) = Object::deserialize(deserializer)?;
        Ok(Self { path, name })
    }
}

/// The fields of a [`File`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFields {
    path: PathBuf,
    text: String,
}

impl<'de> Deserialize<'de> for File {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let Object(FileFields { path, text }) = Object::deserialize(deserializer)?;
        Ok(Self { path, text })
    }
}

impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Request;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object with one operation")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Request, A::Error> {
                let Some(name) = map.next_key::<String>()? else {
                    return Err(de::Error::custom("no operation"));
                };
                let request = match name.as_str() {
                    "plan" => {
                        let Object(plan) = map.next_value()?;
                        Request::Plan(plan)
                    }
                    "apply" => {
                        let Object(Empty {}) = map.next_value()?;
                        Request::Apply
                    }
                    "status" => {
                        let Object(Empty {}) = map.next_value()?;
                        Request::Status
                    }
                    "browse" => {
                        let Object(browse) = map.next_value()?;
                        Request::Browse(browse)
                    }
                    "import" => {
                        let Object(import) = map.next_value()?;
                        Request::Import(import)
                    }
                    _ => {
                        let IgnoredAny = map.next_value()?;
                        Request::Other(name)
                    }
                };
                match map.next_key::<IgnoredAny>()? {
                    Some(IgnoredAny) => Err(de::Error::custom("a second operation")),
                    None => Ok(request),
                }
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

/// The first line of a reply: the output of an operation that ran, or its errors with
/// the exit status of the program. `fitting` counts the layout of `Failed`.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Answer<T, E> {
    Done { output: T },
    Failed { errors: E, status: u8 },
}

/// The errors of a reply over the bound: the problems that fit, then
/// `ops.more-problems`.
struct Kept<'a> {
    problems: &'a [Problem],
    more: Problem,
}

impl Serialize for Kept<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.problems.iter().chain([&self.more]))
    }
}

/// Whether `len` bytes are within the bound.
fn fits(len: usize) -> bool {
    u64::try_from(len).is_ok_and(|len| len <= BODY_BYTES_MAX)
}

/// The reply that holds `error`. When that reply passes the bound, it holds the first
/// problems of `error` that fit, then `ops.more-problems` for the rest, with the status
/// of `error`, or it is the reply of `large` when not even the first problem fits.
fn failed(error: &Error, large: &Error) -> Body {
    let problems = error.problems();
    let status = error.status();
    Body::failed(&*problems, status).unwrap_or_else(|Large| {
        let kept = fitting(&problems, status);
        if kept == 0 {
            return Body::failed(&*large.problems(), large.status()).unwrap_or_else(
                |Large| panic!("invariant: the reply of an error of the bound fits"),
            );
        }
        let errors = Kept {
            problems: &problems[..kept],
            more: Problem::more(problems.len() - kept),
        };
        Body::failed(&errors, status)
            .unwrap_or_else(|Large| panic!("invariant: the problems that fit fit"))
    })
}

/// How many of the first `problems` fit in one reply of `status`, with
/// `ops.more-problems` for the rest. It encodes each problem at most once.
fn fitting(problems: &[Problem], status: u8) -> usize {
    let empty = Answer::<(), &[Problem]>::Failed {
        errors: &[],
        status,
    };
    // Each problem that is kept adds one `,`, and the line ends in `\n`.
    let mut len = encoded(&empty).saturating_add(1);
    // The digits of the rest and the length of its `ops.more-problems`, which only
    // the count of digits changes.
    let mut more = (None, 0);
    let mut kept = 0;
    for (index, problem) in problems.iter().enumerate() {
        len = len.saturating_add(encoded(problem)).saturating_add(1);
        let rest = problems.len() - index - 1;
        let digits = Some(rest.checked_ilog10());
        if more.0 != digits {
            more = (digits, encoded(&Problem::more(rest)));
        }
        if !fits(len.saturating_add(more.1)) {
            break;
        }
        kept = index + 1;
    }
    kept
}

/// The length of `value` as JSON.
pub(crate) fn encoded(value: &impl Serialize) -> usize {
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).unwrap_or_else(|error| {
        panic!("invariant: a body is plain JSON data: {error}")
    });
    count.0
}

/// A writer that counts the bytes written to it.
struct Count(usize);

impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The body of a request of `plan` of `files`, for [`serve`].
///
/// # Errors
///
/// [`Error::LargeRequest`] when the body passes the bound.
///
/// # Panics
///
/// When the path of a file is not UTF-8 and the body before that path fits the bound.
pub(crate) fn plan_request(files: &[File]) -> Result<Vec<u8>, Error> {
    let plan = BTreeMap::from([("files", files)]);
    let body = Body::line(&BTreeMap::from([("plan", plan)]));
    body.map(|body| body.0).map_err(|Large| Error::LargeRequest)
}

/// The body of a request of `apply` of the plan file `plan`, for [`serve`].
///
/// # Errors
///
/// [`Error::LargeRequest`] when the body passes the bound.
pub(crate) fn apply_request(plan: &[u8]) -> Result<Vec<u8>, Error> {
    let large = |Large| Error::LargeRequest;
    let mut body = Body::line(&json!({ "apply": {} })).map_err(large)?;
    body.add(plan).map_err(large)?;
    Ok(body.0)
}

/// The body of a request of `browse`, for [`serve`].
///
/// # Errors
///
/// [`Error::LargeRequest`] when the body passes the bound.
///
/// # Panics
///
/// When the path of a file is not UTF-8 and the body before that path fits the bound.
pub(crate) fn browse_request(browse: &Browse) -> Result<Vec<u8>, Error> {
    let body = Body::line(&BTreeMap::from([("browse", browse)]));
    body.map(|body| body.0).map_err(|Large| Error::LargeRequest)
}

/// The body of a request of `import`, for [`serve`].
///
/// # Errors
///
/// [`Error::LargeRequest`] when the body passes the bound.
///
/// # Panics
///
/// When the path of a file is not UTF-8 and the body before that path fits the bound.
pub(crate) fn import_request(import: &Import) -> Result<Vec<u8>, Error> {
    let body = Body::line(&BTreeMap::from([("import", import)]));
    body.map(|body| body.0).map_err(|Large| Error::LargeRequest)
}

/// The body of a request of `status`, for [`serve`].
pub(crate) const STATUS_REQUEST: &[u8] = b"{\"status\":{}}\n";

/// The output of the reply `body` of [`serve`] to a request of `plan`, and the plan
/// file after its first line.
///
/// # Errors
///
/// Each error of [`answer`], and [`Error::Reply`] when no plan follows the output.
pub(crate) fn plan_reply(body: &[u8]) -> Result<(Output, &[u8]), Error> {
    let (output, plan) = answer(body)?;
    if plan.is_empty() {
        return Err(bad_reply(
            "no plan follows the first line of the reply of `plan`",
        ));
    }
    Ok((output, plan))
}

/// The output of the reply `body` of [`serve`] to a request of `import`, and the new
/// text of the file after its first line.
///
/// # Errors
///
/// Each error of [`answer`], and [`Error::Reply`] when no UTF-8 text follows the
/// output.
pub(crate) fn import_reply(
    body: &[u8],
) -> Result<(crate::import::Output, &str), Error> {
    let (output, text) = answer(body)?;
    if text.is_empty() {
        return Err(bad_reply(
            "no text follows the first line of the reply of `import`",
        ));
    }
    let text = std::str::from_utf8(text).map_err(|_not_utf8| {
        bad_reply("the text of the reply of `import` is not UTF-8")
    })?;
    Ok((output, text))
}

/// The output of the reply `body` of [`serve`] to a request of `operation`.
///
/// # Errors
///
/// Each error of [`answer`], and [`Error::Reply`] when bytes follow the output.
pub(crate) fn reply<T: DeserializeOwned>(
    body: &[u8],
    operation: &str,
) -> Result<T, Error> {
    let (output, rest) = answer(body)?;
    if !rest.is_empty() {
        return Err(bad_reply(format!(
            "bytes follow the first line of the reply of `{operation}`"
        )));
    }
    Ok(output)
}

/// The output of the reply `body` of [`serve`], and the bytes after its first line.
///
/// # Errors
///
/// - [`Error::Remote`] with the errors of the reply and its exit status.
/// - [`Error::Reply`] when `body` is no reply, or holds no error with a status of 0.
fn answer<T: DeserializeOwned>(body: &[u8]) -> Result<(T, &[u8]), Error> {
    let Some((head, rest)) = first_line(body) else {
        return Err(bad_reply("no newline ends the first line"));
    };
    match serde_json::from_slice::<Answer<Value, Vec<Problem>>>(head) {
        Ok(Answer::Done { output }) => match T::deserialize(output) {
            Ok(output) => Ok((output, rest)),
            Err(error) => Err(bad_reply(error.to_string())),
        },
        Ok(Answer::Failed { errors, status }) if status == 0 || errors.is_empty() => {
            Err(bad_reply("an error has the exit status 0, or no problem"))
        }
        Ok(Answer::Failed { errors, status }) => Err(Error::Remote {
            problems: errors,
            status,
        }),
        Err(error) => Err(bad_reply(error.to_string())),
    }
}

/// The first line of `body` and the bytes after its newline, or `None` when no newline
/// ends it.
fn first_line(body: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = body.iter().position(|byte| *byte == b'\n')?;
    Some((&body[..end], &body[end + 1..]))
}

/// The most bytes of the cause that `ops.bad-request` quotes.
const CAUSE_BYTES_MAX: usize = 256;

/// `Request` with the first `CAUSE_BYTES_MAX` bytes of `message`, cut at a char
/// boundary, then `...` when it cuts.
fn bad_request(message: impl Into<String>) -> Error {
    let mut message = message.into();
    if message.len() > CAUSE_BYTES_MAX {
        message.truncate(message.floor_char_boundary(CAUSE_BYTES_MAX));
        message.push_str("...");
    }
    Error::Request { message }
}

/// `Reply` with `message`.
fn bad_reply(message: impl Into<String>) -> Error {
    Error::Reply {
        message: message.into(),
    }
}

/// The request of `body` and the bytes after its first line.
///
/// # Errors
///
/// [`Error::Request`] when the first line is no request.
fn read(body: &[u8]) -> Result<(Request, &[u8]), Error> {
    let Some((head, rest)) = first_line(body) else {
        return Err(bad_request("no newline ends the first line"));
    };
    let request =
        serde_json::from_slice(head).map_err(|e| bad_request(e.to_string()))?;
    Ok((request, rest))
}

/// The output of `request` of `subject`, with the bytes `rest` after its first line.
async fn run(
    node: &Node,
    subject: &Name,
    request: Request,
    rest: &[u8],
) -> Result<Ran, Error> {
    match request {
        Request::Plan(_) if !rest.is_empty() => {
            Err(bad_request("bytes follow the first line of `plan`"))
        }
        Request::Plan(Plan { files }) => plan(node, subject, &files).await,
        Request::Apply => apply(node, subject, rest).await,
        Request::Status if !rest.is_empty() => {
            Err(bad_request("bytes follow the first line of `status`"))
        }
        Request::Status => status(node, subject).await,
        Request::Browse(_) if !rest.is_empty() => {
            Err(bad_request("bytes follow the first line of `browse`"))
        }
        Request::Browse(request) => browse(node, subject, request).await,
        Request::Import(_) if !rest.is_empty() => {
            Err(bad_request("bytes follow the first line of `import`"))
        }
        Request::Import(request) => import(node, subject, request).await,
        Request::Other(name) => Err(bad_request(format!(
            "the operation `{name}` does not run on a node"
        ))),
    }
}

/// The access check of one request: `subject` needs an action on each name that the
/// operation reads or changes, by the rules of the spec in use.
struct Check<'a> {
    rules: Rules,
    subject: &'a Name,
}

impl<'a> Check<'a> {
    /// The check of `subject` by the access policies of `spec`.
    fn new(spec: &used::Spec, subject: &'a Name) -> Self {
        Self {
            rules: Rules::new([(Prefix::ROOT, spec.definitions().iter())]),
            subject,
        }
    }

    /// Whether an access policy gives `action` on `name`.
    fn allows(&self, action: Action, name: &Name) -> bool {
        let grant = self.rules.grant(self.subject, name);
        grant.actions().contains(action.into())
    }

    /// `Denied` of `action` on the first of `names` on which no access policy gives
    /// it, or `Ok` when each has a grant.
    fn names<'n>(
        &self,
        action: Action,
        names: impl IntoIterator<Item = &'n Name>,
    ) -> Result<(), Error> {
        match names.into_iter().find(|name| !self.allows(action, name)) {
            Some(name) => Err(self.denied(action, On::Name(name.clone()))),
            None => Ok(()),
        }
    }

    /// `Denied` unless `subject` may open a session to the device of the connector
    /// `name` with its secrets: `apply`, then `admin`, on `name`.
    fn opens(&self, name: &Name) -> Result<(), Error> {
        self.names(Action::Apply, [name])?;
        self.names(Action::Admin, [name])
    }

    /// `Denied` of `action` on `on`.
    fn denied(&self, action: Action, on: On) -> Error {
        Error::Denied {
            subject: self.subject.clone(),
            action,
            on,
        }
    }
}

/// `plan` of `files`: its output and its plan file.
async fn plan(node: &Node, subject: &Name, files: &[File]) -> Result<Ran, Error> {
    let files = Documents::read(files, &node.front_ends)?;
    let names = files.names(node.supervisor.kinds())?;
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject);
    check.names(Action::Plan, &names)?;
    let mut compared = spec
        .definitions()
        .iter()
        .filter(|(name, definition)| config::plan::planned(name, definition.kind()))
        .map(|(name, _)| name)
        .peekable();
    if names.is_empty() && compared.peek().is_none() {
        return Err(check.denied(Action::Plan, On::Nothing));
    }
    // The files do not hold this name, so the error does not tell what the spec holds.
    if compared.any(|name| !check.allows(Action::Plan, name)) {
        return Err(check.denied(Action::Plan, On::Spec));
    }
    let (base, applied) = spec.newest()?;
    // A subject that may not open a connector learns nothing of its sources.
    let readable = |name: &Name| check.opens(name).is_ok();
    let (output, plan) = files.plan(node, base, &applied, readable).await?;
    Ok(Ran::Plan(output, plan))
}

/// `apply` of the plan file `bytes`.
async fn apply(node: &Node, subject: &Name, bytes: &[u8]) -> Result<Ran, Error> {
    let mut plan = config::plan::Plan::decode(bytes).map_err(Error::Plan)?;
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject);
    check.names(Action::Apply, plan.changes.keys())?;
    let edges = plan.changes.values().filter_map(|change| {
        match &change.new.as_ref()?.definition {
            config::Definition::Channel(kind) => Some(kind),
            _ => None,
        }
    });
    check.names(
        Action::Apply,
        edges.flat_map(|kind| kind.edges().map(|(_, to)| to)),
    )?;
    // A policy governs its whole region, the keys of a subject decide who acts as it,
    // a region record makes a region, and a connector reaches channels by `select`,
    // so `apply` on a subtree would reach each name of the region.
    let admin = plan.changes.iter().filter_map(|(name, change)| {
        let stored = spec.definitions().get(name).map(Definition::kind);
        let new = change.new.as_ref().map(|entry| entry.definition.kind());
        // A connector and a channel share a key, so a change can turn one into the
        // other.
        let exempt = matches!(
            (stored, new),
            (Some(Kind::Channel) | None, Some(Kind::Channel))
                | (Some(Kind::Channel), None)
        );
        (!exempt).then_some(name)
    });
    check.names(Action::Admin, admin)?;
    // The homes list each placed index, so a home with no grant drops with no error.
    plan.homes
        .retain(|index, _| check.allows(Action::Apply, index));
    // Else a subject with no grant learns the pointer in use from the output.
    if plan.changes.is_empty() && plan.homes.is_empty() {
        return Err(check.denied(Action::Apply, On::Nothing));
    }
    let effect =
        apply::apply(plan, spec, &node.mesh, node.supervisor.kinds(), &node.keys)
            .await?;
    Ok(Ran::Apply(effect))
}

/// `status`. It leaves out each connector unless `subject` has `read` on its label and
/// on each of its status channels. When it leaves out each connector, or the node
/// places none, it needs `read` on the name of the node. It gives the failures of the
/// OS roots only with `read` on the name of the node.
async fn status(node: &Node, subject: &Name) -> Result<Ran, Error> {
    let asked = node.clock.now();
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject);
    let mut placed = status::placed(spec.definitions(), &node.name);
    placed.retain(|placed| {
        check.allows(Action::Read, placed.name())
            && placed
                .channels()
                .all(|name| check.allows(Action::Read, name))
    });
    let readable = check.allows(Action::Read, &node.name);
    let connectors = if placed.is_empty() {
        if !readable {
            return Err(check.denied(Action::Read, On::Nothing));
        }
        Vec::new()
    } else {
        status::read(node, subject, &placed, asked).await?
    };
    let output = status::Output {
        connectors,
        roots: readable.then(|| node.roots.clone()),
    };
    Ok(Ran::Status(output))
}

/// The connector of a `browse` or an `import`, the files that hold it, and the start
/// of its walk.
struct Target {
    files: Documents,
    name: Name,
    connector: spec::connector::Connector,
    start: browse::Path,
}

/// The target of the connector `connector` in `files` from `start`, the root when it is
/// `None`. It checks, in this order, that the files read, that they hold the
/// connector, that `subject` may open it ([`Check::opens`]), that it runs on this node,
/// and that the start parses.
async fn target(
    node: &Node,
    subject: &Name,
    files: &[File],
    connector: &str,
    start: Option<String>,
) -> Result<Target, Error> {
    let files = Documents::read(files, &node.front_ends)?;
    let name: Name = connector.parse().map_err(|error| Error::Argument {
        message: format!("the connector `{connector}` is not a name: {error}"),
    })?;
    let connector = files.connector(&name)?;
    let spec = used::spec(&node.mesh).await?;
    Check::new(&spec, subject).opens(&name)?;
    if connector.node() != &node.name {
        return Err(Error::BrowseNode {
            connector: name,
            node: connector.node().clone(),
            this: node.name.clone(),
        });
    }
    let start = match start {
        None => browse::Path::root(),
        Some(text) => text
            .parse()
            .map_err(|cause| Error::BadStart { text, cause })?,
    };
    Ok(Target {
        files,
        name,
        connector,
        start,
    })
}

/// The pattern of the match `text`.
///
/// # Errors
///
/// [`Error::BadPattern`] when it does not parse.
fn pattern(text: String) -> Result<browse::Pattern, Error> {
    text.parse()
        .map_err(|cause| Error::BadPattern { text, cause })
}

/// `browse`. It checks what [`target`] checks, then that the match parses, and that the
/// request gives no match with a cursor.
async fn browse(node: &Node, subject: &Name, request: Browse) -> Result<Ran, Error> {
    let Target {
        files,
        name,
        connector,
        start,
    } = target(
        node,
        subject,
        &request.files,
        &request.connector,
        request.start,
    )
    .await?;
    let pattern = request.pattern.map(pattern).transpose()?;
    let scope = match (pattern, request.cursor) {
        (Some(_), Some(_)) => {
            return Err(Error::Argument {
                message: "a browse takes `match` or `cursor`, not both".to_owned(),
            });
        }
        (Some(pattern), None) => browse::Scope::Match(pattern),
        (None, cursor) => browse::Scope::Level(cursor.map(browse::Cursor::new)),
    };
    let values = if request.values {
        browse::Values::Read
    } else {
        browse::Values::Skipped
    };
    let query = browse::Query {
        start,
        scope,
        values,
    };
    let kind = connector.kind();
    let found = node
        .supervisor
        .browse(kind.as_str(), connector.config().document(), &query)
        .await
        .map_err(|cause| Error::browse(&name, kind, cause, files.sources()))?;
    Ok(Ran::Browse(crate::browse::Output::of(found)))
}

/// `import`: its output and the new text of the file of the connector block. It checks
/// what [`target`] checks. Then that the request gives `points` or `match` but not
/// both, at most [`browse::LIMIT`] points, each path a path, no path twice, and each
/// name a name. Then that the match parses. Then that the walk did not stop at its
/// budget or at its limit, and that each point is one that the connector can read or
/// write. It leaves out each point of a match that the connector cannot.
async fn import(node: &Node, subject: &Name, request: Import) -> Result<Ran, Error> {
    let Target {
        files,
        name,
        connector,
        start,
    } = target(
        node,
        subject,
        &request.files,
        &request.connector,
        request.start,
    )
    .await?;
    let (scope, names) = picked(request.points, request.pattern)?;
    let query = browse::Query {
        start,
        scope,
        values: browse::Values::Skipped,
    };
    let kind = connector.kind();
    let found = node
        .supervisor
        .browse(kind.as_str(), connector.config().document(), &query)
        .await
        .map_err(|cause| Error::browse(&name, kind, cause, files.sources()))?;
    let (picks, unreadable) = match &names {
        Some(names) => (pointed(&found, names)?, 0),
        None => matched(&found)?,
    };
    let added = files.import(&name, &query.start, &picks, node.supervisor.kinds())?;
    let file = &request.files
        [usize::try_from(added.source.0).expect("invariant: a source fits a usize")];
    let text = front_end::update(file, added.source, &added.document, &node.front_ends)
        .map_err(|diagnostics| Error::config(diagnostics, files.sources()))?;
    let path = file
        .path
        .to_str()
        .expect("invariant: `Documents::read` read only UTF-8 paths")
        .to_owned();
    let output = crate::import::Output::of(path, &picks, &added, unreadable);
    Ok(Ran::Import(output, text))
}

/// The given name of each point of a walk of points, in the order of the paths.
type Given = Vec<Option<Name>>;

/// The scope of an import of `points` or of the match `pattern`, and the given name
/// of each point, or `None` for a match.
///
/// # Errors
///
/// - [`Error::Argument`] for both.
/// - [`Error::NoPick`] for neither.
/// - The errors of [`pattern`] and [`points`].
fn picked(
    points: Vec<Point>,
    pattern: Option<String>,
) -> Result<(browse::Scope, Option<Given>), Error> {
    match (points.is_empty(), pattern) {
        (false, Some(_)) => Err(Error::Argument {
            message: "an import takes `points` or `match`, not both".to_owned(),
        }),
        (true, None) => Err(Error::NoPick),
        (true, Some(text)) => Ok((browse::Scope::Match(self::pattern(text)?), None)),
        (false, None) => {
            let (paths, names) = self::points(points)?;
            Ok((browse::Scope::Points(paths), Some(names)))
        }
    }
}

/// The picks of the walk of a match, and the count of points that the connector
/// cannot read or write, which it leaves out.
///
/// # Errors
///
/// [`Error::MatchBudget`] or [`Error::MatchLimit`] when the walk stopped early.
fn matched(
    found: &browse::Found,
) -> Result<(Vec<config::import::Pick<'_>>, usize), Error> {
    match found.end {
        browse::End::Done => {}
        browse::End::Budget => return Err(Error::MatchBudget(Stop::Budget)),
        browse::End::Deadline => return Err(Error::MatchBudget(Stop::Deadline)),
        browse::End::Limit => return Err(Error::MatchLimit),
        browse::End::Next(_) => unreachable!("invariant: only a level gives a cursor"),
    }
    let mut unreadable = 0;
    let picks = (found.entries.iter())
        .filter_map(|entry| match &entry.shape {
            browse::Shape::Point(facts) => Some(pick(entry, facts, None)),
            browse::Shape::Unreadable(_) => {
                unreadable += 1;
                None
            }
            browse::Shape::Folder => None,
        })
        .collect();
    Ok((picks, unreadable))
}

/// The picks of the walk of points, each with its name in `names`, in the order of
/// the paths.
///
/// # Errors
///
/// - [`Error::PickBudget`] when the walk stopped at its budget or its deadline.
/// - [`Error::UnreadablePoint`] with each point that the connector cannot read or
///   write.
///
/// # Panics
///
/// When the walk gives other than one point or unreadable entry for each name.
fn pointed<'a>(
    found: &'a browse::Found,
    names: &'a [Option<Name>],
) -> Result<Vec<config::import::Pick<'a>>, Error> {
    match found.end {
        browse::End::Done => {}
        browse::End::Budget => return Err(Error::PickBudget(Stop::Budget)),
        browse::End::Deadline => return Err(Error::PickBudget(Stop::Deadline)),
        browse::End::Limit | browse::End::Next(_) => {
            unreachable!("invariant: a walk of points gives each path and no cursor")
        }
    }
    assert_eq!(
        found.entries.len(),
        names.len(),
        "invariant: a walk of points gives one entry for each path"
    );
    let mut picks = Vec::with_capacity(names.len());
    let mut unreadable = Vec::new();
    for (entry, name) in found.entries.iter().zip(names) {
        match &entry.shape {
            browse::Shape::Point(facts) => {
                picks.push(pick(entry, facts, name.as_ref()));
            }
            browse::Shape::Unreadable(reason) => {
                unreadable.push((entry.path.to_string(), reason.clone()));
            }
            browse::Shape::Folder => {
                unreachable!(
                    "invariant: a walk of points gives no folder, but gave one at `{}`",
                    entry.path
                )
            }
        }
    }
    if !unreadable.is_empty() {
        return Err(Error::UnreadablePoint { points: unreadable });
    }
    Ok(picks)
}

/// The pick of the point `entry`, with its `facts` and given `name`.
fn pick<'a>(
    entry: &'a browse::Entry,
    facts: &'a browse::Facts,
    name: Option<&'a Name>,
) -> config::import::Pick<'a> {
    config::import::Pick {
        path: &entry.path,
        tag: &entry.tag,
        facts,
        name,
    }
}

/// The paths and the given names of `points`.
///
/// # Errors
///
/// - [`Error::PickLimit`] for more than [`browse::LIMIT`] points.
/// - [`Error::BadPoint`] with each path that does not parse.
/// - [`Error::DuplicatePick`] with each path that is there more than once.
/// - [`Error::BadName`] with each given name that does not parse.
fn points(points: Vec<Point>) -> Result<(Vec<browse::Path>, Given), Error> {
    if points.len() > browse::LIMIT {
        return Err(Error::PickLimit {
            picked: points.len(),
        });
    }
    let mut paths = Vec::with_capacity(points.len());
    let mut given = Vec::with_capacity(points.len());
    let mut bad = Vec::new();
    for Point { path: text, name } in points {
        match text.parse::<browse::Path>() {
            Ok(path) => paths.push(path),
            Err(cause) => bad.push((text, cause)),
        }
        given.push(name);
    }
    if !bad.is_empty() {
        return Err(Error::BadPoint { points: bad });
    }
    let mut seen = BTreeMap::new();
    let mut twice = Vec::new();
    for path in &paths {
        let text = path.to_string();
        let count = seen.entry(text.clone()).or_insert(0_usize);
        *count += 1;
        if *count == 2 {
            twice.push(text);
        }
    }
    if !twice.is_empty() {
        return Err(Error::DuplicatePick { paths: twice });
    }
    let mut names = Vec::with_capacity(given.len());
    let mut bad = Vec::new();
    for (path, name) in paths.iter().zip(given) {
        let Some(text) = name else {
            names.push(None);
            continue;
        };
        match text.parse::<Name>() {
            Ok(name) => names.push(Some(name)),
            Err(cause) => bad.push((path.to_string(), text, cause)),
        }
    }
    if !bad.is_empty() {
        return Err(Error::BadName { names: bad });
    }
    Ok((paths, names))
}
