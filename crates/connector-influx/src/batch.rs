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
    /// The most bytes of a body taken.
    most: usize,
    lines: usize,
    times: Vec<u8>,
    raw: Vec<u8>,
    /// The place of each field of the frame, in entry order.
    places: Vec<usize>,
    /// The values of each sample time of the frame, by the order of `places`.
    values: Vec<Option<Value>>,
}

/// The measurement of the data channels seen in the frames of one key set, and how
/// each field reads its samples.
#[derive(Debug)]
struct Layout {
    set: key_set::Key,
    /// By entry of the key set, the field of each data channel seen.
    fields: Vec<Option<Field>>,
    measurement: Measurement,
}

/// One data channel of a [`Layout`].
#[derive(Clone, Copy, Debug)]
struct Field {
    /// Its place in the measurement.
    place: usize,
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
        let layout = match &mut self.layout {
            Some(layout) if layout.set == set.key() => layout,
            slot => slot.insert(Layout::new(&self.name, received, first)?),
        };
        for at in data() {
            layout.add(received, at)?;
        }
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
        self.body.most = self.body.most.max(self.body.bytes.len());
        Some((
            std::mem::replace(&mut self.body.bytes, Vec::with_capacity(self.body.most)),
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
        let series = || view.iter().filter(|&(entry, _)| entry != index);
        let fields = series().count();
        self.places.clear();
        self.values.clear();
        let cells = count
            .checked_mul(fields)
            .expect("invariant: a frame fits memory");
        self.values.resize(cells, None);
        for (column, (at, bytes)) in series().enumerate() {
            let field = layout.field(at);
            self.places.push(field.place);
            codec::decode(entry(set, at).data_type, count, bytes, &mut self.raw)
                .expect("invariant: the hub gives valid series");
            let samples = self.raw.chunks_exact(field.width);
            for (line, sample) in self.values.chunks_exact_mut(fields).zip(samples) {
                let value = line
                    .get_mut(column)
                    .expect("invariant: a line has each field");
                *value = (field.convert)(sample);
            }
        }
        let lines = self.values.chunks_exact(fields);
        for (values, &time) in lines.zip(self.times.as_chunks::<8>().0) {
            let time = Stamp::from_nanos(i64::from_le_bytes(time));
            let start = self.bytes.len();
            let values = (self.places.iter().zip(values))
                .filter_map(|(&place, value)| Some((place, (*value)?)));
            layout
                .measurement
                .line_by_place(&mut self.bytes, values, time);
            if self.bytes.len() > start {
                self.lines = self.lines.saturating_add(1);
            }
        }
    }
}

impl Layout {
    /// The layout of the key set of `received`, with the field of its data channel
    /// at `first`.
    fn new(name: &str, received: &Received<'_>, first: usize) -> Result<Self, Error> {
        let (width, convert) = conversion(received, first)?;
        let key = received.name(first);
        let measurement =
            Measurement::new(name, &[], &[key]).map_err(|error| bad_channel(&error))?;
        let set = received.set();
        let mut fields = vec![None; set.entries().len()];
        let field = fields.get_mut(first);
        *field.expect("invariant: a view gives entries of its key set") = Some(Field {
            place: 0,
            width,
            convert,
        });
        Ok(Self {
            set: set.key(),
            fields,
            measurement,
        })
    }

    /// Adds the field of the data channel at `at` of `received`, when the layout has
    /// none.
    fn add(&mut self, received: &Received<'_>, at: usize) -> Result<(), Error> {
        let slot = self.fields.get_mut(at);
        let slot = slot.expect("invariant: a view gives entries of its key set");
        if slot.is_none() {
            let (width, convert) = conversion(received, at)?;
            let key = received.name(at);
            let place = self
                .measurement
                .add_field(key)
                .map_err(|error| bad_channel(&error))?;
            *slot = Some(Field {
                place,
                width,
                convert,
            });
        }
        Ok(())
    }

    /// The field of the data channel at `at`.
    fn field(&self, at: usize) -> Field {
        let field = self.fields.get(at).copied().flatten();
        field.expect("invariant: the layout holds each data channel of the frame")
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
