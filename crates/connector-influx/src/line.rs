//! InfluxDB line protocol: one line for each point, with names checked and escaped
//! once, before the first line.

use std::fmt;
use std::io::Write as _;

use types::time::Stamp;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory as _};

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
            line.field(field, |out| {
                value.write(out);
                true
            });
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
pub(crate) struct Line<'a> {
    out: &'a mut Vec<u8>,
    /// The length of `out` before the line.
    start: usize,
    fields: usize,
}

impl Line<'_> {
    /// Appends `field` with the value that `value` appends. When `value` gives
    /// `false`, the line leaves out the field and each byte that `value` appended.
    #[inline]
    pub(crate) fn field(
        &mut self,
        Field(key): &Field,
        value: impl FnOnce(&mut Vec<u8>) -> bool,
    ) {
        let mark = self.out.len();
        self.out.push(if self.fields == 0 { b' ' } else { b',' });
        self.out.extend_from_slice(key);
        self.out.push(b'=');
        if value(self.out) {
            self.fields = self.fields.saturating_add(1);
        } else {
            self.out.truncate(mark);
        }
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

impl Value {
    /// Appends the text of the value to `out`.
    fn write(self, out: &mut Vec<u8>) {
        match self {
            Self::Float(float) => self::float(out, float),
            Self::Integer(value) => integer(out, value),
            Self::Unsigned(value) => unsigned(out, value),
            Self::Boolean(value) => boolean(out, value),
        }
    }
}

/// Appends the text of the float field value `value` to `out`.
#[inline]
pub(crate) fn float(out: &mut Vec<u8>, Float(value): Float) {
    write!(out, "{value:e}").expect("invariant: a write to a Vec never fails");
}

/// Appends the text of the integer field value `value` to `out`.
#[inline]
pub(crate) fn integer(out: &mut Vec<u8>, value: i64) {
    write!(out, "{value}i").expect("invariant: a write to a Vec never fails");
}

/// Appends the text of the unsigned field value `value` to `out`.
#[inline]
pub(crate) fn unsigned(out: &mut Vec<u8>, value: u64) {
    write!(out, "{value}u").expect("invariant: a write to a Vec never fails");
}

/// Appends the text of the boolean field value `value` to `out`.
#[inline]
pub(crate) fn boolean(out: &mut Vec<u8>, value: bool) {
    out.push(if value { b't' } else { b'f' });
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

#[cfg(test)]
pub(crate) mod tests;
