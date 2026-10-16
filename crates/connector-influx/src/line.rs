//! InfluxDB line protocol: one line for each point, with names checked and escaped
//! once, before the first line.

use std::fmt;
use std::io::Write as _;

use base64ct::{Base64, Encoding as _};

use types::time::Stamp;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory as _};

/// The most bytes of a string field between its quotes, after the escapes, that
/// each InfluxDB version takes.
pub(crate) const STRING_MAX: usize = 65_536;

/// One measurement's name and tags, checked and escaped once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measurement {
    prefix: Vec<u8>,
    /// Each tag key, as given.
    tags: Vec<String>,
}

/// A field key, checked and escaped once by [`Measurement::field`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field(Vec<u8>);

impl Measurement {
    /// Checks and escapes `name` and `tags`. The line sorts the tags by key, as
    /// InfluxDB asks for its fastest writes.
    ///
    /// # Errors
    ///
    /// - [`Error::Empty`] for an empty name, key, or tag value.
    /// - [`Error::Character`] for one with a backslash, a newline, a carriage
    ///   return, a tab, NUL, U+FFFD, or a character outside the general categories
    ///   L, M, N, P, and S other than the space U+0020.
    /// - [`Error::Reserved`] for a name or key that starts with `_`, or a key
    ///   `time`.
    /// - [`Error::Comment`] for a name that starts with `#`.
    /// - [`Error::Duplicate`] for a tag key that comes more than once.
    pub fn new(name: &str, tags: &[(&str, &str)]) -> Result<Self, Error> {
        unreserved(name)?;
        if name.starts_with('#') {
            return Err(Error::Comment(name.into()));
        }
        let mut tags = tags.to_vec();
        tags.sort_unstable_by_key(|&(key, _)| key);
        if let Some([(key, _), _]) = tags.array_windows().find(|[a, b]| a.0 == b.0) {
            return Err(Error::Duplicate((*key).into()));
        }
        let mut prefix = Vec::new();
        escape(name, b", ", Part::Measurement, &mut prefix)?;
        for &(key, value) in &tags {
            unreserved_key(key)?;
            prefix.push(b',');
            escape(key, b",= ", Part::TagKey, &mut prefix)?;
            prefix.push(b'=');
            escape(value, b",= ", Part::TagValue(key.into()), &mut prefix)?;
        }
        Ok(Self {
            prefix,
            tags: tags.into_iter().map(|(key, _)| key.into()).collect(),
        })
    }

    /// Checks and escapes the field key `key`. It costs O(tags).
    ///
    /// # Errors
    ///
    /// As [`Measurement::new`] gives for a key, and [`Error::Duplicate`] for a key
    /// that a tag of this measurement has.
    pub fn field(&self, key: &str) -> Result<Field, Error> {
        unreserved_key(key)?;
        if self.tags.iter().any(|tag| tag == key) {
            return Err(Error::Duplicate(key.into()));
        }
        let mut bytes = Vec::with_capacity(key.len());
        escape(key, b",= ", Part::FieldKey, &mut bytes)?;
        Ok(Field(bytes))
    }

    /// Appends one line at `time` to `out`, with each field of `values` and its
    /// value, in order. Each field comes from [`Measurement::field`] of this
    /// measurement: a field of another one can repeat a tag key of this one. With no
    /// value, nothing is written. A field that comes twice is written twice. Gives
    /// the number of fields written.
    pub fn line<'a>(
        &self,
        out: &mut Vec<u8>,
        values: impl IntoIterator<Item = (&'a Field, Value)>,
        time: Stamp,
    ) -> usize {
        let mut line = self.begin(out);
        for (field, value) in values {
            match value {
                Value::Float(value) => line.float(field, value.get()),
                Value::Integer(value) => line.integer(field, value),
                Value::Unsigned(value) => line.unsigned(field, value),
                Value::Boolean(value) => line.boolean(field, value),
            }
        }
        line.end(time)
    }

    /// Appends the prefix of a line to `out`, and gives the line to fill.
    pub(crate) fn begin<'a>(&self, out: &'a mut Vec<u8>) -> Line<'a> {
        let start = out.len();
        out.extend_from_slice(&self.prefix);
        Line {
            out,
            start,
            fields: 0,
        }
    }
}

/// One line that [`Measurement::begin`] started, as [`Measurement::line`] writes it.
/// Until [`Line::end`], the text of the line has no time.
#[must_use = "a line holds no time until `end`"]
pub(crate) struct Line<'a> {
    out: &'a mut Vec<u8>,
    /// The length of `out` before the line.
    start: usize,
    fields: usize,
}

impl Line<'_> {
    /// Appends the float field `field`, or leaves it out when `value` is NaN or
    /// infinite, which InfluxDB cannot store.
    #[inline]
    pub(crate) fn float(&mut self, field: &Field, value: f64) {
        if value.is_finite() {
            self.key(field);
            write!(self.out, "{value:e}")
                .expect("invariant: a write to a Vec never fails");
        }
    }

    /// Appends the integer field `field`.
    #[inline]
    pub(crate) fn integer(&mut self, field: &Field, value: i64) {
        self.key(field);
        write!(self.out, "{value}i").expect("invariant: a write to a Vec never fails");
    }

    /// Appends the unsigned field `field`.
    #[inline]
    pub(crate) fn unsigned(&mut self, field: &Field, value: u64) {
        self.key(field);
        write!(self.out, "{value}u").expect("invariant: a write to a Vec never fails");
    }

    /// Appends the boolean field `field`.
    #[inline]
    pub(crate) fn boolean(&mut self, field: &Field, value: bool) {
        self.key(field);
        self.out.push(if value { b't' } else { b'f' });
    }

    /// Appends the string field `field` of the UTF-8 text `text`, with `\` before
    /// each `"` and `\`, or leaves it out when that is over [`STRING_MAX`] bytes.
    #[inline]
    pub(crate) fn string(&mut self, field: &Field, text: &[u8]) {
        let escapes = text.iter().filter(|&&byte| escaped(byte)).count();
        if text.len().saturating_add(escapes) > STRING_MAX {
            return;
        }
        self.key(field);
        self.out.push(b'"');
        let mut rest = text;
        while let Some(at) = rest.iter().position(|&byte| escaped(byte)) {
            let (head, tail) = rest.split_at(at);
            let Some((&byte, tail)) = tail.split_first() else {
                unreachable!("invariant: `position` gives a byte of `rest`")
            };
            self.out.extend_from_slice(head);
            self.out.extend_from_slice(&[b'\\', byte]);
            rest = tail;
        }
        self.out.extend_from_slice(rest);
        self.out.push(b'"');
    }

    /// Appends the string field `field` of `bytes` in base64 with padding, or leaves
    /// it out when that is over [`STRING_MAX`] bytes: when `bytes` is over 49,152.
    #[inline]
    pub(crate) fn bytes(&mut self, field: &Field, bytes: &[u8]) {
        let len = Base64::encoded_len(bytes);
        if len > STRING_MAX {
            return;
        }
        self.key(field);
        self.out.push(b'"');
        let start = self.out.len();
        self.out.resize(start.saturating_add(len), 0);
        Base64::encode(bytes, self.out.split_at_mut(start).1)
            .expect("invariant: the text has the encoded length");
        self.out.push(b'"');
    }

    /// Appends the string field `field` of the uuid `bits`, as 36 lowercase chars.
    #[inline]
    pub(crate) fn uuid(&mut self, field: &Field, bits: u128) {
        self.key(field);
        write!(
            self.out,
            "\"{:08x}-{:04x}-{:04x}-{:04x}-{:012x}\"",
            bits >> 96,
            bits >> 80 & 0xffff,
            bits >> 64 & 0xffff,
            bits >> 48 & 0xffff,
            bits & 0xffff_ffff_ffff
        )
        .expect("invariant: a write to a Vec never fails");
    }

    /// Appends the separator and key of one more field.
    #[inline]
    fn key(&mut self, Field(key): &Field) {
        self.out.push(if self.fields == 0 { b' ' } else { b',' });
        self.out.extend_from_slice(key);
        self.out.push(b'=');
        self.fields = self.fields.saturating_add(1);
    }

    /// Ends the line at `time`, or removes it when it has no field. Gives the
    /// number of fields written.
    pub(crate) fn end(self, time: Stamp) -> usize {
        if self.fields == 0 {
            self.out.truncate(self.start);
            return 0;
        }
        writeln!(self.out, " {}", time.nanos())
            .expect("invariant: a write to a Vec never fails");
        self.fields
    }
}

/// One field value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A 64-bit float.
    Float(Float),
    /// A signed 64-bit integer.
    Integer(i64),
    /// A boolean.
    Boolean(bool),
    /// An unsigned 64-bit integer (`u`), which InfluxDB 2 and 3 store, and InfluxDB 1
    /// refuses.
    Unsigned(u64),
}

/// A finite 64-bit float. InfluxDB cannot store NaN or infinity.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Float(f64);

impl Float {
    /// Gives `value`, or `None` when it is NaN or infinite.
    #[must_use]
    pub fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    /// Gives the value.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Why a measurement or a field key is not valid line protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// An empty part.
    Empty(Part),
    /// A part with a character that [`Measurement::new`] or [`Measurement::field`]
    /// refuses.
    Character {
        /// The part that holds `character`.
        part: Part,
        /// The text of the part, as given.
        text: String,
        /// The first refused character in `text`.
        character: char,
    },
    /// A name or key that InfluxDB keeps for itself: one that starts with `_`, or
    /// the key `time`.
    Reserved(String),
    /// A measurement name that starts with `#`, which makes each line a comment.
    Comment(String),
    /// A key that comes more than once, in the tags or in a tag and a field.
    Duplicate(String),
}

/// A part of a measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// The measurement name.
    Measurement,
    /// A tag key.
    TagKey,
    /// The value of the tag with this key.
    TagValue(String),
    /// A field key.
    FieldKey,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(Part::Measurement) => {
                write!(f, "the measurement name is empty")
            }
            Self::Empty(Part::TagKey) => write!(f, "a tag key is empty"),
            Self::Empty(Part::TagValue(key)) => {
                write!(f, "the value of the tag {key:?} is empty")
            }
            Self::Empty(Part::FieldKey) => write!(f, "a field key is empty"),
            Self::Character {
                part,
                text,
                character,
            } => {
                match part {
                    Part::Measurement => write!(f, "the measurement name {text:?}")?,
                    Part::TagKey => write!(f, "the tag key {text:?}")?,
                    Part::TagValue(key) => {
                        write!(f, "the value {text:?} of the tag {key:?}")?;
                    }
                    Part::FieldKey => write!(f, "the field key {text:?}")?,
                }
                write!(f, " holds {character:?}, which a line cannot hold")
            }
            Self::Reserved(name) => {
                write!(f, "InfluxDB keeps the name {name:?} for itself")
            }
            Self::Comment(name) => write!(
                f,
                "the name {name:?} starts with '#', which makes each line a comment"
            ),
            Self::Duplicate(key) => write!(f, "the key {key:?} comes more than once"),
        }
    }
}

impl std::error::Error for Error {}

/// Refuses a key that InfluxDB keeps for itself.
fn unreserved_key(key: &str) -> Result<(), Error> {
    if key == "time" {
        return Err(Error::Reserved(key.into()));
    }
    unreserved(key)
}

/// Refuses a name or key that starts with `_`.
fn unreserved(name: &str) -> Result<(), Error> {
    if name.starts_with('_') {
        return Err(Error::Reserved(name.into()));
    }
    Ok(())
}

/// Appends `text`, the `part` of a measurement, to `out` with a backslash before
/// each byte in `special`.
fn escape(
    text: &str,
    special: &[u8],
    part: Part,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    if text.is_empty() {
        return Err(Error::Empty(part));
    }
    if let Some(character) = text.chars().find(|&c| refused(c)) {
        return Err(Error::Character {
            part,
            text: text.into(),
            character,
        });
    }
    for &byte in text.as_bytes() {
        if special.contains(&byte) {
            out.push(b'\\');
        }
        out.push(byte);
    }
    Ok(())
}

/// Past the backslash, this is what InfluxDB 1 and 2 with `validate-keys` drop. The
/// newline, carriage return, tab, and NUL are in `Other`.
fn refused(c: char) -> bool {
    c == '\\'
        || c == char::REPLACEMENT_CHARACTER
        || (c != ' '
            && matches!(
                c.general_category_group(),
                GeneralCategoryGroup::Separator | GeneralCategoryGroup::Other
            ))
}

/// Whether a string field writes a `\` before `byte`.
const fn escaped(byte: u8) -> bool {
    matches!(byte, b'"' | b'\\')
}

#[cfg(test)]
pub(crate) mod tests;
