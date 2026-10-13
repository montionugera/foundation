//! The status of a connector, read from one frame of its status channels.

use std::collections::BTreeMap;

use codec::Variable;
use hub::reader::Received;
use types::name::Name;
use types::sample::Type;

use super::{Class, State, TIME, segment};

#[cfg(test)]
#[cfg(not(loom))]
mod tests;

/// The values of one frame of a connector's status channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    /// What the connector's run does.
    pub state: State,
    /// The runs that the supervisor started again.
    pub restarts: u64,
    /// The class and text of the error that ended the last run, or that the running
    /// kind shows, or `None` when neither holds.
    pub error: Option<(Class, String)>,
    /// Each count, by the last segment of its name.
    pub counts: BTreeMap<Name, u64>,
}

impl Reading {
    /// The newest status in `received`, a frame of a reader of the status channels of
    /// one connector: their index and each channel on it. A count is each channel that
    /// the supervisor does not write. Returns `None` when the frame holds no sample of
    /// `state`, `class`, or `restarts`, a value of `state` or `class` that no state or
    /// class has, or a class with no sample of `error`.
    ///
    /// # Panics
    ///
    /// When a series of `received` does not decode, or a channel has a sample type
    /// that its status channel never has.
    #[must_use]
    pub fn of(received: &Received<'_>) -> Option<Self> {
        let samples = Samples::of(received);
        let error = match samples.class? {
            0 => None,
            code => Some((Class::of(code)?, samples.text?)),
        };
        Some(Self {
            state: State::of(samples.state?)?,
            restarts: samples.restarts?,
            error,
            counts: samples.counts,
        })
    }
}

/// The newest sample of each status channel in a frame.
struct Samples {
    state: Option<u64>,
    class: Option<u64>,
    restarts: Option<u64>,
    text: Option<String>,
    counts: BTreeMap<Name, u64>,
}

impl Samples {
    /// The newest samples in `received`, each channel by the last segment of its name.
    /// Each channel that the supervisor does not write is a count.
    ///
    /// # Panics
    ///
    /// When a series of `received` does not decode.
    fn of(received: &Received<'_>) -> Self {
        let (set, view) = (received.set(), received.view());
        let mut samples = Self {
            state: None,
            class: None,
            restarts: None,
            text: None,
            counts: BTreeMap::new(),
        };
        for (entry, bytes) in view.iter() {
            let name = received.name(entry);
            let last = name.rsplit_once('.').map_or(name, |(_, last)| last);
            let entry = &set.entries()[entry];
            let range = view.range(entry.group);
            let count = range.expect("invariant: a view holds each index").count;
            let sample = || newest(entry.data_type, count, bytes);
            match last {
                TIME | segment::BACKOFF => {}
                segment::STATE => samples.state = Some(number(&sample())),
                segment::CLASS => samples.class = Some(number(&sample())),
                segment::RESTARTS => samples.restarts = Some(number(&sample())),
                segment::ERROR => {
                    let text = String::from_utf8(sample());
                    samples.text = Some(text.expect("invariant: decode checks UTF-8"));
                }
                _ => {
                    let name = last.parse().expect("invariant: a segment of a name");
                    samples.counts.insert(name, number(&sample()));
                }
            }
        }
        samples
    }
}

/// The bytes of the last of the `count` samples of the encoded series `bytes` of
/// `data_type`.
///
/// # Panics
///
/// When the series does not decode, or `count` is 0.
fn newest(data_type: Type, count: u32, bytes: &[u8]) -> Vec<u8> {
    let count =
        usize::try_from(count).expect("invariant: a frame's count fits in memory");
    let mut raw = Vec::new();
    // A remote reader passes a peer's series on unchecked (#2144).
    codec::decode(data_type, count, bytes, &mut raw).expect("a series that decodes");
    let sample = if let Some(form) = Variable::of(data_type) {
        form.samples(count, &raw).last()
    } else {
        let width = data_type.width();
        let width = width.expect("invariant: a fixed type has a width");
        raw.rchunks_exact(width).next()
    };
    sample
        .expect("invariant: a group in a frame has a sample")
        .to_vec()
}

/// The sample `bytes` of an unsigned type of at most 8 bytes, as a `u64`.
fn number(bytes: &[u8]) -> u64 {
    let mut widened = [0; 8];
    widened[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(widened)
}
