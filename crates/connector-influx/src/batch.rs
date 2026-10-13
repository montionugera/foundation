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

/// The measurement of the data channels seen in the frames of one key set, and how
/// each field reads its samples.
#[derive(Debug)]
struct Layout {
    set: key_set::Key,
    /// In entry order.
    fields: Vec<Field>,
    measurement: Measurement,
}

/// One data channel of a [`Layout`].
#[derive(Debug)]
struct Field {
    entry: usize,
    name: String,
    width: usize,
    convert: Convert,
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
        let fits = (self.layout.as_ref())
            .is_some_and(|layout| layout.set == set.key() && layout.holds(data()));
        let layout = match &mut self.layout {
            Some(layout) if fits => layout,
            slot => {
                let mut fields = match slot.take() {
                    Some(layout) if layout.set == set.key() => layout.fields,
                    _ => Vec::new(),
                };
                for at in data() {
                    if let Err(place) = fields.binary_search_by_key(&at, |f| f.entry) {
                        fields.insert(place, field(received, at)?);
                    }
                }
                slot.insert(layout(&self.name, set.key(), fields)?)
            }
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
        let group = entry(set, first).group;
        let range = view
            .range(group)
            .expect("invariant: a view holds its index");
        let count = usize::try_from(range.count).expect("invariant: u32 fits usize");
        let index = set.index(first);
        let (_, bytes) = (view.iter().find(|&(entry, _)| entry == index))
            .expect("invariant: a view holds its index");
        codec::decode(entry(set, index).data_type, count, bytes, &mut self.times)
            .expect("invariant: the hub gives valid series");
        let fields = layout.fields.len();
        self.values.clear();
        let cells = count
            .checked_mul(fields)
            .expect("invariant: a frame fits memory");
        self.values.resize(cells, None);
        let series = view.iter().filter(|&(entry, _)| entry != index);
        let mut places = layout.fields.iter().enumerate();
        for (at, bytes) in series {
            let (place, field) = (places.find(|(_, field)| field.entry == at))
                .expect("invariant: the layout holds each data channel of the frame");
            codec::decode(entry(set, at).data_type, count, bytes, &mut self.raw)
                .expect("invariant: the hub gives valid series");
            let samples = self.raw.chunks_exact(field.width);
            for (line, sample) in self.values.chunks_exact_mut(fields).zip(samples) {
                let value = line
                    .get_mut(place)
                    .expect("invariant: a line has each field");
                *value = (field.convert)(sample);
            }
        }
        let lines = self.values.chunks_exact(fields);
        for (values, &time) in lines.zip(self.times.as_chunks::<8>().0) {
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

impl Layout {
    /// Whether the layout has a field for each of `entries`, in entry order.
    fn holds(&self, mut entries: impl Iterator<Item = usize>) -> bool {
        let mut fields = self.fields.iter();
        entries.all(|at| fields.any(|field| field.entry == at))
    }
}

/// The field of the data channel at `at` of `received`.
fn field(received: &Received<'_>, at: usize) -> Result<Field, Error> {
    let name = received.name(at);
    let data_type = entry(received.set(), at).data_type;
    let Some((width, convert)) = conversion(data_type) else {
        return Err(Error::Config(vec![Diagnostic::new(
            SAMPLE_TYPE,
            None,
            format!(
                "the channel {name:?} holds {data_type} samples, which the influx \
                 kind does not write"
            ),
            "Select only f64 channels".into(),
        )]));
    };
    Ok(Field {
        entry: at,
        name: name.to_owned(),
        width,
        convert,
    })
}

/// The layout of `fields` of the key set `set`.
fn layout(name: &str, set: key_set::Key, fields: Vec<Field>) -> Result<Layout, Error> {
    let keys: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
    let measurement = Measurement::new(name, &[], &keys).map_err(|error| {
        Error::Config(vec![Diagnostic::new(
            BAD_CHANNEL,
            None,
            format!("a channel name is not a field key: {error}"),
            "Rename the channel, or leave it out of `select`".into(),
        )])
    })?;
    Ok(Layout {
        set,
        fields,
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
