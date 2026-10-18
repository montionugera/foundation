//! What an import writes into the files: the blocks of each pick, and the `channel`
//! blocks, so that a kind never writes one.
//!
//! [`add`] takes the file of the connector block and only adds to it. A pick is present
//! when a block of the connector body, or an earlier pick that adds a channel, has its
//! tag as [`Table::tag`] gives it, so the kind compares tags, not text. A present pick
//! takes no name. The other picks take their names in this order: each given name, then
//! the index, then the defaults of [`::connector::import::names`]. The index is that of
//! the first channel that the connector writes whose `channel` block names one. Else it
//! is the new index `<connector>.time`, or the first free of `_2`, `_3`, and on, which
//! the import adds only when a pick adds a channel. The blocks that [`Table::block`]
//! gives go after the last item of the connector body, in the order of the picks. Then
//! the new index and the `channel` block of each pick go at the end of the file.

use std::collections::{BTreeMap, BTreeSet};

use ::connector::browse::{Facts, Path};
use ::connector::import::{self, Fill, fill};
use ::connector::kind::Table;
use document::diagnostic::Diagnostic;
use document::encoding::Checked;
use document::value::{Kind, Value};
use document::{Attribute, Block, Document, Label, Map, Source, read};
use spec::data_type::DataType;
use spec::definition;
use spec::unit::Unit;
use types::name::Name;

use crate::duplicate::DUPLICATE_NAME;
use crate::walk::{self, Part};

/// One point that an import adds: its path and tag as a browse gave them, its facts,
/// and the name that the import gives it, if any.
#[derive(Clone, Copy, Debug)]
pub struct Pick<'a> {
    /// The path of the point, under the start of the browse.
    pub path: &'a Path,
    /// The tag of the point.
    pub tag: &'a str,
    /// The facts of the point.
    pub facts: &'a Facts,
    /// The name that the user gives the point, if any.
    pub name: Option<&'a Name>,
}

/// What an import did with one pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It added the channel `name`. `fills` holds the text of each `fill` in the
    /// blocks of the pick.
    Added {
        /// The name of the channel.
        name: Name,
        /// The text of each `fill`.
        fills: Vec<String>,
    },
    /// Its default name passes `Name::MAX_BYTES`, so it added nothing.
    Unnamed,
    /// The connector block or an earlier pick reads or writes its point already.
    Present,
}

/// The file of the connector block, with the blocks of an import added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    /// The source of the file that holds the connector block: `Source(i)` for
    /// `documents[i]` of [`add`].
    pub source: Source,
    /// The document of that file with the new blocks.
    pub document: Checked,
    /// The index channel that the import adds: `None` when a channel that the
    /// connector writes names an index, or when no pick adds a channel.
    pub index: Option<Name>,
    /// What the import did with each pick, in the order of the picks.
    pub picks: Vec<Outcome>,
}

/// Adds the blocks of `picks` of the connector `connector` of `kinds` to the file of
/// the connector block in `documents`, as the module doc gives them. `start` is the
/// start of the browse. It gives no problem of a block outside the connector block
/// other than `config.private-key`.
///
/// # Errors
///
/// The problems of [`config::connector`](crate::connector), then those of the check of
/// the connector's kind, and `config.long-name` when a status name of the kind is too
/// long. Else the diagnostic of an `index` that is not a name, in the `channel` block
/// of the first channel that the connector writes that has one. Else, for each given
/// name of a pick that is not present, `config.reserved-name` for a segment that starts
/// with `@`, `config.duplicate-name` for a name that the files define or imply or that
/// a connector writes, in any ASCII case, and one `config.duplicate-name` that names
/// both paths for a name that an earlier pick has. Each names the path and the name,
/// with the fix "Give another name, or leave it out for `<default>`", with the default
/// of the run with each refused name left out, or "Give another name" when that run
/// gives none or refuses a name. Else the diagnostics of [`Table::block`]. Else `document.too-deep`.
///
/// # Panics
///
/// When a pick that is not present, with no given name or a refused one, has a path
/// that is not under `start`, as [`import::names`] does, or when a kind of `kinds`
/// writes a call `fill` of other arguments.
pub fn add(
    documents: &[Document],
    connector: &Name,
    start: &Path,
    picks: &[Pick<'_>],
    kinds: &Table,
) -> Result<Added, Vec<Diagnostic>> {
    let (block, found) = crate::connector::block(documents, connector)?;
    let kind = found.kind().as_str();
    let body = found.config().document();
    let label = block.labels.first().and_then(|label| label.span);
    let channels = kinds.check(kind, crate::span(block, "kind"), body)?;
    let writes = crate::connector::writes(connector, channels, label)
        .map_err(|problem| vec![problem])?;
    let mut tags = BTreeSet::new();
    for item in &body.blocks {
        tags.extend(kinds.tag(kind, item)?);
    }
    let blocks = crate::blocks(documents);
    let mut taken = taken(&blocks, connector, kinds);
    taken.extend(writes.implied_names().cloned());
    taken.extend(writes.channels().cloned());
    let existing =
        index_of(&blocks, writes.channels()).map_err(|problem| vec![problem])?;
    let (open, index, names) =
        named(connector, start, picks, &tags, &taken, existing.as_ref());
    let open_picks: Vec<&Pick<'_>> = open.iter().map(|&at| &picks[at]).collect();
    let refused: Vec<_> = refused(&open_picks, &taken)
        .into_iter()
        .map(|(at, problem)| (open[at], problem))
        .collect();
    if !refused.is_empty() {
        let mut left = picks.to_vec();
        for &(at, _) in &refused {
            left[at].name = None;
        }
        let (open, _, mut names) =
            named(connector, start, &left, &tags, &taken, existing.as_ref());
        let open_left: Vec<&Pick<'_>> = open.iter().map(|&at| &left[at]).collect();
        // The run can open a pick that was present, whose given name it refuses: then
        // leaving the names out adds nothing, so no fix names a default.
        if !self::refused(&open_left, &taken).is_empty() {
            names.fill(None);
        }
        return Err(fixed(refused, &open, &names));
    }
    let written = written(kinds, kind, &open_picks, names, &index)?;
    let (source, at, item) = place(documents, block);
    let mut document = documents[at].clone();
    document.blocks[item].body.blocks.extend(written.body);
    let index = (existing.is_none() && !written.channels.is_empty()).then_some(index);
    document.blocks.extend(index.iter().map(self::index));
    document.blocks.extend(written.channels);
    let document =
        Checked::new(document).map_err(|error| vec![Diagnostic::from(&error)])?;
    let mut outcomes = vec![Outcome::Present; picks.len()];
    for (at, outcome) in open.into_iter().zip(written.outcomes) {
        outcomes[at] = outcome;
    }
    Ok(Added {
        source,
        document,
        index,
        picks: outcomes,
    })
}

/// The place of each pick that is not present: no block of the connector body, with
/// the tags `tags`, and no earlier open pick that is not in `unnamed` has its tag.
fn open(
    picks: &[Pick<'_>],
    tags: &BTreeSet<String>,
    unnamed: &BTreeSet<usize>,
) -> Vec<usize> {
    let mut added = BTreeSet::new();
    (0..picks.len())
        .filter(|&at| {
            let tag = picks[at].tag;
            if tags.contains(tag) || added.contains(tag) {
                return false;
            }
            if !unnamed.contains(&at) {
                added.insert(tag);
            }
            true
        })
        .collect()
}

/// The place of each pick of `picks` that is not present, the index, and the name of
/// each of those picks.
fn named(
    connector: &Name,
    start: &Path,
    picks: &[Pick<'_>],
    tags: &BTreeSet<String>,
    taken: &[Name],
    existing: Option<&Name>,
) -> (Vec<usize>, Name, Vec<Option<Name>>) {
    // A pick that ends `Unnamed` makes no later pick of its tag present, and opening
    // such a pick can leave another `Unnamed`, so name until no pick changes.
    let mut unnamed = BTreeSet::new();
    loop {
        let open = open(picks, tags, &unnamed);
        let given = open.iter().map(|&at| (picks[at].path, picks[at].name));
        let (index, names) = name(connector, start, given, taken, existing.cloned());
        let dropped = open.iter().zip(&names).filter(|(_, name)| name.is_none());
        let before = unnamed.len();
        unnamed.extend(dropped.map(|(&at, _)| at));
        if unnamed.len() == before {
            return (open, index, names);
        }
    }
}

/// `refused` with the fix of each, by the place of its pick, where `names` holds the
/// name of the pick at each place of `open` with each refused name left out.
fn fixed(
    refused: Vec<(usize, Diagnostic)>,
    open: &[usize],
    names: &[Option<Name>],
) -> Vec<Diagnostic> {
    refused
        .into_iter()
        .map(|(at, mut problem)| {
            // A refused name is reserved, which no default is, or another source holds
            // it. So the run with it left out frees no name, and no pick that ends
            // `Unnamed` takes one: the pick stays open.
            let at = open
                .iter()
                .position(|&open| open == at)
                .expect("invariant: a refused pick stays open with its name left out");
            problem.fix = match &names[at] {
                Some(default) => format!(
                    "Give another name, or leave it out for {:?}",
                    default.as_str()
                ),
                None => "Give another name".into(),
            };
            problem
        })
        .collect()
}

/// The blocks that an import writes, and what it did with each pick.
struct Written {
    /// The blocks of the kind, for the connector body.
    body: Vec<Block>,
    /// The `channel` block of each added pick.
    channels: Vec<Block>,
    outcomes: Vec<Outcome>,
}

/// The blocks of each of `picks` with its name of `names` on the index `index`.
///
/// # Errors
///
/// The diagnostics of [`Table::block`] for each pick.
fn written(
    kinds: &Table,
    kind: &str,
    picks: &[&Pick<'_>],
    names: Vec<Option<Name>>,
    index: &Name,
) -> Result<Written, Vec<Diagnostic>> {
    let mut written = Written {
        body: Vec::new(),
        channels: Vec::new(),
        outcomes: Vec::with_capacity(picks.len()),
    };
    let mut problems = Vec::new();
    for (pick, name) in picks.iter().zip(names) {
        let Some(name) = name else {
            written.outcomes.push(Outcome::Unnamed);
            continue;
        };
        match kinds.block(kind, pick.tag, pick.facts, &name) {
            Ok(blocks) => {
                let channel = channel(&name, index, pick.facts);
                let fills = fills(&blocks, &channel, kind, pick.tag);
                written.body.extend(blocks);
                written.channels.push(channel);
                written.outcomes.push(Outcome::Added { name, fills });
            }
            Err(found) => problems.extend(found),
        }
    }
    if problems.is_empty() {
        Ok(written)
    } else {
        Err(problems)
    }
}

/// The source of the document of `documents` that holds `block`, its place, and the
/// place of `block` in it.
fn place(documents: &[Document], block: &Block) -> (Source, usize, usize) {
    let (at, item) = documents
        .iter()
        .enumerate()
        .find_map(|(at, document)| {
            let item = document
                .blocks
                .iter()
                .position(|item| std::ptr::eq(item, block));
            item.map(|item| (at, item))
        })
        .expect("invariant: the connector block is a block of a document");
    let source = Source(u32::try_from(at).expect("invariant: fewer than 2^32 files"));
    (source, at, item)
}

/// Each name that `blocks` define as a channel or a connector, and each channel that
/// each connector other than `connector` implies or writes.
fn taken(blocks: &[&Block], connector: &Name, kinds: &Table) -> Vec<Name> {
    let mut taken: Vec<Name> = crate::names(blocks, definition::Kind::Channel)
        .map(|(name, ..)| name)
        .collect();
    for (name, _, block) in crate::names(blocks, definition::Kind::Connector) {
        if name != *connector {
            taken.extend(crate::connector::names(block, &name, kinds));
        }
        taken.push(name);
    }
    taken
}

/// The problem of each given name of `picks`, by the place of its pick, with no fix.
fn refused(picks: &[&Pick<'_>], taken: &[Name]) -> Vec<(usize, Diagnostic)> {
    let taken: BTreeSet<String> = taken.iter().map(folded).collect();
    let mut first: BTreeMap<String, &Path> = BTreeMap::new();
    let mut refused = Vec::new();
    for (at, pick) in picks.iter().enumerate() {
        let Some(name) = pick.name else {
            continue;
        };
        let (path, text, lower) = (pick.path, name.as_str(), folded(name));
        let (code, message) = if name.reserved() {
            let message = format!(
                "the name {text:?} of the point at \"{path}\" has a segment that starts \
                 with `@`, which is reserved"
            );
            (crate::RESERVED_NAME, message)
        } else if taken.contains(&lower) {
            let message = format!(
                "the name {text:?} of the point at \"{path}\" is in use in the files"
            );
            (DUPLICATE_NAME, message)
        } else if let Some(earlier) = first.get(&lower) {
            let message = format!(
                "the points at \"{earlier}\" and \"{path}\" have the name {text:?}"
            );
            (DUPLICATE_NAME, message)
        } else {
            first.insert(lower, path);
            continue;
        };
        refused.push((at, Diagnostic::new(code, None, message, String::new())));
    }
    refused
}

/// The index and the channel name of each pick of `picks`, by its path and its given
/// name: each given name, then the index, then the defaults. The index is `existing`,
/// else the first free of `<connector>.time`, `<connector>.time_2`, and on.
fn name<'a>(
    connector: &Name,
    start: &Path,
    picks: impl Iterator<Item = (&'a Path, Option<&'a Name>)>,
    taken: &[Name],
    existing: Option<Name>,
) -> (Name, Vec<Option<Name>>) {
    let picks: Vec<_> = picks.collect();
    let index = existing.unwrap_or_else(|| {
        let given = picks.iter().filter_map(|(_, given)| *given);
        let used: BTreeSet<String> = taken.iter().chain(given).map(folded).collect();
        let base = format!("{connector}.time");
        let (mut text, mut suffix) = (base.clone(), 1);
        while used.contains(&text.to_ascii_lowercase()) {
            suffix += 1;
            text = format!("{base}_{suffix}");
        }
        // `config::connector` checked `<connector>.status.time`, which is as long as
        // `<connector>.time_<n>` for each `n` under 10^6, and each `n` passes a name.
        text.parse()
            .expect("invariant: fewer than 10^6 names hold the index name")
    });
    let taken = taken.iter().chain(std::iter::once(&index));
    let names = import::names(connector, start, &picks, taken);
    (index, names)
}

/// The index that the `channel` block of the first of `writes` names, in `blocks`.
///
/// # Errors
///
/// The diagnostic of [`read::name`] for the `index` of that block.
fn index_of<'a>(
    blocks: &[&Block],
    writes: impl IntoIterator<Item = &'a Name>,
) -> Result<Option<Name>, Diagnostic> {
    let mut channels = BTreeMap::new();
    for (name, _, block) in crate::names(blocks, definition::Kind::Channel) {
        channels.entry(name).or_insert(block);
    }
    let index = writes
        .into_iter()
        .find_map(|name| channels.get(name)?.body.attributes.get("index"));
    index.map(|index| read::name(&index.value)).transpose()
}

/// The text of each `fill("<what>")` in `blocks` and in `channel`, the blocks of the
/// kind `kind` for the tag `tag`.
///
/// # Panics
///
/// If a call `fill` there has other arguments, which neither a kind nor [`channel`]
/// writes.
fn fills(blocks: &[Block], channel: &Block, kind: &str, tag: &str) -> Vec<String> {
    let mut fills = Vec::new();
    for block in blocks.iter().chain([channel]) {
        walk::each(&block.body, &mut |part| match part {
            Part::Value(value) => match import::unfilled(value) {
                Some(Fill::Of(what)) => {
                    fills.push(what.to_owned());
                    false
                }
                Some(Fill::Bad) => panic!(
                    "invariant: a kind writes each call `fill` as `fill` does, and the \
                     kind {kind:?} does not for the tag {tag:?}"
                ),
                None => true,
            },
            Part::Block(_) | Part::Key(..) => true,
        });
    }
    fills
}

/// `name` in lower case, so that two names that differ only in case are one.
fn folded(name: &Name) -> String {
    name.as_str().to_ascii_lowercase()
}

/// The `channel` block of the data channel `name` on the index channel `index`, with
/// the sample type and the unit of `facts`. When `facts` has no sample type, the block
/// holds [`fill`]`("the sample type")`. The unit is what [`Unit::given`] gives for the
/// unit and its code in `facts`, where an empty text or code counts as none. When it
/// gives none, the block holds `fill("the unit, which the device gives as <what>")`,
/// where `<what>` is `"<unit>"`, `the code "<code>"`, or `"<unit>" with the code
/// "<code>"`, as `facts` holds them. A sample type that holds no number gets no unit.
#[must_use]
pub fn channel(name: &Name, index: &Name, facts: &Facts) -> Block {
    let data_type = facts.data_type.map(DataType::Sample);
    let numeric = data_type.as_ref().is_none_or(DataType::numeric);
    let mut attributes = vec![
        attribute(
            "data_type",
            data_type.map_or_else(
                || fill("the sample type"),
                |data_type| string(&data_type.to_string()),
            ),
        ),
        attribute("index", string(index.as_str())),
    ];
    let text = facts.unit.as_deref().filter(|text| !text.is_empty());
    let code = facts.unit_code.as_deref().filter(|code| !code.is_empty());
    let given = |what: String| {
        Some(fill(&format!("the unit, which the device gives as {what}")))
    };
    let unit = match (Unit::given(text, code), text, code) {
        (Some(unit), ..) => Some(string(unit.as_str())),
        (None, None, None) => None,
        (None, Some(text), None) => given(format!("{text:?}")),
        (None, None, Some(code)) => given(format!("the code {code:?}")),
        (None, Some(text), Some(code)) => {
            given(format!("{text:?} with the code {code:?}"))
        }
    };
    if let Some(unit) = unit.filter(|_| numeric) {
        attributes.push(attribute("unit", unit));
    }
    block(name, attributes)
}

/// The `channel` block of the index channel `name`.
#[must_use]
pub fn index(name: &Name) -> Block {
    block(name, vec![attribute("kind", string("index"))])
}

fn block(name: &Name, attributes: Vec<Attribute>) -> Block {
    Block {
        keyword: "channel".into(),
        keyword_span: None,
        labels: vec![Label {
            text: name.as_str().into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(attributes)
                .expect("invariant: each key is written once"),
            blocks: Vec::new(),
        },
        span: None,
    }
}

fn attribute(key: &str, value: Value) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value,
    }
}

fn string(text: &str) -> Value {
    Value {
        kind: Kind::String(text.into()),
        span: None,
    }
}

#[cfg(test)]
mod tests;
