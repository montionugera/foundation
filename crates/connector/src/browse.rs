//! Browses a device as a tree of folders and points: one level, or each point whose
//! path matches a pattern.

use std::fmt;
use std::ops::ControlFlow;
use std::str::FromStr;

use document::diagnostic::Code;
use types::sample::Value;

use crate::kind::{Error, config_error};

/// The most points that one browse of a pattern gives.
pub const LIMIT: usize = 500;

/// The most entries that one browse takes from the device: the entries that find its
/// start, and for a pattern each entry that it walks, where a page with no entries
/// counts as one.
pub const VISITS: usize = 10_000;

/// The most names in the path of a folder that a pattern opens.
pub const DEPTH: usize = 64;

/// A device open for one browse. A kind implements it on its session, so one walk
/// connects once.
pub trait Device {
    /// Gives one page of the entries of `folder`, or of the root when `None`, in
    /// device order, from `cursor`, or from the first entry when `None`. A page holds
    /// at most [`LIMIT`] entries. The path of each entry is the path of `folder` and
    /// its name, which is not empty: a kind names an entry whose name in the device
    /// is empty, such as by its tag. No two entries of one folder have the same name.
    /// `None` when the device no longer has `folder` as a folder.
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
/// can be under, and no folder whose tag is the tag of a folder above it, so a loop
/// in the device's graph ends. It ends at a folder deeper than [`DEPTH`].
///
/// # Errors
///
/// [`Error::Config`] with `connector.no-folder` when no folder is at a name of the
/// start, or at a folder that the device gave and then did not have. The errors of
/// `device`.
///
/// # Panics
///
/// When `device` gives a page of more than [`LIMIT`] entries, an entry that is not in
/// the folder it browsed, or a count of values that is not the count of tags.
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
        ControlFlow::Break(end) => (Vec::new(), end),
        ControlFlow::Continue(folder) => match &query.scope {
            Scope::Level(cursor) => {
                let page = walker.page(folder.as_ref(), cursor.as_ref()).await?;
                (page.entries, page.next.map_or(End::Done, End::Next))
            }
            Scope::Match(pattern) => walker.search(folder, pattern, limit).await?,
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
    ) -> Result<Page, Error> {
        let root = Path::root();
        let path = folder.map_or(&root, |folder| &folder.path);
        let page = self
            .device
            .level(folder, cursor)
            .await?
            .ok_or_else(|| no_folder(path))?;
        assert!(
            page.entries.len() <= LIMIT,
            "invariant: the device gave {} entries at {path:?}, over {LIMIT}",
            page.entries.len(),
        );
        for entry in &page.entries {
            let parent = entry.path.names.split_last().map(|(_, parent)| parent);
            assert!(
                parent == Some(&path.names[..]),
                "invariant: the device gave {:?} in the folder {path:?}",
                entry.path,
            );
        }
        Ok(page)
    }

    /// The page of `folder` in a walk, where a page with no entries counts as one
    /// visit, so pages with no entries and a next one end at the budget.
    async fn take(
        &mut self,
        folder: Option<&Entry>,
        cursor: Option<&Cursor>,
    ) -> Result<ControlFlow<End, Page>, Error> {
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

    /// The folder at `start`, `None` for the root.
    async fn find(
        &mut self,
        start: &Path,
    ) -> Result<ControlFlow<End, Option<Entry>>, Error> {
        let mut folder = None;
        for (i, name) in start.names.iter().enumerate() {
            let mut cursor = None;
            let child = 'pages: loop {
                let ControlFlow::Continue(page) =
                    self.take(folder.as_ref(), cursor.as_ref()).await?
                else {
                    return Ok(ControlFlow::Break(End::Budget));
                };
                for entry in page.entries {
                    if !self.visit() {
                        return Ok(ControlFlow::Break(End::Budget));
                    }
                    if entry.path.names.last() == Some(name) {
                        break 'pages Some(entry);
                    }
                }
                cursor = page.next;
                if cursor.is_none() {
                    break None;
                }
            };
            match child {
                Some(entry) if entry.shape == Shape::Folder => folder = Some(entry),
                _ => {
                    let names = start.names[..=i].to_vec();
                    return Err(no_folder(&Path { names }));
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
            let name = entry
                .path
                .names
                .last()
                .expect("invariant: an entry has a name");
            let state = pattern.step(&frame.state, name);
            if entry.shape == Shape::Folder {
                let looped = stack
                    .iter()
                    .filter_map(|frame| frame.folder.as_ref())
                    .any(|folder| folder.tag == entry.tag);
                if pattern.open(&state) && !looped {
                    if entry.path.names.len() > DEPTH {
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
    assert_eq!(
        read.len(),
        points.len(),
        "invariant: the device gave {} values for {} tags",
        read.len(),
        points.len(),
    );
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

/// The path of an entry from the root of a device's tree: the name of each entry on
/// the way, its own last, and no name is empty. Its text joins the names with `/`,
/// with `\` before each `/`, `*`, and `\` in a name. The root is the empty text.
#[derive(Clone, PartialEq, Eq)]
pub struct Path {
    names: Vec<String>,
}

impl Path {
    /// The root, with no name.
    #[must_use]
    pub fn root() -> Self {
        Self { names: Vec::new() }
    }

    /// The path of the entry `name` in this folder.
    ///
    /// # Panics
    ///
    /// When `name` is empty: no text names it.
    #[must_use]
    pub fn child(&self, name: impl Into<String>) -> Self {
        let name = name.into();
        assert!(!name.is_empty(), "invariant: a name is not empty");
        let mut names = self.names.clone();
        names.push(name);
        Self { names }
    }

    /// The name of each entry from the root, in order.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

impl FromStr for Path {
    type Err = Malformed;

    /// Reads the text of a path. A `*` is a name's own character.
    fn from_str(text: &str) -> Result<Self, Malformed> {
        if text.is_empty() {
            return Ok(Self::root());
        }
        let names = split(text)?
            .into_iter()
            .map(|name| name.into_iter().map(Char::char).collect())
            .collect();
        Ok(Self { names })
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for name in &self.names {
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
/// next character as it is.
#[derive(Clone, PartialEq, Eq)]
pub struct Pattern {
    segments: Vec<Segment>,
}

/// What one segment of a [`Pattern`] matches.
#[derive(Clone, PartialEq, Eq)]
enum Segment {
    /// Any number of names.
    Any,
    /// One name, with each `None` as any text. No two `None` are next to each other.
    Name(Vec<Option<char>>),
}

/// The positions in a pattern's segments that a prefix of a path reaches, in order,
/// with no repeat.
#[derive(Clone, Debug)]
struct State(Vec<usize>);

impl Pattern {
    /// The state of the empty path.
    fn start(&self) -> State {
        self.close(vec![0])
    }

    /// The state after `name` from `state`.
    fn step(&self, state: &State, name: &str) -> State {
        let name: Vec<char> = name.chars().collect();
        let mut next = Vec::new();
        for &at in &state.0 {
            match self.segments.get(at) {
                Some(Segment::Any) => next.push(at),
                Some(Segment::Name(glob)) if glob_matches(glob, &name) => {
                    next.push(at + 1);
                }
                Some(Segment::Name(_)) | None => {}
            }
        }
        next.sort_unstable();
        next.dedup();
        self.close(next)
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
    fn close(&self, mut positions: Vec<usize>) -> State {
        let mut i = 0;
        while let Some(&at) = positions.get(i) {
            let after = at + 1;
            if self.segments.get(at) == Some(&Segment::Any)
                && !positions.contains(&after)
            {
                positions.push(after);
            }
            i += 1;
        }
        positions.sort_unstable();
        State(positions)
    }
}

/// Whether `name` matches `glob`, in which `None` is any text.
fn glob_matches(glob: &[Option<char>], name: &[char]) -> bool {
    let (mut g, mut n) = (0, 0);
    // Past the last `None`, and where in `name` its text ends now.
    let mut star = None;
    while n < name.len() {
        match glob.get(g) {
            Some(None) => {
                g += 1;
                star = Some((g, n));
            }
            Some(Some(c)) if *c == name[n] => {
                g += 1;
                n += 1;
            }
            _ => match &mut star {
                Some((after, end)) => {
                    *end += 1;
                    g = *after;
                    n = *end;
                }
                None => return false,
            },
        }
    }
    glob[g..].iter().all(Option::is_none)
}

impl FromStr for Pattern {
    type Err = Malformed;

    fn from_str(text: &str) -> Result<Self, Malformed> {
        let segments = split(text)?.into_iter().map(segment).collect();
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
                Segment::Name(glob) => {
                    for c in glob {
                        match c {
                            None => f.write_str("*")?,
                            Some(c) => escape(f, *c)?,
                        }
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
    let mut glob: Vec<Option<char>> = Vec::new();
    for c in name {
        let c = match c {
            Char::Star if glob.last() == Some(&None) => continue,
            Char::Star => None,
            Char::Plain(c) | Char::Escaped(c) => Some(c),
        };
        glob.push(c);
    }
    Segment::Name(glob)
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
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("a name in it is empty"),
            Self::Escape => f.write_str("it ends in a `\\` that takes no character"),
        }
    }
}

impl std::error::Error for Malformed {}

/// One entry of a device's tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Where it is.
    pub path: Path,
    /// Its address in the device, in the form of the kind: an OPC UA node ID, a
    /// register.
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
    pub entries: Vec<Entry>,
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
    /// One level of `start`, or the points under it that match a pattern.
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
    /// The entries, in device order: at most [`LIMIT`] for [`Scope::Match`].
    pub entries: Vec<Entry>,
    /// With [`Values::Read`], the value of each entry, in the order of `entries`:
    /// `None` for a folder, an unreadable point, or a value that the device did not
    /// give. Empty with [`Values::Skipped`].
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
    /// It took [`VISITS`] entries from the device before it found its start or saw
    /// each entry under it, or met a folder deeper than [`DEPTH`] that a match can be
    /// under, so more may match.
    Budget,
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests;
