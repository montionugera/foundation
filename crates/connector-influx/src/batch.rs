//! The lines of the frames that a reader gives, in one request body.

use connector::kind::Error;
use document::diagnostic::{Code, Diagnostic};

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
/// position to ack after them.
#[derive(Debug)]
pub struct Batch {
    measurement: Measurement,
    connector: Name,
    /// The gap of the reader's index, made at the first gap.
    gap: Option<Gap>,
    /// The wait of the gap for a line with a value, while the gap holds seqs.
    waiting: Option<Waiting>,
    /// Whether a frame was added since the last take.
    added: bool,
    /// The position after the last frame added, also before the last take.
    last: Option<Position>,
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

/// A gap that waits for a line with a value: INFLUX SEQ AND GAPS puts a gap line at
/// the stamp of a data point.
#[derive(Clone, Copy, Debug)]
struct Waiting {
    /// The seq of the next sample of the reader.
    seq: u64,
    /// The position after the last frame before the gap, or `None` when no frame came
    /// before it.
    ack: Option<Position>,
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
    /// The reader's position after the last frame of the batch, or, while a gap waits
    /// for a line with a value, after the last frame before that gap. `None` when no
    /// frame came before that gap. A reader that opens again at it gets the gap again.
    pub position: Option<Position>,
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
            waiting: None,
            added: false,
            last: None,
            layout: None,
            body: Body::default(),
        }
    }

    /// Adds one line for each sample time of `received`, with a field for each data
    /// channel of the frame, keyed by the channel's name. A line whose values are each
    /// NaN or infinite is left out. Before the first line after a [`Received::gap`], it
    /// adds the `live` gap line, at the stamp of that line. The line counts each seq
    /// from the gap's first seq up to that line's seq, so also each sample time left
    /// out between them.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `influx.sample-type` for a channel of a type that the
    /// kind does not write, and with `influx.bad-channel` for a channel name that is
    /// not a valid field key.
    pub fn add(&mut self, received: &Received<'_>) -> Result<(), Error> {
        let before = self.last.replace(received.position());
        self.added = true;
        if let Some(seqs) = received.gap() {
            let connector = &self.connector;
            let gap = (self.gap).get_or_insert_with(|| live(connector, received));
            let ack = self.waiting.map_or(before, |waiting| waiting.ack);
            self.waiting = Some(Waiting { seq: seqs.end, ack });
            gap.add(seqs);
        }
        let (view, set) = (received.view(), received.set());
        let data = || {
            (view.iter())
                .map(|(entry, _)| entry)
                .filter(|&entry| set.index(entry) != entry)
        };
        let Some(first) = data().next() else {
            if let Some(waiting) = &mut self.waiting {
                waiting.seq =
                    (waiting.seq).saturating_add(count(received, index(received)));
            }
            return Ok(());
        };
        let layout = match &mut self.layout {
            Some(layout) if layout.set == set.key() => layout,
            slot => slot.insert(Layout::new(set)),
        };
        for at in data() {
            layout.add(&self.measurement, received, at)?;
        }
        let gap = (self.gap.as_mut()).zip(self.waiting.as_mut());
        let measurement = &self.measurement;
        if self.body.write(measurement, layout, received, first, gap) {
            self.waiting = None;
        }
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
        if !std::mem::take(&mut self.added) {
            return None;
        }
        let position = self.waiting.map_or(self.last, |waiting| waiting.ack);
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
    /// Decodes the stamps of the index at `index` of `received` into `times`, and
    /// gives their count.
    fn times(&mut self, received: &Received<'_>, index: usize) -> usize {
        let (view, set) = (received.view(), received.set());
        let count = count(received, index);
        let count = usize::try_from(count).expect("invariant: u64 fits usize");
        let (_, bytes) = (view.iter().find(|&(entry, _)| entry == index))
            .expect("invariant: a view holds its index");
        codec::decode(entry(set, index).data_type, count, bytes, &mut self.times)
            .expect("invariant: the hub gives valid series");
        count
    }

    /// Writes the lines of `measurement` of `received`, whose first data channel is
    /// at `first`, with the `layout` of its channels. With a `gap` that waits, writes
    /// its line before the first line with a value, and gives whether it did. Else the
    /// wait's seq moves past the frame.
    fn write(
        &mut self,
        measurement: &Measurement,
        layout: &Layout,
        received: &Received<'_>,
        first: usize,
        mut gap: Option<(&mut Gap, &mut Waiting)>,
    ) -> bool {
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
            if let Some((held, waiting)) = &mut gap {
                if values.clone().next().is_none() {
                    waiting.seq = waiting.seq.saturating_add(1);
                } else {
                    let lost = held.line(&mut self.bytes, waiting.seq, time);
                    self.lines = self.lines.saturating_add(1);
                    self.lost = self.lost.saturating_add(lost);
                    gap = None;
                }
            }
            let wrote = measurement.line(&mut self.bytes, values, time);
            if wrote > 0 {
                self.lines = self.lines.saturating_add(1);
            }
            let wrote = u64::try_from(wrote).expect("invariant: usize fits u64");
            self.samples = self.samples.saturating_add(wrote);
        }
        gap.is_none()
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

/// The count of samples of `received` on the index at `index`.
fn count(received: &Received<'_>, index: usize) -> u64 {
    let (view, set) = (received.view(), received.set());
    let range = view.range(entry(set, index).group);
    u64::from(range.expect("invariant: a view holds its index").count)
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
