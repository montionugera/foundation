//! Runs the request of a remote program on the node.

use std::fmt;

use access::Rules;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess};
use serde::{Deserialize, Serialize};
use types::name::{Name, Prefix};
use wire::hub::client::BODY_BYTES_MAX;

use crate::error::{Action, Error, Problem};
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
/// The reply is at most `wire::hub::client::BODY_BYTES_MAX` bytes. No `body` makes
/// it panic. The reply holds `ops.bad-request` for a body that is no request of
/// `plan` or `apply`, `ops.denied` for an action that no access policy gives
/// `subject`, and `ops.large-reply` in place of a reply over the bound. `apply`
/// gives a home only to an index on which `subject` has `apply`, and drops each other
/// home with no error.
///
/// [`cli`]: crate::cli
pub async fn serve(node: &Node, subject: &Name, body: &[u8]) -> Vec<u8> {
    let reply = match run(node, subject, body).await {
        Ok(reply) => reply,
        Err(error) => failed(&error),
    };
    let bytes = reply.len();
    if u64::try_from(bytes).is_ok_and(|bytes| bytes <= BODY_BYTES_MAX) {
        reply
    } else {
        failed(&Error::LargeReply { bytes })
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

/// `head` as one line of JSON.
fn line(head: &impl Serialize) -> Vec<u8> {
    let mut line =
        serde_json::to_vec(head).expect("invariant: a reply is plain JSON data");
    line.push(b'\n');
    line
}

/// The reply that holds `error`.
fn failed(error: &Error) -> Vec<u8> {
    line(&Failed {
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

/// The reply of the request `body` of `subject`, with no bound.
async fn run(node: &Node, subject: &Name, body: &[u8]) -> Result<Vec<u8>, Error> {
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

    /// `Denied` on `name`, or on a name that the request does not hold.
    fn denied(&self, name: Option<&Name>) -> Error {
        Error::Denied {
            subject: self.subject.clone(),
            action: self.action,
            name: name.cloned(),
        }
    }
}

/// The reply of `plan` of `files`: its output, then the plan file.
async fn plan(node: &Node, subject: &Name, files: &[File]) -> Result<Vec<u8>, Error> {
    let files = Documents::read(files, &node.front_ends)?;
    let names = files.names(&node.kinds)?;
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject, Action::Plan);
    if let Some(name) = check.refused(&names) {
        return Err(check.denied(Some(name)));
    }
    let compared = spec
        .definitions()
        .iter()
        .filter(|(name, definition)| config::plan::planned(name, definition.kind()))
        .map(|(name, _)| name);
    // The files do not hold this name, so the error does not tell what the spec holds.
    if check.refused(compared).is_some() {
        return Err(check.denied(None));
    }
    let (base, applied) = spec.newest()?;
    let (output, plan) = files.plan(base, &applied, &node.mesh.names(), &node.kinds)?;
    let mut reply = line(&Done { output: &output });
    reply.extend(plan.encode());
    Ok(reply)
}

/// The reply of `apply` of the plan file `bytes`: its output.
async fn apply(node: &Node, subject: &Name, bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut plan = config::plan::Plan::decode(bytes).map_err(Error::Plan)?;
    let spec = used::spec(&node.mesh).await?;
    let check = Check::new(&spec, subject, Action::Apply);
    if let Some(name) = check.refused(plan.changes.keys()) {
        return Err(check.denied(Some(name)));
    }
    // The homes list each placed index, so a home with no grant drops with no error.
    plan.homes.retain(|index, _| check.allows(index));
    let effect = apply::apply(plan, spec, &node.mesh, &node.kinds, &node.key).await?;
    Ok(line(&Done { output: &effect }))
}
