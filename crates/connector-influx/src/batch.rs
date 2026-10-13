//! The lines of the frames that a reader gives, in one request body.

use connector::kind::Error;
use document::diagnostic::{Code, Diagnostic};
use hub::reader::{Position, Received};
use types::frame::key_set::{self, KeySet};
use types::sample::{Scalar, Type};
use types::time::Stamp;

use crate::line::{Float, Measurement, Value};

const SAMPLE_TYPE: Code = Code::new("influx.sample-type");
const BAD_CHANNEL: Code = Code::new("influx.bad-channel");

/// The most lines that a batch takes before [`Batch::full`].
const LINES_MAX: usize = 5_000;

/// The lines of the frames added since the last [`Batch::take`], and the reader's
/// position after the last of them.
#[derive(Debug)]
pub struct Batch {
    name: String,
    position: Option<Position>,
    layout: Option<Layout>,
    body: Body,
}

/// The lines of a batch, and the buffers that decode a frame into them.
#[derive(Debug, Default)]
struct Body {
    bytes: Vec<u8>,
    lines: usize,
    times: Vec<u8>,
    raw: Vec<u8>,
    values: Vec<Option<Value>>,
}

/// The measurement of one key set and set of present channels, and how each field
/// reads its samples.
#[derive(Debug)]
struct Layout {
    set: key_set::Key,
    entries: Vec<usize>,
    conversions: Vec<(usize, Convert)>,
    measurement: Measurement,
}

/// The value of one raw sample, or `None` when InfluxDB cannot store it.
type Convert = fn(&[u8]) -> Option<Value>;

impl Batch {
    /// An empty batch of lines of the measurement `name`, which
    /// [`Measurement::new`] takes.
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            position: None,
            layout: None,
            body: Body::default(),
        }
    }

    /// Adds one line for each sample time of `received`, with a field for each data
    /// channel of the frame, keyed by the channel's name. A line whose values are each
    /// NaN or infinite is left out.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `influx.sample-type` for a channel of a type that the
    /// kind does not write, and with `influx.bad-channel` for a channel name that is
    /// not a valid field key.
    pub fn add(&mut self, received: &Received<'_>) -> Result<(), Error> {
        self.position = Some(received.position());
        let (view, set) = (received.view(), received.set());
        let data = || {
            (view.iter())
                .map(|(entry, _)| entry)
                .filter(|&entry| set.index(entry) != entry)
        };
        let Some(first) = data().next() else {
            return Ok(());
        };
        let fits = self.layout.as_ref().is_some_and(|layout| {
            layout.set == set.key() && data().eq(layout.entries.iter().copied())
        });
        let layout = match &mut self.layout {
            Some(layout) if fits => layout,
            slot => slot.insert(layout(&self.name, received, data().collect())?),
        };
        self.body.write(layout, received, first);
        Ok(())
    }

    /// Whether the batch holds at least 5,000 lines, so the caller sends it before it
    /// adds more.
    #[must_use]
    pub const fn full(&self) -> bool {
        self.body.lines >= LINES_MAX
    }

    /// The body of the lines and the position after the last frame added, or `None`
    /// when no frame was added. Empties the batch.
    pub fn take(&mut self) -> Option<(Vec<u8>, Position)> {
        let position = self.position.take()?;
        self.body.lines = 0;
        let capacity = self.body.bytes.len();
        Some((
            std::mem::replace(&mut self.body.bytes, Vec::with_capacity(capacity)),
            position,
        ))
    }
}

impl Body {
    /// Writes the lines of `received`, whose first data channel is at `first`, with
    /// the `layout` of its channels.
    fn write(&mut self, layout: &Layout, received: &Received<'_>, first: usize) {
        let (view, set) = (received.view(), received.set());
        let Some(range) = view.range(entry(set, first).group) else {
            return;
        };
        let count = usize::try_from(range.count).expect("invariant: u32 fits usize");
        let index = set.index(first);
        if let Some((_, bytes)) = view.iter().find(|&(entry, _)| entry == index) {
            let data_type = entry(set, index).data_type;
            codec::decode(data_type, count, bytes, &mut self.times)
                .expect("invariant: the hub gives valid series");
        }
        let fields = layout.entries.len();
        self.values.clear();
        let cells = count
            .checked_mul(fields)
            .expect("invariant: a frame fits memory");
        self.values.resize(cells, None);
        let series = view.iter().filter(|&(entry, _)| entry != index);
        for (field, ((at, bytes), &(width, convert))) in
            series.zip(&layout.conversions).enumerate()
        {
            let data_type = entry(set, at).data_type;
            codec::decode(data_type, count, bytes, &mut self.raw)
                .expect("invariant: the hub gives valid series");
            let samples = self.raw.chunks_exact(width);
            for (row, sample) in self.values.chunks_exact_mut(fields).zip(samples) {
                if let Some(value) = row.get_mut(field) {
                    *value = convert(sample);
                }
            }
        }
        let rows = self.values.chunks_exact(fields);
        for (values, &time) in rows.zip(self.times.as_chunks::<8>().0) {
            let time = i64::from_le_bytes(time);
            let start = self.bytes.len();
            layout
                .measurement
                .line(&mut self.bytes, values, Stamp::from_nanos(time));
            if self.bytes.len() > start {
                self.lines = self.lines.saturating_add(1);
            }
        }
    }
}

/// The layout of the data channels at `entries` of `received`.
fn layout(
    name: &str,
    received: &Received<'_>,
    entries: Vec<usize>,
) -> Result<Layout, Error> {
    let set = received.set();
    let mut keys = Vec::with_capacity(entries.len());
    let mut conversions = Vec::with_capacity(entries.len());
    for &at in &entries {
        let channel = received.name(at);
        let data_type = entry(set, at).data_type;
        let Some(conversion) = conversion(data_type) else {
            return Err(Error::Config(vec![Diagnostic::new(
                SAMPLE_TYPE,
                None,
                format!(
                    "the channel {channel:?} holds {data_type} samples, which the \
                     influx kind does not write"
                ),
                "Select only f64 channels".into(),
            )]));
        };
        keys.push(channel);
        conversions.push(conversion);
    }
    let measurement = Measurement::new(name, &[], &keys).map_err(|error| {
        Error::Config(vec![Diagnostic::new(
            BAD_CHANNEL,
            None,
            format!("a channel name is not a field key: {error}"),
            "Rename the channel, or leave it out of `select`".into(),
        )])
    })?;
    Ok(Layout {
        set: set.key(),
        entries,
        conversions,
        measurement,
    })
}

fn entry(set: &KeySet, at: usize) -> &key_set::Entry {
    let entry = set.entries().get(at);
    entry.expect("invariant: a view gives entries of its key set")
}

/// The width and the conversion of a sample of `data_type`, or `None` when the kind
/// does not write that type.
fn conversion(data_type: Type) -> Option<(usize, Convert)> {
    match data_type {
        Type::Scalar(Scalar::F64) => Some((8, float)),
        _ => None,
    }
}

fn float(sample: &[u8]) -> Option<Value> {
    let bytes = sample
        .try_into()
        .expect("invariant: an f64 sample has 8 bytes");
    Float::new(f64::from_le_bytes(bytes)).map(Value::Float)
}
