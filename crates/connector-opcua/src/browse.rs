//! The browse of the `opcua` kind: the tree of a server from its Objects folder, as
//! [`browse::walk`] walks it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::num::{NonZeroU32, NonZeroUsize};

use connector::browse::{self, Access, Child, Cursor, Entry, Facts, Page, Shape};
use connector::kind::Error;
use document::diagnostic::{Code, Diagnostic};
use types::sample::{self, Type};

use crate::client::Client;
use crate::client::browse::{
    self as client, Base, Cause, Class, Description, Point, Reference,
};
use crate::ffi::Status;
use crate::kind::{Failure, device};
use crate::node::{Node, ns0};

const BAD_CURSOR: Code = Code::new("opcua.bad-cursor");

/// The most references that a cursor with no continuation point of the browse skips.
const SKIP: usize = 100_000;

/// The Objects folder, the root of a browse.
const OBJECTS: Node = Node::ns0(ns0::OBJECTS);

/// The bit `CurrentRead` of a `UserAccessLevel`.
const CURRENT_READ: u8 = 1;

/// A server open for one browse, on the session of a client.
pub(crate) struct Server<'a> {
    client: &'a mut Client,
    /// What each cursor of this browse goes on from, by its folder and the count of
    /// references of the folder before its page.
    points: BTreeMap<(Node, usize), Held>,
    /// The count of points that this browse held so far, which orders them by use.
    uses: u64,
}

/// Where the page of a cursor starts.
struct Held {
    /// The continuation point of the next page of the server.
    point: Point,
    /// The value of `Server::uses` when this browse held the point.
    used: u64,
    /// What the references of the folder before the point gave.
    seen: Seen,
    /// The count of references from the point that are before the page.
    left: usize,
}

/// The nodes and names that the references of one folder gave before a page.
#[derive(Default)]
struct Seen {
    /// The key of each node, as [`key`] gives it.
    nodes: BTreeSet<Vec<u8>>,
    names: BTreeSet<String>,
}

impl<'a> Server<'a> {
    /// Opens a browse on the session of `client`.
    pub(crate) fn new(client: &'a mut Client) -> Self {
        Self {
            client,
            points: BTreeMap::new(),
            uses: 0,
        }
    }

    /// Gives the references of `folder` from the count `before`, what the references
    /// before them gave, and the continuation point after them with the count of its
    /// references still to skip. A cursor whose point this browse holds goes on with
    /// `BrowseNext`. Else, or when the server freed its point, it browses `folder`
    /// again from its start and skips `before` references. It reads pages of the
    /// server while each gives only references to skip, so it gives no references and
    /// a held point after a page with none. `None` when the server does not have
    /// `folder`.
    ///
    /// # Errors
    ///
    /// `opcua.bad-cursor` when `before` is past [`SKIP`] and this browse holds no
    /// point for `cursor`, `BadContinuationPointInvalid` when it holds one that the
    /// server freed, and the status of a Browse or `BrowseNext` that fails.
    async fn references(
        &mut self,
        folder: &Node,
        cursor: Option<&Cursor>,
        before: usize,
    ) -> Result<Option<(Vec<Reference>, Seen, Option<(Point, usize)>)>, Error> {
        let resumed = match self.points.remove(&(folder.clone(), before)) {
            Some(held) => match self.client.browse_next(held.point).await {
                Ok(page) => Some((page, held.seen, held.left)),
                Err(Status::BAD_CONTINUATION_POINT_INVALID) if before <= SKIP => None,
                Err(status) => return Err(failed(status)),
            },
            None => None,
        };
        let (mut page, mut seen, mut left) = match resumed {
            Some(resumed) => resumed,
            None if before > SKIP => {
                return Err(bad_cursor(cursor.expect("a count past 0 has a cursor")));
            }
            None => {
                let Some(page) = self.first(folder).await? else {
                    return Ok(None);
                };
                (page, Seen::default(), before)
            }
        };
        loop {
            let skipped = left.min(page.references.len());
            let rest = page.references.split_off(skipped);
            for reference in &page.references {
                seen.name(&reference.name, key(reference), &tag(reference));
            }
            left -= skipped;
            match page.next {
                Some(point) if rest.is_empty() && skipped > 0 => {
                    page = self.client.browse_next(point).await.map_err(failed)?;
                }
                next => return Ok(Some((rest, seen, next.map(|point| (point, left))))),
            }
        }
    }

    /// Gives the first page of `folder`, or `None` when the server does not have it.
    /// When this browse holds as many points as the server holds for a session, or
    /// [`browse::DEPTH`] when the server gives no bound, it first releases one point,
    /// as [`Server::release`] picks it. On `BadNoContinuationPoints`, it releases each
    /// point that [`Server::release`] can and browses once more.
    async fn first(&mut self, folder: &Node) -> Result<Option<client::Page>, Error> {
        let most = u32::try_from(browse::LIMIT).ok().and_then(NonZeroU32::new);
        let most = most.expect("invariant: `LIMIT` is a u32 that is not 0");
        let bound = self.client.points().await.map_err(failed)?;
        let bound = bound.map_or(browse::DEPTH, NonZeroUsize::get);
        if self.points.len() >= bound {
            self.release(1).await?;
        }
        match self.client.browse(folder, most).await {
            Err(Status::BAD_NO_CONTINUATION_POINTS) => {
                self.release(usize::MAX).await?;
                self.client.browse(folder, most).await
            }
            browsed => browsed,
        }
        .map_err(failed)
    }

    /// Releases the `most` points that this browse used least lately of those whose
    /// cursor can browse its folder again, at a count of at most [`SKIP`].
    async fn release(&mut self, most: usize) -> Result<(), Error> {
        let mut held: Vec<(u64, (Node, usize))> = self
            .points
            .iter()
            .filter(|((_, before), _)| *before <= SKIP)
            .map(|(key, held)| (held.used, key.clone()))
            .collect();
        held.sort_unstable_by_key(|(used, _)| *used);
        let points: Vec<Point> = held
            .into_iter()
            .take(most)
            .map(|(_, key)| {
                self.points
                    .remove(&key)
                    .expect("invariant: each key of `held` is a key of `points`")
                    .point
            })
            .collect();
        self.client.release(&points).await.map_err(failed)
    }

    /// Gives the shape of each of `references`, with one `Client::describe` of its
    /// variables.
    async fn shapes(
        &mut self,
        references: &[&Reference],
    ) -> Result<Vec<Shape>, Status> {
        let variables: Vec<Node> = references
            .iter()
            .filter(|reference| reference.class == Class::Variable)
            .filter_map(|reference| reference.node.as_ref().ok().cloned())
            .collect();
        let mut descriptions = self.client.describe(&variables).await?.into_iter();
        let shapes =
            references.iter().map(|reference| match &reference.node {
                Err(foreign) => Shape::Unreadable(reason(foreign.cause).into()),
                Ok(_) if reference.class != Class::Variable => Shape::Folder,
                Ok(_) => variable(descriptions.next().expect(
                    "invariant: describe gives one description for each variable",
                )),
            });
        Ok(shapes.collect())
    }
}

impl Seen {
    /// Gives the name of a reference of the browse name `name` to the node of the key
    /// `key` and the tag `tag`: `name`, or `tag` when `name` is empty, with
    /// ` (<tag>)` after it for each time that an earlier name has it. `None` when an
    /// earlier reference gave the node.
    fn name(&mut self, name: &str, key: Vec<u8>, tag: &str) -> Option<String> {
        if !self.nodes.insert(key) {
            return None;
        }
        let mut name = if name.is_empty() { tag } else { name }.to_owned();
        while self.names.contains(&name) {
            name = format!("{name} ({tag})");
        }
        self.names.insert(name.clone());
        Some(name)
    }
}

impl browse::Device for Server<'_> {
    /// Gives the references of a page of at most [`browse::LIMIT`], less each one to
    /// a node that an earlier reference of the folder gave. The root is the Objects
    /// folder. The text of a cursor is the count of references of the folder before
    /// its page. A page of the server with no references gives a page with no
    /// entries and a cursor, so [`browse::walk`] counts it.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `opcua.bad-cursor` when the text of `cursor` is not a
    /// count, or is a count past 100 000 that this browse did not give.
    /// [`Error::Device`] when a Browse, a `BrowseNext`, or a describe fails.
    ///
    /// # Panics
    ///
    /// If the tag of `folder` is not a node, which no page of the kind gives.
    async fn level(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> Result<Option<Page>, Error> {
        let folder = folder.map_or(OBJECTS, |folder| {
            folder.tag.parse().unwrap_or_else(|error| {
                panic!(
                    "invariant: the folder {:?} has a node as its tag: {error}",
                    folder.tag
                )
            })
        });
        let before = cursor.map_or(Ok(0), |cursor| {
            cursor.as_str().parse().map_err(|_text| bad_cursor(cursor))
        })?;
        let browsed = self.references(&folder, cursor, before).await?;
        let Some((references, mut seen, next)) = browsed else {
            return Ok(None);
        };
        let after = before + references.len();
        let mut kept = Vec::new();
        for reference in references {
            let tag = tag(&reference);
            if let Some(name) = seen.name(&reference.name, key(&reference), &tag) {
                kept.push((name, tag, reference));
            }
        }
        let references: Vec<&Reference> = kept.iter().map(|(_, _, it)| it).collect();
        let shapes = self.shapes(&references).await.map_err(failed)?;
        let entries = kept.into_iter().zip(shapes);
        let entries = entries.map(|((name, tag, _), shape)| Child { name, tag, shape });
        let mut cursor = None;
        if let Some((point, left)) = next {
            self.uses += 1;
            let held = Held {
                point,
                used: self.uses,
                seen,
                left,
            };
            // A folder that a walk opens twice can give a cursor that it holds.
            if let Some(earlier) = self.points.insert((folder, after), held) {
                self.client
                    .release(&[earlier.point])
                    .await
                    .map_err(failed)?;
            }
            cursor = Some(Cursor::new(after.to_string()));
        }
        Ok(Some(Page {
            entries: entries.collect(),
            next: cursor,
        }))
    }

    /// Reads each node in Read requests that each fit one message of the server. A
    /// node whose Read alone does not fit gives `None`.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when a Read fails.
    ///
    /// # Panics
    ///
    /// If a tag is not a node, which no point of the kind has.
    async fn values(
        &mut self,
        tags: &[&str],
    ) -> Result<Vec<Option<sample::Value>>, Error> {
        let nodes: Vec<Node> = tags
            .iter()
            .map(|tag| {
                tag.parse().unwrap_or_else(|error| {
                    panic!(
                        "invariant: the point {tag:?} has a node as its tag: {error}"
                    )
                })
            })
            .collect();
        let (fit, large) = self.client.nodes(&nodes);
        let read = self.client.read(&fit).await;
        let mut read = read.map_err(|status| device(Failure::Read(status)))?.iter();
        let large: BTreeSet<usize> = large.iter().map(|large| large.index).collect();
        let values = (0..nodes.len()).map(|index| {
            if large.contains(&index) {
                return None;
            }
            let value = read
                .next()
                .expect("invariant: a read gives one value for each node");
            value.ok().map(sample::Value::from)
        });
        Ok(values.collect())
    }
}

/// The key of the node of `reference`, which no other node has: the bytes of its
/// text, which for a [`Node`] are UTF-8 in a form that no [`Foreign`] node has.
fn key(reference: &Reference) -> Vec<u8> {
    match &reference.node {
        Ok(node) => node.to_string().into_bytes(),
        Err(foreign) => foreign.text.clone(),
    }
}

/// The tag of the node of `reference`: the text of a [`Node`], else the text of the
/// foreign node with `%` and each byte that is not UTF-8 as `%XX`, and, for a name,
/// `s%=` in place of `s=`, so that no node that [`Node::from_str`] reads has it.
///
/// [`Node::from_str`]: std::str::FromStr::from_str
fn tag(reference: &Reference) -> String {
    let foreign = match &reference.node {
        Ok(node) => return node.to_string(),
        Err(foreign) => foreign,
    };
    let mut tag = String::new();
    for chunk in foreign.text.utf8_chunks() {
        for character in chunk.valid().chars() {
            match character {
                '%' => tag.push_str("%25"),
                character => tag.push(character),
            }
        }
        for byte in chunk.invalid() {
            write!(tag, "%{byte:02X}").expect("invariant: a write to a String works");
        }
    }
    if foreign.cause != Cause::Name {
        return tag;
    }
    match tag.strip_prefix("s=") {
        Some(name) => format!("s%={name}"),
        None => tag.replacen(";s=", ";s%=", 1),
    }
}

/// The shape of a variable that `description` describes.
fn variable(description: Description) -> Shape {
    if description
        .access
        .is_some_and(|access| access & CURRENT_READ == 0)
    {
        return Shape::Unreadable("the server does not let this user read it".into());
    }
    if description.rank.is_some_and(|rank| rank >= 0) {
        return Shape::Unreadable("an array".into());
    }
    let data_type = match description.base {
        None => None,
        Some(Base::Scalar(scalar)) => Some(Type::Scalar(scalar)),
        Some(Base::Other(name)) => {
            return Shape::Unreadable(format!("a value of the type {name}"));
        }
    };
    let (unit, unit_code) = description
        .unit
        .map_or((None, None), |unit| (unit.text, unit.code));
    Shape::Point(Facts {
        data_type,
        unit,
        unit_code,
        access: description.access.map(|_| Access::Read),
    })
}

/// The reason why the kind cannot read a node of `cause`.
fn reason(cause: Cause) -> &'static str {
    match cause {
        Cause::Key => "its node ID is a GUID or opaque, which the kind cannot read",
        Cause::Name => {
            "its node ID is a name that is empty or not UTF-8, which the kind cannot read"
        }
        Cause::Remote => "it is a node of another server, which the kind cannot read",
    }
}

fn failed(status: Status) -> Error {
    device(Failure::Browse(status))
}

fn bad_cursor(cursor: &Cursor) -> Error {
    Error::Config(vec![Diagnostic::new(
        BAD_CURSOR,
        None,
        format!(
            "the cursor {:?} is not a cursor of the opcua kind",
            cursor.as_str()
        ),
        "Browse from the first page, with no cursor".into(),
    )])
}

#[cfg(test)]
#[cfg(feature = "sim")]
mod tests;
