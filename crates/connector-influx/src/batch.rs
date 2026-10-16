//! The lines of the frames that a reader gives, in one request body.

use connector::kind::Error;
use document::diagnostic::{Code, Diagnostic};
use std::ops::Range;

use hub::reader::{Position, Received};
use types::frame::Path;
use types::frame::key_set::{self, KeySet};
use types::name::Name;
use types::sample::{Scalar, Type};
use types::time::Stamp;

use crate::gap::Gap;
use crate::line::{self, Float, Measurement, Value};

const SAMPLE_TYPE: Code = Code::new("influx.sample-type");
const BAD_CHANNEL: Code = Code::new("influx.bad-channel");

/// The most lines that a batch takes before [`Batch::full`].
const LINES_MAX: usize = 5_000;

/// The lines of the frames added since the last [`Batch::take`], and the reader's
/// position after the last of them.
#[derive(Debug)]
pub struct Batch {
    measurement: Measurement,
    connector: Name,
    /// The gap of the reader's index, made at the first gap.
    gap: Option<Gap>,
    position: Option<Position>,
    layout: Option<Layout>,
    body: Body,
}

/// The lines of a batch, and the buffers that decode a frame into them.
#[derive(Debug, Default)]
struct Body {
    bytes: Vec<u8>,
    /// The most bytes of a body taken.
    most: usize,
    lines: usize,
    samples: u64,
    lost: u64,
    times: Vec<u8>,
    raw: Vec<u8>,
    /// The entry of each data channel of the frame, in view order.
    entries: Vec<usize>,
    /// The values of each sample time of the frame, by the order of `entries`.
    values: Vec<Option<Value>>,
}

/// The lines that [`Batch::take`] gives.
#[derive(Debug)]
pub struct Taken {
    /// The lines, as a request body.
    pub body: Vec<u8>,
    /// What the run acks and counts once InfluxDB takes the lines.
    pub ack: Ack,
}

/// What the run acks and counts once InfluxDB takes the lines of a [`Taken`].
#[derive(Clone, Copy, Debug)]
pub struct Ack {
    /// The reader's position after the last frame of the batch.
    pub position: Position,
    /// The samples written as fields: a NaN or infinite sample is not.
    pub samples: u64,
    /// The seqs that the gap lines count.
    pub lost: u64,
}

/// The data channels seen in the frames of one key set.
#[derive(Debug)]
struct Layout {
    set: key_set::Key,
    /// By entry of the key set, each data channel seen.
    channels: Vec<Option<Channel>>,
}

/// One data channel of a [`Layout`]: its field, and how it reads its samples.
#[derive(Debug)]
struct Channel {
    field: line::Field,
    width: usize,
    convert: Convert,
}

/// The value of one raw sample, or `None` when InfluxDB cannot store it.
type Convert = fn(&[u8]) -> Option<Value>;

impl Batch {
    /// An empty batch of lines of the measurement `name`, whose gap lines name
    /// `connector`.
    ///
    /// # Panics
    ///
    /// When [`Measurement::new`] refuses `name` with no tags.
    #[must_use]
    pub fn new(name: &str, connector: &Name) -> Self {
        Self {
            measurement: Measurement::new(name, &[])
                .expect("invariant: config checks the measurement name"),
            connector: connector.clone(),
            gap: None,
            position: None,
            layout: None,
            body: Body::default(),
        }
    }

    /// Adds one line for each sample time of `received`, with a field for each data
    /// channel of the frame, keyed by the channel's name. A line whose values are each
    /// NaN or infinite is left out. Before them, it adds the `live` gap line of the
    /// frame's [`Received::gap`], at the stamp of the frame's first sample.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `influx.sample-type` for a channel of a type that the
    /// kind does not write, and with `influx.bad-channel` for a channel name that is
    /// not a valid field key.
    pub fn add(&mut self, received: &Received<'_>) -> Result<(), Error> {
        self.position = Some(received.position());
        if let Some(seqs) = received.gap() {
            let connector = &self.connector;
            let gap = (self.gap).get_or_insert_with(|| live(connector, received));
            self.body.gap(gap, received, seqs);
        }
        let (view, set) = (received.view(), received.set());
        let data = || {
            (view.iter())
                .map(|(entry, _)| entry)
                .filter(|&entry| set.index(entry) != entry)
        };
        let Some(first) = data().next() else {
            return Ok(());
        };
        let layout = match &mut self.layout {
            Some(layout) if layout.set == set.key() => layout,
            slot => slot.insert(Layout::new(set)),
        };
        for at in data() {
            layout.add(&self.measurement, received, at)?;
        }
        self.body.write(&self.measurement, layout, received, first);
        Ok(())
    }

    /// Whether the batch holds at least 5,000 lines, so the caller sends it before it
    /// adds more.
    #[must_use]
    pub const fn full(&self) -> bool {
        self.body.lines >= LINES_MAX
    }

    /// The lines of the frames added, or `None` when no frame was added. Empties the
    /// batch.
    pub fn take(&mut self) -> Option<Taken> {
        let position = self.position.take()?;
        self.body.lines = 0;
        self.body.most = self.body.most.max(self.body.bytes.len());
        let bytes = Vec::with_capacity(self.body.most);
        Some(Taken {
            body: std::mem::replace(&mut self.body.bytes, bytes),
            ack: Ack {
                position,
                samples: std::mem::take(&mut self.body.samples),
                lost: std::mem::take(&mut self.body.lost),
            },
        })
    }
}

impl Body {
    /// Writes the line of `gap` with the seqs `seqs`, which end at the first sample
    /// of `received`, at that sample's stamp.
    fn gap(&mut self, gap: &mut Gap, received: &Received<'_>, seqs: Range<u64>) {
        let index = index(received);
        self.times(received, index);
        let (first, _) = self.times.split_first_chunk::<8>().unwrap_or_else(|| {
            panic!("invariant: a frame after the gap {seqs:?} holds a sample")
        });
        let stamp = Stamp::from_nanos(i64::from_le_bytes(*first));
        let end = seqs.end;
        gap.add(seqs);
        let lost = gap.line(&mut self.bytes, end, stamp);
        self.lines = self.lines.saturating_add(1);
        self.lost = self.lost.saturating_add(lost);
    }

    /// Decodes the stamps of the index at `index` of `received` into `times`, and
    /// gives their count.
    fn times(&mut self, received: &Received<'_>, index: usize) -> usize {
        let (view, set) = (received.view(), received.set());
        let range = view
            .range(entry(set, index).group)
            .expect("invariant: a view holds its index");
        let count = usize::try_from(range.count).expect("invariant: u32 fits usize");
        let (_, bytes) = (view.iter().find(|&(entry, _)| entry == index))
            .expect("invariant: a view holds its index");
        codec::decode(entry(set, index).data_type, count, bytes, &mut self.times)
            .expect("invariant: the hub gives valid series");
        count
    }

    /// Writes the lines of `measurement` of `received`, whose first data channel is
    /// at `first`, with the `layout` of its channels.
    fn write(
        &mut self,
        measurement: &Measurement,
        layout: &Layout,
        received: &Received<'_>,
        first: usize,
    ) {
        let (view, set) = (received.view(), received.set());
        let index = set.index(first);
        let count = self.times(received, index);
        let series = || view.iter().filter(|&(entry, _)| entry != index);
        let fields = series().count();
        self.entries.clear();
        self.values.clear();
        let cells = count
            .checked_mul(fields)
            .expect("invariant: a frame fits memory");
        self.values.resize(cells, None);
        for (column, (at, bytes)) in series().enumerate() {
            let channel = layout.channel(at);
            self.entries.push(at);
            codec::decode(entry(set, at).data_type, count, bytes, &mut self.raw)
                .expect("invariant: the hub gives valid series");
            let samples = self.raw.chunks_exact(channel.width);
            for (line, sample) in self.values.chunks_exact_mut(fields).zip(samples) {
                let value = line
                    .get_mut(column)
                    .expect("invariant: a line has each field");
                *value = (channel.convert)(sample);
            }
        }
        let lines = self.values.chunks_exact(fields);
        for (values, &time) in lines.zip(self.times.as_chunks::<8>().0) {
            let time = Stamp::from_nanos(i64::from_le_bytes(time));
            let values =
                (self.entries.iter().zip(values)).filter_map(|(&at, value)| {
                    Some((&layout.channel(at).field, (*value)?))
                });
            let wrote = measurement.line(&mut self.bytes, values, time);
            if wrote > 0 {
                self.lines = self.lines.saturating_add(1);
            }
            let wrote = u64::try_from(wrote).expect("invariant: usize fits u64");
            self.samples = self.samples.saturating_add(wrote);
        }
    }
}

impl Layout {
    /// The layout of the key set `set`, with no channel seen.
    fn new(set: &KeySet) -> Self {
        Self {
            set: set.key(),
            channels: std::iter::repeat_with(|| None)
                .take(set.entries().len())
                .collect(),
        }
    }

    /// Adds the data channel at `at` of `received`, a field of `measurement`, when
    /// the layout has none.
    fn add(
        &mut self,
        measurement: &Measurement,
        received: &Received<'_>,
        at: usize,
    ) -> Result<(), Error> {
        let slot = self.channels.get_mut(at);
        let slot = slot.expect("invariant: a view gives entries of its key set");
        if slot.is_none() {
            let (width, convert) = conversion(received, at)?;
            let field = measurement
                .field(received.name(at))
                .map_err(|error| bad_channel(&error))?;
            *slot = Some(Channel {
                field,
                width,
                convert,
            });
        }
        Ok(())
    }

    /// The data channel at `at`.
    fn channel(&self, at: usize) -> &Channel {
        let channel = self.channels.get(at).and_then(Option::as_ref);
        channel.expect("invariant: the layout holds each data channel of the frame")
    }
}

fn bad_channel(error: &crate::line::Error) -> Error {
    Error::Config(vec![Diagnostic::new(
        BAD_CHANNEL,
        None,
        format!("a channel name is not a field key: {error}"),
        "Rename the channel, or leave it out of `select`".into(),
    )])
}

/// The `live` gap of `connector` on the index of `received`.
fn live(connector: &Name, received: &Received<'_>) -> Gap {
    let index = received.name(index(received));
    let index = index.parse().expect("invariant: a channel name is a name");
    Gap::new(connector, &index, Path::Live)
}

/// The entry of the index of `received`: a reader reads one index.
fn index(received: &Received<'_>) -> usize {
    let (view, set) = (received.view(), received.set());
    let mut entries = view.iter().map(|(entry, _)| entry);
    let index = entries.find(|&entry| set.index(entry) == entry);
    index.expect("invariant: a view holds its index")
}

fn entry(set: &KeySet, at: usize) -> &key_set::Entry {
    let entry = set.entries().get(at);
    entry.expect("invariant: a view gives entries of its key set")
}

/// The width and the conversion of a sample of the data channel at `at` of
/// `received`.
fn conversion(received: &Received<'_>, at: usize) -> Result<(usize, Convert), Error> {
    match entry(received.set(), at).data_type {
        Type::Scalar(Scalar::F64) => Ok((8, float)),
        data_type => Err(Error::Config(vec![Diagnostic::new(
            SAMPLE_TYPE,
            None,
            format!(
                "the channel {:?} holds {data_type} samples, which the influx kind \
                 does not write",
                received.name(at)
            ),
            "Select only f64 channels".into(),
        )])),
    }
}

fn float(sample: &[u8]) -> Option<Value> {
    let bytes = sample
        .try_into()
        .expect("invariant: an f64 sample has 8 bytes");
    Float::new(f64::from_le_bytes(bytes)).map(Value::Float)
}
