//! The status of a connector, read from one frame of its status channels.

use std::collections::BTreeMap;

use codec::Variable;
use hub::reader::Received;
use types::channel;
use types::name::Name;
use types::sample::Type;

use super::{Class, State, TIME};

#[cfg(test)]
mod tests;

/// The values of one frame of a connector's status channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    /// What the connector's run does.
    pub state: State,
    /// The runs that the supervisor started again.
    pub restarts: u64,
    /// The class and text of the error that ended the last run, or `None` when the
    /// last run returned `Ok` or no run ended.
    pub error: Option<(Class, String)>,
    /// Each count that the kind names, by name.
    pub counts: BTreeMap<Name, u64>,
}

impl Reading {
    /// The newest status in `received`, a frame of a reader of the status channels
    /// of `connector`. `names` gives the name of each key in the frame. Gives `None`
    /// when the frame holds no sample of a channel of the supervisor, or a value of
    /// `state` or `class` that no state or class has.
    ///
    /// # Panics
    ///
    /// When the frame holds a series that does not decode, which the home refuses, or
    /// a status channel with a fixed type wider than 8 bytes.
    #[must_use]
    pub fn of(
        connector: &Name,
        received: &Received<'_>,
        names: &BTreeMap<channel::Key, Name>,
    ) -> Option<Self> {
        let prefix = format!("{connector}.status.");
        let names: BTreeMap<channel::Key, &str> = names
            .iter()
            .filter_map(|(key, name)| {
                let last = name.as_str().strip_prefix(&prefix)?;
                (last != TIME).then_some((*key, last))
            })
            .collect();
        let (set, view) = (received.set(), received.view());
        let mut numbers = BTreeMap::new();
        let mut text = None;
        for (entry, bytes) in view.iter() {
            let entry = &set.entries()[entry];
            let Some(&last) = names.get(&entry.key) else {
                continue;
            };
            let count = view.range(entry.group).map_or(0, |range| range.count);
            if count == 0 {
                continue;
            }
            match newest(entry.data_type, count, bytes) {
                Sample::Number(value) => numbers.insert(last, value),
                Sample::Text(value) => text.replace(value).and(None),
            };
        }
        let state = State::of(numbers.remove("state")?)?;
        let error = match numbers.remove("class")? {
            0 => None,
            code => Some((Class::of(code)?, text?)),
        };
        let restarts = numbers.remove("restarts")?;
        numbers.remove("backoff")?;
        let counts = numbers
            .into_iter()
            .map(|(last, value)| {
                let name = last.parse().expect("invariant: a segment of a name");
                (name, value)
            })
            .collect();
        Some(Self {
            state,
            restarts,
            error,
            counts,
        })
    }
}

/// The newest sample of a series of a status channel.
enum Sample {
    Number(u64),
    /// The text of `error`.
    Text(String),
}

/// The last of the `count` samples of the encoded series `bytes` of `data_type`.
///
/// # Panics
///
/// When the series does not decode, which the home refuses, or `count` is 0.
fn newest(data_type: Type, count: u32, bytes: &[u8]) -> Sample {
    let count =
        usize::try_from(count).expect("invariant: a frame's count fits in memory");
    let mut raw = Vec::new();
    codec::decode(data_type, count, bytes, &mut raw)
        .expect("invariant: the home holds series that decode");
    if let Some(form) = Variable::of(data_type) {
        let sample = form.samples(count, &raw).last();
        let sample = sample.expect("invariant: a group in a frame has a sample");
        return Sample::Text(String::from_utf8_lossy(sample).into_owned());
    }
    let width = data_type
        .width()
        .expect("invariant: a fixed type has a width");
    let sample = raw.rchunks_exact(width).next();
    let sample = sample.expect("invariant: a group in a frame has a sample");
    let mut widened = [0; 8];
    widened[..width].copy_from_slice(sample);
    Sample::Number(u64::from_le_bytes(widened))
}
