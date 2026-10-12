//! Runs the request of a remote program on the node.

use std::fmt;
use std::io::{self, Write};

use access::Rules;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess};
use serde::{Deserialize, Serialize};
use types::name::{Name, Prefix};
use wire::hub::client::BODY_BYTES_MAX;

use crate::error::{Action, Error, On, Problem};
use crate::front_end::File;
use crate::operation::Empty;
use crate::plan::Documents;
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
/// gives a home only to an index on which `subject` has `apply`, and drops each other
/// home with no error.
///
/// [`cli`]: crate::cli
pub async fn serve(node: &Node, subject: &Name, body: &[u8]) -> Vec<u8> {
    let reply = match run(node, subject, body).await {
        Ok(reply) => Ok(reply),
        Err(error) => failed(&error),
    };
    reply.unwrap_or_else(|_large| large()).0
}

/// The reply of `ops.large-reply`.
fn large() -> Reply {
    failed(&Error::LargeReply).expect("invariant: the reply of `ops.large-reply` fits")
}

/// The bytes of a reply: at most `BODY_BYTES_MAX`.
#[derive(Default)]
struct Reply(Vec<u8>);

impl Reply {
    /// The reply of `head` as one line of JSON. Stops at the bound.
    ///
    /// # Errors
    ///
    /// `LargeReply` when the line passes the bound.
    fn line(head: &impl Serialize) -> Result<Self, Error> {
        let mut reply = Self::default();
        match serde_json::to_writer(&mut reply, head) {
            Ok(()) => {}
            Err(error) if error.is_io() => return Err(Error::LargeReply),
            Err(error) => panic!("invariant: a reply is plain JSON data: {error}"),
        }
        reply.add(b"\n")?;
        Ok(reply)
    }

    /// Adds `bytes` to the end.
    ///
    /// # Errors
    ///
    /// `LargeReply`, with no byte added, when the reply would pass the bound.
    fn add(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let len = self.0.len().saturating_add(bytes.len());
        if u64::try_from(len).is_ok_and(|len| len <= BODY_BYTES_MAX) {
            self.0.extend_from_slice(bytes);
            Ok(())
        } else {
            Err(Error::LargeReply)
        }
    }
}

impl Write for Reply {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.add(bytes)
            .map(|()| bytes.len())
            .map_err(|_large| io::ErrorKind::FileTooLarge.into())
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

/// The first line of the reply of an operation that ran.
#[derive(Serialize)]
struct Done<'a, T> {
    output: &'a T,
}

/// The first line of the reply of an error, with the exit status of the program.
#[derive(Serialize)]
struct Failed<'a> {
    errors: &'a [Problem],
    status: u8,
}

/// The reply that holds `error`.
///
/// # Errors
///
/// `LargeReply` when the reply passes the bound.
fn failed(error: &Error) -> Result<Reply, Error> {
    Reply::line(&Failed {
        errors: &error.problems(),
        status: error.status(),
    })
}

/// `Request` with `message`.
fn bad(message: impl Into<String>) -> Error {
    Error::Request {
        message: message.into(),
    }
}

/// The reply of the request `body` of `subject`.
async fn run(node: &Node, subject: &Name, body: &[u8]) -> Result<Reply, Error> {
    let Some(end) = body.iter().position(|byte| *byte == b'\n') else {
        return Err(bad("no newline ends the first line"));
    };
    let (head, rest) = (&body[..end], &body[end + 1..]);
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

/// The access check of one request: `subject` needs `action` on each name that the
/// operation reads or changes, by the rules of the spec in use.
struct Check<'a> {
    rules: Rules,
    subject: &'a Name,
    action: Action,
}

impl<'a> Check<'a> {
    /// The check of `action` for `subject` by the access policies of `spec`.
    fn new(spec: &used::Spec, subject: &'a Name, action: Action) -> Self {
        Self {
            rules: Rules::new([(Prefix::ROOT, spec.definitions().iter())]),
            subject,
            action,
        }
    }

    /// Whether an access policy gives the action on `name`.
    fn allows(&self, name: &Name) -> bool {
        let grant = self.rules.grant(self.subject, name);
        grant.actions().contains(self.action.into())
    }

    /// The first of `names` on which no access policy gives the action.
    fn refused<'n>(
        &self,
        names: impl IntoIterator<Item = &'n Name>,
    ) -> Option<&'n Name> {
        names.into_iter().find(|name| !self.allows(name))
    }

    /// `Denied` on `on`.
    fn denied(&self, on: On) -> Error {
        Error::Denied {
            subject: self.subject.clone(),
            action: self.action,
            on,
        }
    }
}

/// The reply of `plan` of `files`: its output, then the plan file.
async fn plan(node: &Node, subject: &Name, files: &[File]) -> Result<Reply, Error> {
    let files = Documents::read(files, &node.front_ends)?;
    let names = files.names(&node.kinds)?;
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject, Action::Plan);
    if let Some(name) = check.refused(&names) {
        return Err(check.denied(On::Name(name.clone())));
    }
    let mut compared = spec
        .definitions()
        .iter()
        .filter(|(name, definition)| config::plan::planned(name, definition.kind()))
        .map(|(name, _)| name)
        .peekable();
    if names.is_empty() && compared.peek().is_none() {
        return Err(check.denied(On::Nothing));
    }
    // The files do not hold this name, so the error does not tell what the spec holds.
    if check.refused(compared).is_some() {
        return Err(check.denied(On::Spec));
    }
    let (base, applied) = spec.newest()?;
    let (output, plan) = files.plan(base, &applied, &node.mesh.names(), &node.kinds)?;
    let mut reply = Reply::line(&Done { output: &output })?;
    reply.add(&plan.encode())?;
    Ok(reply)
}

/// The reply of `apply` of the plan file `bytes`: its output.
async fn apply(node: &Node, subject: &Name, bytes: &[u8]) -> Result<Reply, Error> {
    let mut plan = config::plan::Plan::decode(bytes).map_err(Error::Plan)?;
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject, Action::Apply);
    if let Some(name) = check.refused(plan.changes.keys()) {
        return Err(check.denied(On::Name(name.clone())));
    }
    // The homes list each placed index, so a home with no grant drops with no error.
    plan.homes.retain(|index, _| check.allows(index));
    // Else a subject with no grant learns the pointer in use from the output.
    if plan.changes.is_empty() && plan.homes.is_empty() {
        return Err(check.denied(On::Nothing));
    }
    let effect = apply::apply(plan, spec, &node.mesh, &node.kinds, &node.key).await?;
    Reply::line(&Done { output: &effect })
}
