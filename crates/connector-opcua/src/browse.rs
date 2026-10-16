//! The browse of the `opcua` kind: the tree of a server from its Objects folder, as
//! [`browse::walk`] walks it.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use connector::browse::{self, Access, Child, Cursor, Entry, Facts, Page, Shape};
use connector::kind::Error;
use document::diagnostic::{Code, Diagnostic};
use types::sample::{self, Type};

use crate::client::Client;
use crate::client::browse::{Cause, Class, Description, Point, Reference};
use crate::ffi::{self, Status};
use crate::kind::{Failure, device};
use crate::node::{Key, Node};
use crate::value::SCALARS;

const BAD_CURSOR: Code = Code::new("opcua.bad-cursor");

/// The Objects folder, the root of a browse.
const OBJECTS: Node = Node {
    namespace: 0,
    key: Key::Number(85),
};

/// The bit `CurrentRead` of a `UserAccessLevel`.
const CURRENT_READ: u8 = 1;

/// A server open for one browse, on the session of a client.
pub(crate) struct Server<'a> {
    client: &'a mut Client,
    /// The continuation point of each page that a cursor of this browse starts, by
    /// its folder and the count of references of the folder before it, with what the
    /// references before it gave.
    points: BTreeMap<(Node, usize), (Point, Seen)>,
}

/// The tags and names that the references of one folder gave before a page.
#[derive(Default)]
struct Seen {
    tags: BTreeSet<String>,
    names: BTreeSet<String>,
}

impl<'a> Server<'a> {
    /// Opens a browse on the session of `client`.
    pub(crate) fn new(client: &'a mut Client) -> Self {
        Self {
            client,
            points: BTreeMap::new(),
        }
    }

    /// Gives the references of `folder` from the count `before`, the continuation
    /// point after them, and what the references before them gave. A point of this
    /// browse goes on with `BrowseNext`. Else, or when the server no longer holds the
    /// point, it browses `folder` again from its start and skips `before` references.
    /// `None` when the server does not have `folder`.
    async fn references(
        &mut self,
        folder: &Node,
        before: usize,
    ) -> Result<Option<(Vec<Reference>, Option<Point>, Seen)>, Status> {
        if let Some((point, seen)) = self.points.remove(&(folder.clone(), before)) {
            match self.client.browse_next(point).await {
                Ok(page) => return Ok(Some((page.references, page.next, seen))),
                Err(Status::BAD_CONTINUATION_POINT_INVALID) => {}
                Err(status) => return Err(status),
            }
        }
        let most = u32::try_from(browse::LIMIT).ok().and_then(NonZeroU32::new);
        let most = most.expect("invariant: `LIMIT` is a u32 that is not 0");
        let Some(mut page) = self.client.browse(folder, most).await? else {
            return Ok(None);
        };
        let (mut seen, mut left) = (Seen::default(), before);
        loop {
            let skipped = left.min(page.references.len());
            let rest = page.references.split_off(skipped);
            for reference in &page.references {
                seen.name(&reference.name, &tag(reference));
            }
            match page.next {
                Some(point) if rest.is_empty() => {
                    left -= skipped;
                    page = self.client.browse_next(point).await?;
                }
                next => return Ok(Some((rest, next, seen))),
            }
        }
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
        let shapes = references.iter().map(|reference| match &reference.node {
            Err(foreign) => Shape::Unreadable(reason(foreign.cause).into()),
            Ok(_) if reference.class != Class::Variable => Shape::Folder,
            Ok(_) => variable(descriptions.next().expect("one for each variable")),
        });
        Ok(shapes.collect())
    }
}

impl Seen {
    /// Gives the name of a reference of the browse name `name` to the node `tag`:
    /// `name`, or `tag` when `name` is empty, with ` (<tag>)` after it for each time
    /// that an earlier name has it. `None` when an earlier reference gave the node.
    fn name(&mut self, name: &str, tag: &str) -> Option<String> {
        if !self.tags.insert(tag.to_owned()) {
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
    /// its page.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `opcua.bad-cursor` when the text of `cursor` is not a
    /// count. [`Error::Device`] when a Browse, a `BrowseNext`, or a describe fails.
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
            folder
                .tag
                .parse()
                .expect("invariant: a folder has a node as its tag")
        });
        let before = cursor.map_or(Ok(0), |cursor| {
            cursor.as_str().parse().map_err(|_text| bad_cursor(cursor))
        })?;
        let browsed = self.references(&folder, before).await;
        let Some((references, point, mut seen)) = browsed.map_err(failed)? else {
            return Ok(None);
        };
        let after = before + references.len();
        let mut kept = Vec::new();
        for reference in references {
            let tag = tag(&reference);
            if let Some(name) = seen.name(&reference.name, &tag) {
                kept.push((name, tag, reference));
            }
        }
        let references: Vec<&Reference> = kept.iter().map(|(_, _, it)| it).collect();
        let shapes = self.shapes(&references).await.map_err(failed)?;
        let entries = kept.into_iter().zip(shapes);
        let entries = entries.map(|((name, tag, _), shape)| Child { name, tag, shape });
        let next = point.map(|point| {
            self.points.insert((folder, after), (point, seen));
            Cursor::new(after.to_string())
        });
        Ok(Some(Page {
            entries: entries.collect(),
            next,
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
                tag.parse()
                    .expect("invariant: a point has a node as its tag")
            })
            .collect();
        let mut values = vec![None; nodes.len()];
        let mut asked: Vec<usize> = (0..nodes.len()).collect();
        while !asked.is_empty() {
            let some: Vec<Node> =
                asked.iter().map(|&index| nodes[index].clone()).collect();
            match self.client.nodes(&some) {
                Err(large) => {
                    asked.remove(large.index);
                }
                Ok(split) => {
                    let read = self.client.read(&split).await;
                    let read = read.map_err(|status| device(Failure::Read(status)))?;
                    for (&index, value) in asked.iter().zip(read) {
                        values[index] = value.ok().map(sample::Value::from);
                    }
                    break;
                }
            }
        }
        Ok(values)
    }
}

/// The tag of the node of `reference`.
fn tag(reference: &Reference) -> String {
    match &reference.node {
        Ok(node) => node.to_string(),
        Err(foreign) => foreign.text.clone(),
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
        // A base of 1 to 11 is a scalar of a read, at `id - 1` in `SCALARS`.
        Some(id @ 1..=11) => Some(Type::Scalar(SCALARS[id as usize - 1])),
        // Each other base of `Client::describe` is at `id - 1` in `UA_TYPES`, as
        // `shim.c` asserts.
        Some(id) => {
            let name = ffi::types()[id as usize - 1].name();
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
