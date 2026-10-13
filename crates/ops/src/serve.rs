//! The requests and replies of a remote program, and how the node runs each request.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Write};

use access::Rules;
use serde::de::{self, DeserializeOwned, Deserializer, IgnoredAny, MapAccess};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spec::definition::{Definition, Kind};
use types::name::{Name, Prefix};
use wire::hub::client::BODY_BYTES_MAX;

use crate::apply::Effect;
use crate::error::{Action, Error, On, Problem};
use crate::front_end::File;
use crate::operation::Empty;
use crate::plan::{Documents, Output};
use crate::{Node, apply, used};

#[cfg(test)]
mod tests;

/// Runs the request `body` of `subject` on `node`, and gives the body of the reply:
/// the output of the operation, or its error with code, message, and fix. `body` is
/// what [`cli`] sends. Call it only for a subject whose key the hub verified.
///
/// The reply is at most `wire::hub::client::BODY_BYTES_MAX` bytes, and `serve` stops
/// writing a reply at that bound. No `body` makes it panic. The reply holds
/// `ops.bad-request` for a body that is no request of `plan` or `apply`, `ops.denied`
/// for an action that no access policy gives `subject` and for a request that names
/// no definition, and `ops.large-reply` in place of a reply over the bound. `apply`
/// also needs `apply` on the channel at each edge of each channel that the plan adds
/// or changes, and `admin` on the key of each definition other than a channel that the
/// plan adds, changes, or removes. It gives a home only to an index on which `subject`
/// has `apply`, and drops each other home with no error.
///
/// [`cli`]: crate::cli
pub async fn serve(node: &Node, subject: &Name, body: &[u8]) -> Vec<u8> {
    let reply = run(node, subject, body).await;
    reply.unwrap_or_else(|error| failed(&error)).0
}

/// A reply that would pass the bound.
#[derive(Debug)]
struct Large;

impl From<Large> for Error {
    fn from(Large: Large) -> Self {
        Self::LargeReply
    }
}

/// The bytes of a reply: at most `BODY_BYTES_MAX`.
#[derive(Default)]
struct Reply(Vec<u8>);

impl Reply {
    /// The reply of `head` as one line of JSON. Stops at the bound.
    ///
    /// # Errors
    ///
    /// `Large` when the line passes the bound.
    fn line(head: &impl Serialize) -> Result<Self, Large> {
        let mut reply = Self::default();
        match serde_json::to_writer(&mut reply, head) {
            Ok(()) => {}
            Err(error) if error.is_io() => return Err(Large),
            Err(error) => panic!("invariant: a reply is plain JSON data: {error}"),
        }
        reply.add(b"\n")?;
        Ok(reply)
    }

    /// Adds `bytes` to the end.
    ///
    /// # Errors
    ///
    /// `Large`, with no byte added, when the reply would pass the bound.
    fn add(&mut self, bytes: &[u8]) -> Result<(), Large> {
        let len = self.0.len().saturating_add(bytes.len());
        if u64::try_from(len).is_ok_and(|len| len <= BODY_BYTES_MAX) {
            self.0.extend_from_slice(bytes);
            Ok(())
        } else {
            Err(Large)
        }
    }
}

impl Write for Reply {
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
    /// An operation of another name.
    Other(String),
}

/// The input of `plan`: each config file, with its path as the user gave it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    files: Vec<File>,
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
                    "plan" => Request::Plan(map.next_value()?),
                    "apply" => {
                        let Empty {} = map.next_value()?;
                        Request::Apply
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

/// The reply that holds `error`, or `ops.large-reply` when that reply passes the bound.
fn failed(error: &Error) -> Reply {
    let line = |error: &Error| {
        Reply::line(&Answer::<()>::Failed {
            errors: error.problems(),
            status: error.status(),
        })
    };
    line(error).unwrap_or_else(|Large| {
        line(&Error::LargeReply).unwrap_or_else(|Large| {
            panic!("invariant: the reply of `ops.large-reply` fits")
        })
    })
}

/// The body of a request of `plan` of `files`, for [`serve`].
///
/// # Panics
///
/// When the path of a file is not UTF-8.
pub(crate) fn plan_request(files: &[File]) -> Vec<u8> {
    let files =
        serde_json::to_value(files).expect("invariant: the path of each file is UTF-8");
    let mut body = json!({ "plan": { "files": files } })
        .to_string()
        .into_bytes();
    body.push(b'\n');
    body
}

/// The body of a request of `apply` of the plan file `plan`, for [`serve`].
pub(crate) fn apply_request(plan: &[u8]) -> Vec<u8> {
    [b"{\"apply\":{}}\n", plan].concat()
}

/// The output of the reply `body` of [`serve`] to a request of `plan`, and the plan
/// file after its first line.
///
/// # Errors
///
/// Each error of [`answer`], and [`Error::Reply`] when no plan follows the output.
pub(crate) fn plan_reply(body: &[u8]) -> Result<(Output, &[u8]), Error> {
    let (output, plan) = answer(body)?;
    if plan.is_empty() {
        return Err(Error::Reply {
            message: "no plan follows the first line of the reply of `plan`".to_owned(),
        });
    }
    Ok((output, plan))
}

/// The output of the reply `body` of [`serve`] to a request of `apply`.
///
/// # Errors
///
/// Each error of [`answer`], and [`Error::Reply`] when bytes follow the output.
pub(crate) fn apply_reply(body: &[u8]) -> Result<Effect, Error> {
    let (effect, rest) = answer(body)?;
    if !rest.is_empty() {
        return Err(Error::Reply {
            message: "bytes follow the first line of the reply of `apply`".to_owned(),
        });
    }
    Ok(effect)
}

/// The output of the reply `body` of [`serve`], and the bytes after its first line.
///
/// # Errors
///
/// - [`Error::Remote`] with the errors of the reply and its exit status.
/// - [`Error::Reply`] when `body` is no reply, or holds no error with a status of 0.
fn answer<T: DeserializeOwned>(body: &[u8]) -> Result<(T, &[u8]), Error> {
    let bad = |message: &str| Error::Reply {
        message: message.to_owned(),
    };
    let Some((head, rest)) = first_line(body) else {
        return Err(bad("no newline ends the first line"));
    };
    match serde_json::from_slice::<Answer<'_, Value>>(head) {
        Ok(Answer::Done { output }) => match T::deserialize(output) {
            Ok(output) => Ok((output, rest)),
            Err(error) => Err(bad(&error.to_string())),
        },
        Ok(Answer::Failed { errors, status }) if status == 0 || errors.is_empty() => {
            Err(bad("an error has the exit status 0, or no problem"))
        }
        Ok(Answer::Failed { errors, status }) => Err(Error::Remote {
            problems: errors.into_owned(),
            status,
        }),
        Err(error) => Err(bad(&error.to_string())),
    }
}

/// The first line of `body` and the bytes after its newline, or `None` when no newline
/// ends it.
fn first_line(body: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = body.iter().position(|byte| *byte == b'\n')?;
    Some((&body[..end], &body[end + 1..]))
}

/// `Request` with `message`.
fn bad(message: impl Into<String>) -> Error {
    Error::Request {
        message: message.into(),
    }
}

/// The reply of the request `body` of `subject`.
async fn run(node: &Node, subject: &Name, body: &[u8]) -> Result<Reply, Error> {
    let Some((head, rest)) = first_line(body) else {
        return Err(bad("no newline ends the first line"));
    };
    let request = serde_json::from_slice(head).map_err(|e| bad(e.to_string()))?;
    match request {
        Request::Plan(_) if !rest.is_empty() => {
            Err(bad("bytes follow the first line of `plan`"))
        }
        Request::Plan(Plan { files }) => plan(node, subject, &files).await,
        Request::Apply => apply(node, subject, rest).await,
        Request::Other(name) => Err(bad(format!(
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
async fn plan(node: &Node, subject: &Name, files: &[File]) -> Result<Reply, Error> {
    let files = Documents::read(files, &node.front_ends)?;
    let names = files.names(&node.kinds)?;
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
    let (output, plan) = files.plan(base, &applied, &node.mesh.names(), &node.kinds)?;
    let mut reply = Reply::line(&Answer::Done { output: &output })?;
    reply.add(&plan.encode())?;
    Ok(reply)
}

/// The reply of `apply` of the plan file `bytes`: its output.
async fn apply(node: &Node, subject: &Name, bytes: &[u8]) -> Result<Reply, Error> {
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
    let effect = apply::apply(plan, spec, &node.mesh, &node.kinds, &node.key).await?;
    Ok(Reply::line(&Answer::Done { output: &effect })?)
}
