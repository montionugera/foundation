//! Browses a device as a tree of folders and points: one level, each point whose
//! path matches a pattern, or the entry at each of a list of paths.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::ControlFlow;
use std::str::FromStr;
use std::sync::Arc;

use document::Document;
use document::diagnostic::{Code, Diagnostic};
use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;
use types::sample::Value;
use types::time::Span;

use crate::kind::{self, Error, config_error};
use crate::{deadline, secret, supervisor};

/// The most points that one browse of a pattern gives.
pub const LIMIT: usize = 500;

/// The most entries that one browse looks at: the entries that find its start, and
/// for a pattern or a list of paths each entry of each level that it reads, where a
/// page with no entries counts as one.
pub const VISITS: usize = 10_000;

/// The most that [`Context::time`] waits for the node's first mesh time.
pub const WAIT: Span = Span::from_nanos(5_000_000_000);

/// The most folders that a pattern holds open at one time, its start with them.
pub const DEPTH: usize = 64;

/// The most `*` in the names of a pattern, since each adds a piece that a walk tests in
/// each name.
pub const STARS: usize = 256;

/// One browse's inputs: the connector block as the files give it, and the node's
/// network, clock, randomness, and secrets. It is not `Send`.
#[derive(Debug)]
pub struct Context<'a> {
    config: &'a Document,
    inputs: &'a supervisor::Config,
}

impl<'a> Context<'a> {
    pub(crate) fn new(config: &'a Document, inputs: &'a supervisor::Config) -> Self {
        Self { config, inputs }
    }

    /// The connector block from the files. A kind reads only the attributes that it
    /// needs to reach its device, so a block that `parse` refuses can browse.
    #[must_use]
    pub fn config(&self) -> &Document {
        self.config
    }

    /// The node's clock, as [`kind::Context::clock`].
    #[must_use]
    pub fn clock(&self) -> &Clock {
        &self.inputs.clock
    }

    /// A new random source, as [`kind::Context::rng`].
    #[must_use]
    pub fn rng(&self) -> Rng {
        self.inputs.entropy.rng()
    }

    /// Connects streams and datagrams, as [`kind::Context::net`].
    #[must_use]
    pub fn net(&self) -> &Net {
        &self.inputs.net
    }

    /// Reads the value of `source`, as [`kind::Context::secret`].
    ///
    /// # Errors
    ///
    /// The errors of [`kind::Context::secret`]. Nothing retries a browse: the caller
    /// browses again once the value is in its place.
    pub async fn secret(
        &self,
        source: &secret::Source,
    ) -> Result<secret::Value, Error> {
        kind::read_secret(&self.inputs.secrets, source).await
    }

    /// The node's mesh time. Waits for the node's first mesh time, at most [`WAIT`]
    /// from the call, so a kind that reads no mesh time never waits.
    ///
    /// # Errors
    ///
    /// [`Error::Retry`] with "the node has no mesh time after 5s" when the node has
    /// none at [`WAIT`]. Nothing retries a browse: the caller browses again once the
    /// node has mesh time.
    pub async fn time(&self) -> Result<hub::Time, Error> {
        let late = self.inputs.clock.sleep(WAIT);
        let time = deadline::before(self.inputs.hub.time(), late).await;
        time.ok_or_else(|| {
            Error::Retry(format!("the node has no mesh time after {WAIT}").into())
        })
    }
}

/// A device open for one browse. A kind implements it on its session, so one walk
/// connects once. [`walk`] gives `connector.broken-browse` when the kind gives a page
/// over [`LIMIT`], an entry with an empty name, or a count of values that is not the
/// count of tags.
pub trait Device {
    /// Gives one page of the entries of `folder`, or of the root when `None`, in
    /// device order, from `cursor`, or from the first entry when `None`. A page holds
    /// at most [`LIMIT`] entries. `None` when the device no longer has `folder` as a
    /// folder. A walk may never ask for the page at a `next` that it got, so a kind
    /// bounds the state that it keeps for its cursors in one session.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when the device fails.
    fn level(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> impl Future<Output = Result<Option<Page>, Error>>;

    /// Reads the value of the point of each tag, in the order of `tags`: `None` for a
    /// value that the device did not give, or that is not one fixed-width scalar, such
    /// as the value of a `String` or `Bytes` point. The default gives [`Error::Config`]
    /// with `connector.no-values`: the kind cannot read values.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when the device fails.
    fn values(
        &mut self,
        tags: &[&str],
    ) -> impl Future<Output = Result<Vec<Option<Value>>, Error>> {
        let _: &[&str] = tags;
        std::future::ready(Err(config_error(
            NO_VALUES,
            &"this connector kind cannot read the values of its points",
            "Browse without values",
        )))
    }
}

/// Runs `query` over `device`. It finds the start from the root, one name at a time.
/// A pattern walks depth first, in device order. It browses no folder that no match
/// can be under, and no folder whose tag and pattern state are those of a folder that
/// it holds open, so a loop in the device's graph ends. It ends when it holds
/// [`DEPTH`] folders open and a match can be under one more. Paths browse only the
/// folders that a path is under.
///
/// # Errors
///
/// [`Error::Config`] with `connector.no-folder` when no folder is at a name of the
/// start, or at a folder that the device gave and then did not have, with
/// `connector.no-point` for each path of [`Scope::Points`] at a folder, at no entry,
/// or not under the start, other than a path at a folder or at no entry when the walk
/// ends at [`End::Budget`], and with `connector.broken-browse` when `device` breaks a
/// rule of [`Device`] that its doc says [`walk`] checks.
/// The errors of `device`.
pub async fn walk(device: &mut impl Device, query: &Query) -> Result<Found, Error> {
    walk_within(device, query, LIMIT, VISITS).await
}

/// [`walk`] with `limit` points and `visits` entries.
async fn walk_within<D: Device>(
    device: &mut D,
    query: &Query,
    limit: usize,
    visits: usize,
) -> Result<Found, Error> {
    let mut walker = Walker {
        device,
        seen: 0,
        visits,
    };
    let (entries, end) = match walker.find(&query.start).await? {
        ControlFlow::Break(end) => {
            if let Scope::Points(paths) = &query.scope {
                let names = Names::new(paths, query.start.clone());
                names.found(&vec![None; names.count], true)?;
            }
            (Vec::new(), end)
        }
        ControlFlow::Continue(folder) => match &query.scope {
            Scope::Level(cursor) => {
                let page = walker.page(folder.as_ref(), cursor.as_ref()).await?;
                (page.entries, page.next.map_or(End::Done, End::Next))
            }
            Scope::Match(pattern) => walker.search(folder, pattern, limit).await?,
            Scope::Points(paths) => walker.points(folder, paths).await?,
        },
    };
    let values = match query.values {
        Values::Skipped => Vec::new(),
        Values::Read => read(walker.device, &entries).await?,
    };
    Ok(Found {
        entries,
        values,
        end,
    })
}

/// A device and the count of the entries taken from it.
struct Walker<'a, D> {
    device: &'a mut D,
    seen: usize,
    visits: usize,
}

/// A page of a folder whose entries the walk made from what the kind gave.
struct Level {
    entries: Vec<Entry>,
    next: Option<Cursor>,
}

/// The names after a start of a list of paths, as a tree whose nodes are in one
/// `Vec`, so that no build or drop recurses on a deep path.
struct Names<'a> {
    paths: &'a [Path],
    start: Path,
    /// The first is the start.
    nodes: Vec<Node<'a>>,
    /// The slot of each path, in order: `None` for a path not under the start, one
    /// slot for equal paths.
    slots: Vec<Option<usize>>,
    count: usize,
}

/// A name of [`Names`]: the slot of the entry at it, and the node of each name under
/// it.
#[derive(Default)]
struct Node<'a> {
    at: Option<usize>,
    under: BTreeMap<&'a str, usize>,
}

impl<'a> Names<'a> {
    /// The names of each of `paths` that is under `start`.
    fn new(paths: &'a [Path], start: Path) -> Self {
        let mut names = Self {
            paths,
            start,
            nodes: vec![Node::default()],
            slots: Vec::with_capacity(paths.len()),
            count: 0,
        };
        for path in paths {
            let mut rest = path.names();
            if !names.start.names().all(|name| rest.next() == Some(name)) {
                names.slots.push(None);
                continue;
            }
            let mut node = 0;
            for name in rest {
                let next = names.nodes.len();
                node = *names.nodes[node].under.entry(name).or_insert(next);
                if node == next {
                    names.nodes.push(Node::default());
                }
            }
            let at = *names.nodes[node].at.get_or_insert(names.count);
            names.count = names.count.max(at + 1);
            names.slots.push(Some(at));
        }
        names
    }

    /// The entry in `seen` at the slot of each path, which is a point or an unreadable
    /// entry. A walk that ended at the budget gives what it found, and refuses only a
    /// path not under the start.
    fn found(&self, seen: &[Option<Entry>], budget: bool) -> Result<Vec<Entry>, Error> {
        let mut found = Vec::new();
        let mut problems = Vec::new();
        for (slot, path) in self.slots.iter().zip(self.paths) {
            match slot.map(|at| &seen[at]) {
                Some(Some(entry)) if entry.shape != Shape::Folder => {
                    found.push(entry.clone());
                }
                Some(_) if budget => {}
                Some(_) => problems.push(no_point(&format_args!(
                    "the device has no point at \"{path}\""
                ))),
                None => problems.push(no_point(&format_args!(
                    "the path \"{path}\" is not under the start \"{}\"",
                    self.start
                ))),
            }
        }
        if problems.is_empty() {
            Ok(found)
        } else {
            Err(Error::Config(problems))
        }
    }
}

/// One folder of a walk whose entries are not all seen.
struct Frame {
    folder: Option<Entry>,
    state: State,
    entries: std::vec::IntoIter<Entry>,
    next: Option<Cursor>,
}

impl<D: Device> Walker<'_, D> {
    /// The page of `folder`.
    async fn page(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> Result<Level, Error> {
        let root = Path::root();
        let path = folder.map_or(&root, |folder| &folder.path);
        let page = self
            .device
            .level(folder, cursor)
            .await?
            .ok_or_else(|| no_folder(path))?;
        if page.entries.len() > LIMIT {
            return Err(Broken::Large {
                folder: path.clone(),
                count: page.entries.len(),
            }
            .into());
        }
        let entries = page.entries.into_iter().map(|child| {
            if child.name.is_empty() {
                return Err(Broken::Empty {
                    folder: path.clone(),
                });
            }
            Ok(Entry {
                path: path.child(child.name),
                tag: child.tag,
                shape: child.shape,
            })
        });
        Ok(Level {
            entries: entries.collect::<Result<_, _>>()?,
            next: page.next,
        })
    }

    /// The page of `folder` in a walk, where a page with no entries counts as one
    /// visit, so pages with no entries and a next one end at the budget. It asks the
    /// device for no page past the budget.
    async fn take(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> Result<ControlFlow<End, Level>, Error> {
        if self.seen >= self.visits {
            return Ok(ControlFlow::Break(End::Budget));
        }
        let page = self.page(folder, cursor).await?;
        if page.entries.is_empty() && !self.visit() {
            return Ok(ControlFlow::Break(End::Budget));
        }
        Ok(ControlFlow::Continue(page))
    }

    /// Counts one entry taken from the device: `false` past the budget.
    fn visit(&mut self) -> bool {
        self.seen += 1;
        self.seen <= self.visits
    }

    /// Gives each entry of `folder` to `each`, page by page, until `each` breaks or
    /// the folder ends. It breaks with [`End::Budget`] when the budget ends first.
    async fn scan(
        &mut self,
        folder: Option<&Entry>,
        mut each: impl FnMut(Entry) -> ControlFlow<()>,
    ) -> Result<ControlFlow<End>, Error> {
        let mut cursor = None;
        loop {
            let ControlFlow::Continue(page) =
                self.take(folder, cursor.as_ref()).await?
            else {
                return Ok(ControlFlow::Break(End::Budget));
            };
            for entry in page.entries {
                if !self.visit() {
                    return Ok(ControlFlow::Break(End::Budget));
                }
                if each(entry).is_break() {
                    return Ok(ControlFlow::Continue(()));
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                return Ok(ControlFlow::Continue(()));
            }
        }
    }

    /// The folder at `start`, `None` for the root.
    async fn find(
        &mut self,
        start: &Path,
    ) -> Result<ControlFlow<End, Option<Entry>>, Error> {
        let mut folder = None;
        for at in start.links() {
            let mut child = None;
            let scan = self.scan(folder.as_ref(), |entry| {
                if entry.path.0.as_ref().is_some_and(|e| e.name == at.name) {
                    child = Some(entry);
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(())
            });
            if let ControlFlow::Break(end) = scan.await? {
                return Ok(ControlFlow::Break(end));
            }
            match child {
                Some(entry) if entry.shape == Shape::Folder => folder = Some(entry),
                _ => {
                    return Err(no_folder(&Path(Some(Arc::clone(at)))));
                }
            }
        }
        Ok(ControlFlow::Continue(folder))
    }

    /// Each entry under `folder` that is not a folder and whose path from it matches.
    async fn search(
        &mut self,
        folder: Option<Entry>,
        pattern: &Pattern,
        limit: usize,
    ) -> Result<(Vec<Entry>, End), Error> {
        let mut found = Vec::new();
        let ControlFlow::Continue(first) = self.open(folder, pattern.start()).await?
        else {
            return Ok((found, End::Budget));
        };
        let mut stack = vec![first];
        while let Some(frame) = stack.last_mut() {
            let Some(entry) = frame.entries.next() else {
                match frame.next.take() {
                    Some(cursor) => {
                        let ControlFlow::Continue(page) =
                            self.take(frame.folder.as_ref(), Some(&cursor)).await?
                        else {
                            return Ok((found, End::Budget));
                        };
                        frame.entries = page.entries.into_iter();
                        frame.next = page.next;
                    }
                    None => {
                        stack.pop();
                    }
                }
                continue;
            };
            if !self.visit() {
                return Ok((found, End::Budget));
            }
            let link = entry.path.0.as_ref();
            let link = link.expect("invariant: an entry has a name");
            let state = pattern.step(&frame.state, &link.name);
            if entry.shape == Shape::Folder {
                let looped = stack.iter().any(|frame| {
                    frame.state == state
                        && frame.folder.as_ref().is_some_and(|f| f.tag == entry.tag)
                });
                if pattern.open(&state) && !looped {
                    if stack.len() == DEPTH {
                        return Ok((found, End::Budget));
                    }
                    match self.open(Some(entry), state).await? {
                        ControlFlow::Continue(frame) => stack.push(frame),
                        ControlFlow::Break(end) => return Ok((found, end)),
                    }
                }
            } else if pattern.matched(&state) {
                if found.len() == limit {
                    return Ok((found, End::Limit));
                }
                found.push(entry);
            }
        }
        Ok((found, End::Done))
    }

    /// The entry at each of `paths` under `folder`, in the order of the paths. It
    /// stops a folder when it has seen each name that a path takes in it.
    async fn points(
        &mut self,
        folder: Option<Entry>,
        paths: &[Path],
    ) -> Result<(Vec<Entry>, End), Error> {
        let start = folder.as_ref().map_or_else(Path::root, |f| f.path.clone());
        let names = Names::new(paths, start);
        let mut seen: Vec<Option<Entry>> = vec![None; names.count];
        let mut stack = Vec::new();
        if !names.nodes[0].under.is_empty() {
            stack.push((folder, 0));
        }
        let budget = 'walk: {
            while let Some((folder, node)) = stack.pop() {
                let under = &names.nodes[node].under;
                let mut left = under.len();
                let scan = self.scan(folder.as_ref(), |entry| {
                    let link = entry.path.0.as_ref();
                    let link = link.expect("invariant: an entry has a name");
                    let Some(&at) = under.get(&*link.name) else {
                        return ControlFlow::Continue(());
                    };
                    let child = &names.nodes[at];
                    if entry.shape == Shape::Folder && !child.under.is_empty() {
                        stack.push((Some(entry), at));
                    } else if let Some(slot) = child.at {
                        seen[slot] = Some(entry);
                    }
                    left -= 1;
                    if left == 0 {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                });
                if scan.await?.is_break() {
                    break 'walk true;
                }
            }
            false
        };
        let entries = names.found(&seen, budget)?;
        Ok((entries, if budget { End::Budget } else { End::Done }))
    }

    /// The frame of the first page of `folder`.
    async fn open(
        &mut self,
        folder: Option<Entry>,
        state: State,
    ) -> Result<ControlFlow<End, Frame>, Error> {
        let page = match self.take(folder.as_ref(), None).await? {
            ControlFlow::Continue(page) => page,
            ControlFlow::Break(end) => return Ok(ControlFlow::Break(end)),
        };
        Ok(ControlFlow::Continue(Frame {
            folder,
            state,
            entries: page.entries.into_iter(),
            next: page.next,
        }))
    }
}

/// The error of no folder at `path`.
fn no_folder(path: &Path) -> Error {
    config_error(
        NO_FOLDER,
        &format_args!("the device has no folder at \"{path}\""),
        "Browse the folder above it to see the names in it",
    )
}

/// The problem of a path of [`Scope::Points`].
fn no_point(problem: &dyn fmt::Display) -> Diagnostic {
    Diagnostic::new(
        NO_POINT,
        None,
        problem.to_string(),
        "Give the path of a point that `browse` gives".into(),
    )
}

/// The value of each of `entries`, read in one call.
async fn read(
    device: &mut impl Device,
    entries: &[Entry],
) -> Result<Vec<Option<Value>>, Error> {
    let points: Vec<_> = entries
        .iter()
        .filter(|entry| matches!(entry.shape, Shape::Point(_)))
        .map(|entry| entry.tag.as_str())
        .collect();
    if points.is_empty() {
        return Ok(vec![None; entries.len()]);
    }
    let read = device.values(&points).await?;
    if read.len() != points.len() {
        return Err(Broken::Values {
            asked: points.len(),
            given: read.len(),
        }
        .into());
    }
    let mut read = read.into_iter();
    Ok(entries
        .iter()
        .map(|entry| match entry.shape {
            Shape::Point(_) => read.next().expect("invariant: one value per point"),
            Shape::Folder | Shape::Unreadable(_) => None,
        })
        .collect())
}

const NO_VALUES: Code = Code::new("connector.no-values");
const NO_FOLDER: Code = Code::new("connector.no-folder");
const NO_POINT: Code = Code::new("connector.no-point");
const BROKEN: Code = Code::new("connector.broken-browse");

/// The path of an entry from the root of a device's tree: the name of each entry on
/// the way, its own last. No name of a path is empty. Its text joins the names with
/// `/`, with `\` before each `/`, `*`, and `\` in a name. The root is the empty text.
#[derive(Clone)]
pub struct Path(Option<Arc<Link>>);

/// The last name of a path that is not the root, and the path of its folder.
struct Link {
    folder: Path,
    name: Box<str>,
}

impl Path {
    /// The root, with no name.
    #[must_use]
    pub fn root() -> Self {
        Self(None)
    }

    /// The path of the entry `name` in this folder. It shares this path, so it costs
    /// one link at any depth.
    #[must_use]
    fn child(&self, name: impl Into<Box<str>>) -> Self {
        Self(Some(Arc::new(Link {
            folder: self.clone(),
            name: name.into(),
        })))
    }

    /// The name of each entry from the root, in order.
    #[must_use]
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.links().into_iter().map(|link| &*link.name)
    }

    /// The link of each name from the root, in order.
    fn links(&self) -> Vec<&Arc<Link>> {
        let mut links = Vec::new();
        let mut at = self;
        while let Some(link) = &at.0 {
            links.push(link);
            at = &link.folder;
        }
        links.reverse();
        links
    }
}

impl PartialEq for Path {
    fn eq(&self, other: &Self) -> bool {
        let (mut a, mut b) = (self, other);
        loop {
            match (&a.0, &b.0) {
                (Some(x), Some(y)) if x.name == y.name => {
                    (a, b) = (&x.folder, &y.folder);
                }
                (None, None) => return true,
                _ => return false,
            }
        }
    }
}

impl Eq for Path {}

impl Drop for Path {
    /// Drops the links that only this path holds one at a time, so a deep path does
    /// not drop by recursion.
    fn drop(&mut self) {
        let mut next = self.0.take();
        while let Some(link) = next {
            next = Arc::into_inner(link).and_then(|mut link| link.folder.0.take());
        }
    }
}

impl FromStr for Path {
    type Err = Malformed;

    /// Reads the text of a path. A `*` is a name's own character.
    fn from_str(text: &str) -> Result<Self, Malformed> {
        if text.is_empty() {
            return Ok(Self::root());
        }
        Ok(split(text)?.into_iter().fold(Self::root(), |path, name| {
            path.child(name.into_iter().map(Char::char).collect::<String>())
        }))
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for name in self.names() {
            f.write_str(separator)?;
            name.chars().try_for_each(|c| escape(f, c))?;
            separator = "/";
        }
        Ok(())
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_string())
    }
}

/// A pattern of paths: `/` splits names, `**` alone between two `/` matches any
/// number of names (none too), `*` matches any text in one name, and `\` takes the
/// next character as it is. A run of `**` names is one, and a pattern holds at most
/// [`DEPTH`] other names. A run of `*` in a name is one, and a pattern holds at most
/// [`STARS`] `*` in its names other than `**`.
#[derive(Clone, PartialEq, Eq)]
pub struct Pattern {
    segments: Vec<Segment>,
}

/// What one segment of a [`Pattern`] matches. No two `Any` are next to each other.
#[derive(Clone, PartialEq, Eq)]
enum Segment {
    /// Any number of names.
    Any,
    /// One name.
    Name(Glob),
}

/// The text of one name of a pattern, split at each `*`. With one piece, a name
/// matches when it is that piece. Else it matches when it holds at least the bytes of
/// all pieces, starts with the first, ends with the last, and holds the pieces between
/// in order. Each piece between the first and the last is not empty.
#[derive(Clone, PartialEq, Eq)]
struct Glob {
    pieces: Vec<String>,
    /// The bytes of all pieces.
    len: usize,
}

impl Glob {
    fn new(pieces: Vec<String>) -> Self {
        let len = pieces.iter().map(String::len).sum();
        Self { pieces, len }
    }

    /// Whether `name` matches, in time linear in the length of `name`.
    fn matches(&self, name: &str) -> bool {
        let [first, middle @ .., last] = self.pieces.as_slice() else {
            return self.pieces.first().is_some_and(|piece| piece == name);
        };
        // So that the first and the last piece do not overlap in `name`, and so that
        // `middle`, of pieces that are not empty, holds no more pieces than `name` has
        // bytes.
        if name.len() < self.len
            || !name.starts_with(first.as_str())
            || !name.ends_with(last.as_str())
        {
            return false;
        }
        // With `*` the only wildcard, the leftmost place of each piece is right.
        let mut rest = &name[first.len()..name.len() - last.len()];
        for piece in middle {
            match rest.find(piece.as_str()) {
                Some(at) => rest = &rest[at + piece.len()..],
                None => return false,
            }
        }
        true
    }
}

/// The positions in a pattern's segments that a prefix of a path reaches, in order,
/// with no repeat.
#[derive(Clone, Debug, PartialEq, Eq)]
struct State(Vec<usize>);

impl Pattern {
    /// The state of the empty path.
    fn start(&self) -> State {
        self.close([0])
    }

    /// The state after `name` from `state`.
    fn step(&self, state: &State, name: &str) -> State {
        self.close(
            state
                .0
                .iter()
                .filter_map(|&at| match self.segments.get(at) {
                    Some(Segment::Any) => Some(at),
                    Some(Segment::Name(glob)) if glob.matches(name) => Some(at + 1),
                    Some(Segment::Name(_)) | None => None,
                }),
        )
    }

    /// Whether the path of `state` matches.
    fn matched(&self, state: &State) -> bool {
        state.0.last() == Some(&self.segments.len())
    }

    /// Whether a longer path than that of `state` can match.
    fn open(&self, state: &State) -> bool {
        state.0.first().is_some_and(|&at| at < self.segments.len())
    }

    /// `positions` and each one after an `Any` in it, since `Any` matches no name too.
    /// `positions` is in order, so the state is too, in time linear in its count. No
    /// two `Any` segments are next to each other, so `at + 1` is the only position
    /// that `at` adds.
    fn close(&self, positions: impl IntoIterator<Item = usize>) -> State {
        let mut reached: Vec<usize> = Vec::new();
        for at in positions {
            let any = self.segments.get(at) == Some(&Segment::Any);
            for at in [Some(at), any.then_some(at + 1)].into_iter().flatten() {
                if reached.last().is_none_or(|&last| last < at) {
                    reached.push(at);
                }
            }
        }
        State(reached)
    }
}

impl FromStr for Pattern {
    type Err = Malformed;

    /// Reads a pattern. A run of `**` names is one, and a run of `*` in a name is one.
    ///
    /// # Errors
    ///
    /// [`Malformed`] when the text is not a pattern, holds more than [`DEPTH`] names
    /// other than `**`, or more than [`STARS`] `*` in those names.
    fn from_str(text: &str) -> Result<Self, Malformed> {
        let mut segments: Vec<Segment> =
            split(text)?.into_iter().map(segment).collect();
        segments.dedup_by(|a, b| *a == Segment::Any && *b == Segment::Any);
        let globs = segments.iter().filter_map(|segment| match segment {
            Segment::Any => None,
            Segment::Name(glob) => Some(glob),
        });
        if globs.clone().count() > DEPTH {
            return Err(Malformed::Deep);
        }
        if globs.map(|glob| glob.pieces.len() - 1).sum::<usize>() > STARS {
            return Err(Malformed::Stars);
        }
        Ok(Self { segments })
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for segment in &self.segments {
            f.write_str(separator)?;
            match segment {
                Segment::Any => f.write_str("**")?,
                Segment::Name(Glob { pieces, .. }) => {
                    let mut star = "";
                    for piece in pieces {
                        f.write_str(star)?;
                        for c in piece.chars() {
                            escape(f, c)?;
                        }
                        star = "*";
                    }
                }
            }
            separator = "/";
        }
        Ok(())
    }
}

impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_string())
    }
}

/// One character of the text of a path or a pattern.
#[derive(Clone, Copy)]
enum Char {
    Plain(char),
    Escaped(char),
    Star,
}

impl Char {
    /// The character of a path's name, in which a `*` is its own.
    fn char(self) -> char {
        match self {
            Self::Plain(c) | Self::Escaped(c) => c,
            Self::Star => '*',
        }
    }
}

/// The segment of the text of one name of a pattern.
fn segment(name: Vec<Char>) -> Segment {
    if let [Char::Star, Char::Star] = name.as_slice() {
        return Segment::Any;
    }
    let mut pieces = vec![String::new()];
    for c in name {
        match c {
            // A run of `*` is one.
            Char::Star
                if pieces.len() > 1 && pieces.last().is_some_and(String::is_empty) => {}
            Char::Star => pieces.push(String::new()),
            Char::Plain(c) | Char::Escaped(c) => pieces
                .last_mut()
                .expect("invariant: one piece at the least")
                .push(c),
        }
    }
    Segment::Name(Glob::new(pieces))
}

/// The names of `text`, split on each `/` that no `\` takes.
fn split(text: &str) -> Result<Vec<Vec<Char>>, Malformed> {
    let mut names = vec![Vec::new()];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let name = names.last_mut().expect("invariant: one name at the least");
        match c {
            '\\' => name.push(Char::Escaped(chars.next().ok_or(Malformed::Escape)?)),
            '*' => name.push(Char::Star),
            '/' => names.push(Vec::new()),
            c => name.push(Char::Plain(c)),
        }
    }
    if names.iter().any(Vec::is_empty) {
        return Err(Malformed::Empty);
    }
    Ok(names)
}

/// Writes `c` as the text of a name.
fn escape(f: &mut fmt::Formatter<'_>, c: char) -> fmt::Result {
    if matches!(c, '/' | '*' | '\\') {
        f.write_str("\\")?;
    }
    write!(f, "{c}")
}

/// Text that is not a path or a pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// A name is empty, as in `a//b` or with a `/` at an end. A pattern needs one
    /// name at the least.
    Empty,
    /// The text ends in a `\` that takes no character.
    Escape,
    /// A pattern holds more than [`DEPTH`] names other than `**`, so a walk can match
    /// no path. Only a pattern gives it.
    Deep,
    /// A pattern holds more than [`STARS`] `*` in its names other than `**`, a run of
    /// `*` as one. Only a pattern gives it.
    Stars,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("a name in it is empty"),
            Self::Escape => f.write_str("it ends in a `\\` that takes no character"),
            Self::Deep => write!(
                f,
                "it has more than {DEPTH} names, and a browse finds no path that long"
            ),
            Self::Stars => write!(
                f,
                "it has more than {STARS} `*`, and each costs a search of each name"
            ),
        }
    }
}

impl std::error::Error for Malformed {}

/// A break of a rule of [`Device`] by the code of a kind.
#[derive(Debug)]
enum Broken {
    /// A page of `folder` held `count` entries, over [`LIMIT`].
    Large { folder: Path, count: usize },
    /// A page of `folder` held an entry whose name is empty.
    Empty { folder: Path },
    /// The device gave `given` values for `asked` tags.
    Values { asked: usize, given: usize },
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the connector kind broke a rule of a browse: ")?;
        match self {
            Self::Large { folder, count } => write!(
                f,
                "a page holds at most {LIMIT} entries, and a page of \"{folder}\" held \
                 {count}",
            ),
            Self::Empty { folder } => write!(
                f,
                "an entry has a name, and an entry of \"{folder}\" has an empty name",
            ),
            Self::Values { asked, given } => write!(
                f,
                "one value comes for each tag, and {given} came for {asked} tags",
            ),
        }
    }
}

impl From<Broken> for Error {
    fn from(broken: Broken) -> Self {
        config_error(
            BROKEN,
            &broken,
            "Report this defect of the connector kind with this message",
        )
    }
}

/// One entry of a device's tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Where it is.
    pub path: Path,
    /// Its address in the device, in the form of the kind: an OPC UA node ID, a
    /// register. Two entries with the same tag are one entry of the device.
    pub tag: String,
    /// What it is.
    pub shape: Shape,
}

/// An entry of a folder as a kind gives it in a [`Page`]. [`walk`] gives it as an
/// [`Entry`] whose path is the path of the folder and `name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    /// Its name in the folder. It is not empty: the record of the kind says what it
    /// gives for an empty name of the device. No two entries of one folder have the
    /// same name. [`walk`] does not check this: one level gives one page, so it never
    /// holds each name of a folder.
    pub name: String,
    /// Its address in the device, as [`Entry::tag`].
    pub tag: String,
    /// What it is.
    pub shape: Shape,
}

/// What an entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// It holds other entries.
    Folder,
    /// A point that a connector of the kind can read or write.
    Point(Facts),
    /// A point that a connector of the kind cannot read or write, with the reason,
    /// such as "an array".
    Unreadable(String),
}

/// What the device knows of a point. `None` is a fact that the device cannot know.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The sample type.
    pub data_type: Option<types::sample::Type>,
    /// The unit, as the device gives it.
    pub unit: Option<String>,
    /// The UN/CEFACT Recommendation 20 common code of the unit, such as `CEL`.
    pub unit_code: Option<String>,
    /// Whether a connector can read it, write it, or both.
    pub access: Option<Access>,
}

/// Whether a connector can read a point, write it, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// A connector can read it.
    Read,
    /// A connector can write it.
    Write,
    /// A connector can read it and write it.
    ReadWrite,
}

/// One page of the entries of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The entries, in device order.
    pub entries: Vec<Child>,
    /// Where the next page starts, or `None` after the last.
    pub next: Option<Cursor>,
}

/// Where the next page of a folder starts, in the form of the kind. It holds no
/// session state, so a later browse takes it. In one session, a kind may also keep a
/// faster way on from a cursor that it gave, such as an OPC UA continuation point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(String);

impl Cursor {
    /// Wraps the text of a kind's cursor.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// Its text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a browse looks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The folder where it starts.
    pub start: Path,
    /// One level of `start`, the points under it that match a pattern, or the entries
    /// at paths under it.
    pub scope: Scope,
    /// Whether it reads the value of each point that it gives.
    pub values: Values,
}

/// What a browse gives of its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The page of its entries from the cursor, or the first page when `None`.
    Level(Option<Cursor>),
    /// Each entry under it that is not a folder and whose path from it matches.
    Match(Pattern),
    /// The entry at each path, in the order of the paths: a point or an unreadable
    /// entry. Each path is from the root, as an [`Entry`] gives it, and under the
    /// start. The walk reads the folder at each prefix of the paths at most once.
    Points(Vec<Path>),
}

/// Whether a browse reads values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Values {
    /// It reads none.
    Skipped,
    /// It reads the value of each point that it gives, in one call. A kind that cannot
    /// read values fails only when the browse gives a point.
    Read,
}

/// What a browse found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The entries, in device order: at most [`LIMIT`] for [`Scope::Match`]. In the
    /// order of the paths for [`Scope::Points`].
    pub entries: Vec<Entry>,
    /// With [`Values::Read`], the value of each entry, in the order of `entries`:
    /// `None` for a folder, an unreadable point, or a value that the device did not
    /// give or that is not one fixed-width scalar. Empty with [`Values::Skipped`].
    pub values: Vec<Option<Value>>,
    /// Why it stopped.
    pub end: End,
}

/// Why a browse stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// It gave each entry that it looks for.
    Done,
    /// The next page of the level starts at the cursor.
    Next(Cursor),
    /// More than [`LIMIT`] points match.
    Limit,
    /// It looked at [`VISITS`] entries, where each entry of each level that it reads
    /// counts, before it found its start, saw each entry under it, or found the entry
    /// at each path of [`Scope::Points`]. Or it held [`DEPTH`] folders open and met
    /// one more that a match can be under. So more may match.
    Budget,
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests;
