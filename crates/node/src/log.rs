//! The log of a node: one line for each `tracing` event.

use std::fmt;
use std::io;
use std::sync::Mutex;

use clock::log::{Mesh, Time};
use tracing::field::{Field, Visit};
use tracing::{Dispatch, Event, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, layer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use types::time::Stamp;

/// Where and how the node writes its log: one line for each event at `level` or
/// above, with the stamp, the level, the target, the message, and the fields.
pub struct Log {
    /// Where each line goes, with one write for each line. `main` gives stderr. A
    /// failed write drops the line.
    pub writer: Box<dyn io::Write + Send>,
    /// The form of each line, text or JSON, and the least level of an event that the
    /// log holds.
    pub format: ops::Log,
}

impl fmt::Debug for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Log")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// The dispatch that writes each event of `log`'s level or above to `log`, stamped
/// with the time of `wall` and the mesh time of `mesh`.
pub(crate) fn dispatch(
    log: Log,
    wall: env::wall::Wall,
    mesh: clock::Reader,
) -> Dispatch {
    let Log {
        writer,
        format: ops::Log { json, level },
    } = log;
    let line = Line {
        json,
        time: clock::log::Reader::new(wall, mesh),
    };
    // The doc of `fmt` gives true as the default, though 0.3.23 sets false.
    let lines = layer()
        .event_format(line)
        .with_writer(Mutex::new(writer))
        .log_internal_errors(false);
    let registry = tracing_subscriber::registry();
    Dispatch::new(registry.with(LevelFilter::from_level(level)).with(lines))
}

/// Writes each event as one line, in text or in JSON.
struct Line {
    json: bool,
    time: clock::log::Reader,
}

impl<S, N> FormatEvent<S, N> for Line
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        #[expect(clippy::disallowed_methods, reason = "only the log stamps lines")]
        let time = self.time.now();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let metadata = event.metadata();
        let (level, target) = (*metadata.level(), metadata.target());
        if self.json {
            json(&mut writer, time, level, target, fields)
        } else {
            text(&mut writer, time, level, target, fields)
        }
    }
}

/// The fields of one event, in its order, with its message apart.
#[derive(Default)]
struct Fields {
    message: Option<String>,
    others: Vec<(&'static str, Value)>,
}

impl Fields {
    fn add(&mut self, field: &Field, value: Value) {
        match (field.name(), value) {
            ("message", Value::Text(message)) => self.message = Some(message),
            (name, value) => self.others.push((name, value)),
        }
    }
}

impl Visit for Fields {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.add(field, Value::Signed(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.add(field, Value::Unsigned(value.into()));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.add(field, Value::Signed(value));
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.add(field, Value::Unsigned(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.add(field, Value::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.add(field, Value::Text(value.to_owned()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.add(field, Value::Text(format!("{value:?}")));
    }
}

/// The value of one field. Integers and booleans keep their type, so JSON writes them
/// as numbers and booleans; each other value is text.
enum Value {
    Signed(i128),
    Unsigned(u128),
    Bool(bool),
    Text(String),
}

impl Value {
    /// Writes the value in the form of the log, with `text` for a `Text` value.
    fn write(
        &self,
        w: &mut Writer<'_>,
        text: fn(&mut Writer<'_>, &str) -> fmt::Result,
    ) -> fmt::Result {
        match self {
            Self::Signed(n) => write!(w, "{n}"),
            Self::Unsigned(n) => write!(w, "{n}"),
            Self::Bool(b) => write!(w, "{b}"),
            Self::Text(t) => text(w, t),
        }
    }
}

/// `<stamp> <LEVEL> <target>: <message> <key>=<value>... [mesh_time=<time>|none]`.
/// The message has `\` and each control character escaped, and each string value is
/// quoted as logfmt does ([`value`]), so the line stays one line.
fn text(
    w: &mut Writer<'_>,
    time: Time,
    level: tracing::Level,
    target: &str,
    fields: Fields,
) -> fmt::Result {
    write!(w, "{} {level} {target}:", micros(time.wall))?;
    if let Some(message) = fields.message {
        w.write_char(' ')?;
        for c in message.chars() {
            escape(w, c, c == '\\')?;
        }
    }
    for (name, field) in fields.others {
        write!(w, " {name}=")?;
        field.write(w, value)?;
    }
    match time.mesh {
        Mesh::Near => {}
        Mesh::Apart(mesh) => write!(w, " mesh_time={}", micros(mesh))?,
        Mesh::Unsynced => w.write_str(" mesh_time=none")?,
    }
    w.write_char('\n')
}

/// `text` as logfmt writes a value: in double quotes when it is empty or holds a
/// space, `=`, `"`, `\`, or a control character, with `"`, `\`, and each control
/// character escaped; else plain.
fn value(w: &mut Writer<'_>, text: &str) -> fmt::Result {
    let quoted = |c: char| matches!(c, ' ' | '=' | '"' | '\\') || c.is_control();
    if !text.is_empty() && !text.chars().any(quoted) {
        return w.write_str(text);
    }
    w.write_char('"')?;
    for c in text.chars() {
        escape(w, c, matches!(c, '"' | '\\'))?;
    }
    w.write_char('"')
}

/// Writes `c`, as `char::escape_debug` gives it when it is `escaped` or a control
/// character.
fn escape(w: &mut Writer<'_>, c: char, escaped: bool) -> fmt::Result {
    if escaped || c.is_control() {
        write!(w, "{}", c.escape_debug())
    } else {
        w.write_char(c)
    }
}

/// One JSON object with the keys `timestamp`, `level`, `target`, `fields` (the
/// message first), and `mesh_time`, in that order, written by hand as a `json!`
/// object sorts its keys.
fn json(
    w: &mut Writer<'_>,
    time: Time,
    level: tracing::Level,
    target: &str,
    fields: Fields,
) -> fmt::Result {
    let wall = micros(time.wall);
    write!(
        w,
        "{{\"timestamp\":\"{wall}\",\"level\":\"{level}\",\"target\":"
    )?;
    string(w, target)?;
    w.write_str(",\"fields\":{")?;
    let message = fields.message.map(|m| ("message", Value::Text(m)));
    for (i, (name, field)) in message.into_iter().chain(fields.others).enumerate() {
        if i > 0 {
            w.write_char(',')?;
        }
        string(w, name)?;
        w.write_char(':')?;
        field.write(w, string)?;
    }
    w.write_char('}')?;
    match time.mesh {
        Mesh::Near => {}
        Mesh::Apart(mesh) => write!(w, ",\"mesh_time\":\"{}\"", micros(mesh))?,
        Mesh::Unsynced => w.write_str(",\"mesh_time\":null")?,
    }
    w.write_str("}\n")
}

/// `text` as a JSON string.
fn string(w: &mut Writer<'_>, text: &str) -> fmt::Result {
    w.write_str(&serde_json::to_string(text).map_err(|_never| fmt::Error)?)
}

/// `stamp` in RFC 3339 UTC with six fraction digits, floored.
fn micros(stamp: Stamp) -> String {
    let mut text = stamp.to_string();
    let end = text.len() - 1;
    text.replace_range(end - 3..end, "");
    text
}
