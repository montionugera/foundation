//! The requests and replies of a remote program, and how the node runs each request.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::marker::PhantomData;

use access::Rules;
use connector::browse;
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, DeserializeOwned, Deserializer, IgnoredAny, MapAccess};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spec::definition::{Definition, Kind};
use types::name::{Name, Prefix};
use wire::hub::client::BODY_BYTES_MAX;

use crate::error::{Action, Error, On, Problem};
use crate::front_end::File;
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
/// `ops.bad-request` for a body that is no request of `plan`, `apply`, `status`, or
/// `browse`, `ops.denied` for an action that no access policy gives `subject` and for a
/// request that names no definition. An error reply over the bound keeps the problems
/// that fit, then `ops.more-problems` for the rest. `ops.large-reply` takes the place
/// of an output over the bound and of an error whose first problem does not fit, and
/// `ops.large-browse` takes it for `browse`. `apply` also needs `apply` on the channel
/// at each edge of each channel that the plan adds or changes, and `admin` on the key
/// of each definition other than a channel that the plan adds, changes, or removes. It
/// gives a home only to an index on which `subject` has `apply`, and drops each other
/// home with no error. `status` shows only each connector on whose label and status
/// channels `subject` has `read`. When it shows none, also when the node places none,
/// it needs `read` on the name of the node, and gives `ops.denied` with no grant. It
/// gives the failures of the read of the OS roots of the node only to a subject with
/// `read` on the name of the node. It waits at most 1 s from the request. A connector
/// is `unknown` when no status as the spec defines it comes in that time, and `status`
/// gives `ops.channel-read` when the reader of a connector fails for another cause.
/// `browse` needs `apply` and `admin` on the name of the connector, and browses only a
/// connector that the files place on this node.
///
/// [`cli`]: crate::cli
pub async fn serve(node: &Node, subject: &Name, body: &[u8]) -> Vec<u8> {
    let reply = run(node, subject, body).await;
    reply
        .unwrap_or_else(|error| failed(&error, &Error::LargeReply))
        .0
}

/// A body that would pass the bound.
#[derive(Debug)]
struct Large;

impl From<Large> for Error {
    fn from(Large: Large) -> Self {
        Self::LargeReply
    }
}

/// The bytes of a request or a reply: at most `BODY_BYTES_MAX`.
#[derive(Default)]
struct Body(Vec<u8>);

impl Body {
    /// The body of `head` as one line of JSON. Stops at the bound.
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
    /// An operation of another name.
    Other(String),
}

/// The input of `plan`: each config file, with its path as the user gave it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    #[serde(deserialize_with = "objects")]
    files: Vec<File>,
}

/// The input of `browse`: the name of a connector, each config file with its path as
/// the user gave it, and the query.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Browse {
    pub(crate) connector: String,
    #[serde(deserialize_with = "objects")]
    pub(crate) files: Vec<File>,
    pub(crate) start: Option<String>,
    #[serde(rename = "match")]
    pub(crate) pattern: Option<String>,
    pub(crate) cursor: Option<String>,
    #[serde(default)]
    pub(crate) values: bool,
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

/// The files of a [`Plan`], each only from an object.
fn objects<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<File>, D::Error> {
    let files = Vec::<Object<File>>::deserialize(deserializer)?;
    Ok(files.into_iter().map(|Object(file)| file).collect())
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
/// the exit status of the program.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Answer<'a, T> {
    Done {
        output: T,
    },
    Failed {
        errors: Cow<'a, [Problem]>,
        status: u8,
    },
}

/// Whether `len` bytes are within the bound.
fn fits(len: usize) -> bool {
    u64::try_from(len).is_ok_and(|len| len <= BODY_BYTES_MAX)
}

/// The reply that holds `error`. When that reply passes the bound, it holds the first
/// problems of `error` that fit, then `ops.more-problems` for the rest, with the status
/// of `error`, or it is the reply of `large` when not even the first problem fits.
fn failed(error: &Error, large: &Error) -> Body {
    let line = |errors: Cow<'_, [Problem]>, status: u8| {
        Body::line(&Answer::<()>::Failed { errors, status })
    };
    let problems = error.problems();
    let status = error.status();
    line(Cow::Borrowed(&problems), status).unwrap_or_else(|Large| {
        let kept = fitting(&problems, status);
        if kept == 0 {
            return line(large.problems(), large.status()).unwrap_or_else(|Large| {
                panic!("invariant: the reply of an error of the bound fits")
            });
        }
        let mut errors = problems[..kept].to_vec();
        errors.push(Problem::more(problems.len() - kept));
        line(Cow::Owned(errors), status)
            .unwrap_or_else(|Large| panic!("invariant: the problems that fit fit"))
    })
}

/// How many of the first `problems` fit in one reply of `status`, with
/// `ops.more-problems` for the rest. It encodes each problem at most once.
fn fitting(problems: &[Problem], status: u8) -> usize {
    let empty = Answer::<()>::Failed {
        errors: Cow::Borrowed(&[]),
        status,
    };
    // Each problem that is kept adds one `,`, and the line ends in `\n`.
    let mut len = encoded(&empty).saturating_add(1);
    let mut kept = 0;
    for (index, problem) in problems.iter().enumerate() {
        len = len.saturating_add(encoded(problem)).saturating_add(1);
        let more = Problem::more(problems.len() - index - 1);
        if !fits(len.saturating_add(encoded(&more))) {
            break;
        }
        kept = index + 1;
    }
    kept
}

/// The length of `value` as JSON.
fn encoded(value: &impl Serialize) -> usize {
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
    match serde_json::from_slice::<Answer<'_, Value>>(head) {
        Ok(Answer::Done { output }) => match T::deserialize(output) {
            Ok(output) => Ok((output, rest)),
            Err(error) => Err(bad_reply(error.to_string())),
        },
        Ok(Answer::Failed { errors, status }) if status == 0 || errors.is_empty() => {
            Err(bad_reply("an error has the exit status 0, or no problem"))
        }
        Ok(Answer::Failed { errors, status }) => Err(Error::Remote {
            problems: errors.into_owned(),
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

/// `Request` with `message`.
fn bad_request(message: impl Into<String>) -> Error {
    Error::Request {
        message: message.into(),
    }
}

/// `Reply` with `message`.
fn bad_reply(message: impl Into<String>) -> Error {
    Error::Reply {
        message: message.into(),
    }
}

/// The reply of the request `body` of `subject`.
async fn run(node: &Node, subject: &Name, body: &[u8]) -> Result<Body, Error> {
    let Some((head, rest)) = first_line(body) else {
        return Err(bad_request("no newline ends the first line"));
    };
    let request =
        serde_json::from_slice(head).map_err(|e| bad_request(e.to_string()))?;
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
        Request::Browse(request) => Ok(browse(node, subject, request)
            .await
            .unwrap_or_else(|error| failed(&error, &Error::LargeBrowse))),
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

/// The reply of `plan` of `files`: its output, then the plan file.
async fn plan(node: &Node, subject: &Name, files: &[File]) -> Result<Body, Error> {
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
    let mut reply = Body::line(&Answer::Done { output: &output })?;
    reply.add(&plan.encode())?;
    Ok(reply)
}

/// The reply of `apply` of the plan file `bytes`: its output.
async fn apply(node: &Node, subject: &Name, bytes: &[u8]) -> Result<Body, Error> {
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
    Ok(Body::line(&Answer::Done { output: &effect })?)
}

/// The reply of `status`: its output. It leaves out each connector unless `subject`
/// has `read` on its label and on each of its status channels. When it leaves out each
/// connector, or the node places none, it needs `read` on the name of the node. It
/// gives the failures of the OS roots only with `read` on the name of the node.
async fn status(node: &Node, subject: &Name) -> Result<Body, Error> {
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
    Ok(Body::line(&Answer::Done { output: &output })?)
}

/// The reply of `browse`: its output. It checks, in this order, that the files read,
/// that they hold the connector, that `subject` may open it ([`Check::opens`]), that it
/// runs on this node, that the start and the match parse, and that the request gives
/// no match with a cursor.
async fn browse(node: &Node, subject: &Name, request: Browse) -> Result<Body, Error> {
    let files = Documents::read(&request.files, &node.front_ends)?;
    let name: Name = request.connector.parse().map_err(|error| Error::Argument {
        message: format!(
            "the connector `{}` is not a name: {error}",
            request.connector
        ),
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
    let start = match request.start {
        None => browse::Path::root(),
        Some(text) => text
            .parse()
            .map_err(|cause| Error::BadStart { text, cause })?,
    };
    let pattern = request
        .pattern
        .map(|text| {
            text.parse()
                .map_err(|cause| Error::BadPattern { text, cause })
        })
        .transpose()?;
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
    let output = crate::browse::Output::of(found);
    Body::line(&Answer::Done { output: &output }).map_err(|Large| Error::LargeBrowse)
}
