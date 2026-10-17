//! The byte layout of samples.

use std::fmt;
use std::str::FromStr;

use crate::time::{Span, Stamp};

/// Defines [`Scalar`], its text, and `SCALARS` from one list, so no scalar can miss its
/// text or its place in `SCALARS`.
macro_rules! scalars {
    ($($(#[$doc:meta])* $variant:ident = $name:literal,)*) => {
        /// A fixed-width sample type.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Scalar {
            $($(#[$doc])* $variant,)*
        }

        impl Scalar {
            /// The name of the scalar in the text of a [`Type`].
            const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }

        /// Each scalar, in the order of its declaration.
        const SCALARS: &[Scalar] = &[$(Scalar::$variant,)*];
    };
}

scalars! {
    /// One byte: 0 or 1.
    Bool = "bool",
    /// Signed 8-bit integer.
    I8 = "i8",
    /// Signed 16-bit integer.
    I16 = "i16",
    /// Signed 32-bit integer.
    I32 = "i32",
    /// Signed 64-bit integer.
    I64 = "i64",
    /// Unsigned 8-bit integer.
    U8 = "u8",
    /// Unsigned 16-bit integer.
    U16 = "u16",
    /// Unsigned 32-bit integer.
    U32 = "u32",
    /// Unsigned 64-bit integer.
    U64 = "u64",
    /// 32-bit float.
    F32 = "f32",
    /// 64-bit float.
    F64 = "f64",
    /// A [`crate::time::Stamp`].
    Stamp = "timestamp",
    /// A [`crate::time::Span`].
    Span = "duration",
    /// A 128-bit UUID.
    Uuid = "uuid",
}

impl Scalar {
    /// Bytes per sample.
    #[must_use]
    pub const fn width(self) -> usize {
        match self {
            Self::Bool | Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 | Self::Stamp | Self::Span => 8,
            Self::Uuid => 16,
        }
    }

    /// The scalar that `text` names.
    fn named(text: &str) -> Option<Self> {
        SCALARS.iter().copied().find(|scalar| scalar.name() == text)
    }
}

/// The byte layout of one channel's samples.
///
/// Enums and flags use an integer layout, and quality uses `U32`; their meaning is in
/// `spec`.
// Declared in the order of the STORED BODY kind codes, so each tag is its kind code
// and `home::stored` maps one to the other with no branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// One fixed-width value per sample.
    Scalar(Scalar),
    /// A fixed array per sample.
    Array {
        /// The element type.
        element: Scalar,
        /// Elements per sample.
        len: u32,
    },
    /// A list of at most `max` elements per sample.
    List {
        /// The element type.
        element: Scalar,
        /// The most elements one sample may hold.
        max: u32,
    },
    /// UTF-8 text per sample.
    String,
    /// Unlabeled bytes per sample.
    Bytes,
    /// A fixed array of arrays per sample, row-major. A sample has the bytes of an
    /// array of `rows * columns` elements; the shape is only in the type.
    Matrix {
        /// The element type.
        element: Scalar,
        /// The rows and columns.
        sides: Sides,
    },
}

/// The sides of a matrix: `rows` arrays of `columns` elements.
// Aligned to 4, so the sides sit at byte 4 of `Type`, as `Array.len`, `List.max`, and
// the stored `n` do. `C` keeps `rows` in the low half, as in `n`.
#[repr(C, align(4))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sides {
    /// Arrays per sample.
    pub rows: u16,
    /// Elements per array.
    pub columns: u16,
}

impl Type {
    /// Bytes per sample, or `None` when samples vary in size.
    #[must_use]
    pub const fn width(self) -> Option<usize> {
        match self {
            Self::Scalar(s) => Some(s.width()),
            Self::Array { element, len } => Some(element.width() * len as usize),
            Self::Matrix {
                element,
                sides: Sides { rows, columns },
            } => Some(element.width() * rows as usize * columns as usize),
            Self::List { .. } | Self::String | Self::Bytes => None,
        }
    }
}

impl fmt::Display for Type {
    /// Writes the text that a user writes and [`Type::from_str`] reads: `f64`,
    /// `f32[3]`, `f32[2][3]`, `list<u8, 16>`, `string`, or `bytes`. The client wire
    /// carries this text too, so a change of it is a change of that wire.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Scalar(scalar) => f.write_str(scalar.name()),
            Self::Array { element, len } => write!(f, "{}[{len}]", element.name()),
            Self::Matrix {
                element,
                sides: Sides { rows, columns },
            } => write!(f, "{}[{rows}][{columns}]", element.name()),
            Self::List { element, max } => {
                write!(f, "list<{}, {max}>", element.name())
            }
            Self::String => f.write_str("string"),
            Self::Bytes => f.write_str("bytes"),
        }
    }
}

impl FromStr for Type {
    type Err = Error;

    /// Reads the text that `Display` writes, and only that text: a scalar (`bool`,
    /// `i8` to `i64`, `u8` to `u64`, `f32`, `f64`, `timestamp`, `duration`, or `uuid`),
    /// an array `<scalar>[<len>]`, a matrix `<scalar>[<rows>][<columns>]`, a list
    /// `list<<scalar>, <max>>`, `string`, or `bytes`. Case is exact, a count has no
    /// leading zero, and the one space is after the comma of a list.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "string" => return Ok(Self::String),
            "bytes" => return Ok(Self::Bytes),
            _ => {}
        }
        if let Some(list) = text
            .strip_prefix("list<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            let (name, max) = list.split_once(", ").ok_or(Error::Syntax)?;
            return Ok(Self::List {
                element: element(name)?,
                max: count(max)?,
            });
        }
        if let Some(array) = text.strip_suffix(']') {
            let (name, len) = array.split_once('[').ok_or(Error::Syntax)?;
            let element = element(name)?;
            let mut lengths = len.split("][");
            let (Some(len), columns, None) =
                (lengths.next(), lengths.next(), lengths.next())
            else {
                return Err(Error::Lengths);
            };
            return match columns {
                None => Ok(Self::Array {
                    element,
                    len: count(len)?,
                }),
                Some(columns) => Ok(Self::Matrix {
                    element,
                    sides: Sides {
                        rows: side(len)?,
                        columns: side(columns)?,
                    },
                }),
            };
        }
        Scalar::named(text).map(Self::Scalar).ok_or(Error::Syntax)
    }
}

/// The scalar that `text` names as the element of an array, a matrix, or a list. A
/// scalar with space around it is a fault of syntax, not of the element.
fn element(text: &str) -> Result<Scalar, Error> {
    match Scalar::named(text) {
        Some(scalar) => Ok(scalar),
        None if Scalar::named(text.trim()).is_some() => Err(Error::Syntax),
        None => Err(Error::Element),
    }
}

/// The array length or list maximum that `text` writes.
fn count(text: &str) -> Result<u32, Error> {
    digits(text).ok_or(Error::Count)
}

/// The length of a matrix side that `text` writes.
fn side(text: &str) -> Result<u16, Error> {
    digits(text).ok_or(Error::Matrix)
}

/// The number that `text` writes in ASCII digits with no leading zero, when it fits a
/// `T`.
fn digits<T: FromStr>(text: &str) -> Option<T> {
    let digits = text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}

/// One sample of a fixed-width scalar, with the byte layout of its [`Scalar`]: little
/// endian, and a `bool` as one byte, 0 or 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A `bool` sample.
    Bool(bool),
    /// An `i8` sample.
    I8(i8),
    /// An `i16` sample.
    I16(i16),
    /// An `i32` sample.
    I32(i32),
    /// An `i64` sample.
    I64(i64),
    /// A `u8` sample.
    U8(u8),
    /// A `u16` sample.
    U16(u16),
    /// A `u32` sample.
    U32(u32),
    /// A `u64` sample.
    U64(u64),
    /// An `f32` sample.
    F32(f32),
    /// An `f64` sample.
    F64(f64),
    /// A `timestamp` sample.
    Stamp(Stamp),
    /// A `duration` sample.
    Span(Span),
    /// A `uuid` sample: its 16 bytes in RFC 9562 order, so byte 0 gives the first two
    /// hex digits of its text form.
    Uuid([u8; 16]),
}

impl Value {
    /// The scalar of the value.
    #[must_use]
    pub const fn scalar(self) -> Scalar {
        match self {
            Self::Bool(_) => Scalar::Bool,
            Self::I8(_) => Scalar::I8,
            Self::I16(_) => Scalar::I16,
            Self::I32(_) => Scalar::I32,
            Self::I64(_) => Scalar::I64,
            Self::U8(_) => Scalar::U8,
            Self::U16(_) => Scalar::U16,
            Self::U32(_) => Scalar::U32,
            Self::U64(_) => Scalar::U64,
            Self::F32(_) => Scalar::F32,
            Self::F64(_) => Scalar::F64,
            Self::Stamp(_) => Scalar::Stamp,
            Self::Span(_) => Scalar::Span,
            Self::Uuid(_) => Scalar::Uuid,
        }
    }

    /// Writes the value as one sample.
    ///
    /// # Panics
    ///
    /// When `sample` is not `self.scalar().width()` bytes.
    #[expect(
        clippy::inline_always,
        reason = "with `inline`, a caller in another crate called it for each sample"
    )]
    #[inline(always)]
    pub fn write(self, sample: &mut [u8]) {
        match self {
            Self::Bool(value) => *sized(sample) = [u8::from(value)],
            Self::I8(value) => *sized(sample) = value.to_le_bytes(),
            Self::I16(value) => *sized(sample) = value.to_le_bytes(),
            Self::I32(value) => *sized(sample) = value.to_le_bytes(),
            Self::I64(value) => *sized(sample) = value.to_le_bytes(),
            Self::U8(value) => *sized(sample) = value.to_le_bytes(),
            Self::U16(value) => *sized(sample) = value.to_le_bytes(),
            Self::U32(value) => *sized(sample) = value.to_le_bytes(),
            Self::U64(value) => *sized(sample) = value.to_le_bytes(),
            Self::F32(value) => *sized(sample) = value.to_le_bytes(),
            Self::F64(value) => *sized(sample) = value.to_le_bytes(),
            Self::Stamp(value) => *sized(sample) = value.nanos().to_le_bytes(),
            Self::Span(value) => *sized(sample) = value.nanos().to_le_bytes(),
            Self::Uuid(value) => *sized(sample) = value,
        }
    }

    /// Reads one sample of `scalar`. A `bool` byte other than 0 is `true`.
    ///
    /// # Panics
    ///
    /// When `sample` is not `scalar.width()` bytes.
    #[inline]
    #[must_use]
    pub fn read(scalar: Scalar, sample: &[u8]) -> Self {
        match scalar {
            Scalar::Bool => Self::Bool(bytes::<1>(sample)[0] != 0),
            Scalar::I8 => Self::I8(i8::from_le_bytes(bytes(sample))),
            Scalar::I16 => Self::I16(i16::from_le_bytes(bytes(sample))),
            Scalar::I32 => Self::I32(i32::from_le_bytes(bytes(sample))),
            Scalar::I64 => Self::I64(i64::from_le_bytes(bytes(sample))),
            Scalar::U8 => Self::U8(u8::from_le_bytes(bytes(sample))),
            Scalar::U16 => Self::U16(u16::from_le_bytes(bytes(sample))),
            Scalar::U32 => Self::U32(u32::from_le_bytes(bytes(sample))),
            Scalar::U64 => Self::U64(u64::from_le_bytes(bytes(sample))),
            Scalar::F32 => Self::F32(f32::from_le_bytes(bytes(sample))),
            Scalar::F64 => Self::F64(f64::from_le_bytes(bytes(sample))),
            Scalar::Stamp => {
                Self::Stamp(Stamp::from_nanos(i64::from_le_bytes(bytes(sample))))
            }
            Scalar::Span => {
                Self::Span(Span::from_nanos(i64::from_le_bytes(bytes(sample))))
            }
            Scalar::Uuid => Self::Uuid(bytes(sample)),
        }
    }
}

/// `sample` as an array of `N` bytes.
///
/// # Panics
///
/// When `sample` is not `N` bytes.
fn sized<const N: usize>(sample: &mut [u8]) -> &mut [u8; N] {
    let len = sample.len();
    <&mut [u8; N]>::try_from(sample)
        .unwrap_or_else(|_| panic!("a sample of {N} bytes, not {len}"))
}

/// The `N` bytes of `sample`.
///
/// # Panics
///
/// When `sample` is not `N` bytes.
fn bytes<const N: usize>(sample: &[u8]) -> [u8; N] {
    <[u8; N]>::try_from(sample)
        .unwrap_or_else(|_| panic!("a sample of {N} bytes, not {}", sample.len()))
}

/// Why a text is not a sample type. `Display` gives the message: a lower-case clause
/// with no final period. [`Error::fix`] gives what to do instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The text has none of the forms of a type. `F64`, `float`, `f32[3`,
    /// `list<u8,16>`, and a scalar element with space around it, as in `f32 [3]`, give
    /// this error.
    Syntax,
    /// The element of an array, a matrix, or a list is not a scalar, as in `string[3]`
    /// or `list<f32[2], 4>`.
    Element,
    /// An array length or a list maximum is not ASCII digits with no leading zero, or
    /// is over 4294967295, as in `f32[]`, `f32[03]`, or `f32[-1]`.
    Count,
    /// An array has more than two lengths, as in `u8[1][1][1]`.
    Lengths,
    /// A matrix length is not ASCII digits with no leading zero, or is over 65535, as
    /// in `f32[2][]`, `f32[2][x]`, or `f32[70000][3]`.
    Matrix,
}

impl Error {
    /// What to do instead: a sentence with no final period.
    #[must_use]
    pub const fn fix(self) -> &'static str {
        match self {
            Self::Syntax => {
                "Use one of the forms that the message names, with exact case and a \
                 space only after the comma of a list"
            }
            Self::Element => {
                "Use a scalar as the element, such as f32[3] or list<u8, 16>"
            }
            Self::Count => "Write the count in plain digits, such as 16",
            Self::Lengths => {
                "Use one or two array lengths, such as f32[3] or f32[2][3]"
            }
            Self::Matrix => {
                "Write each matrix length in plain digits up to 65535, such as \
                 f32[2][3], or use an array"
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Syntax => {
                "expected a sample type such as f64, f32[3], list<u8, 16>, string, or \
                 bytes"
            }
            Self::Element => {
                f.write_str("expected a scalar element: ")?;
                for (index, scalar) in SCALARS.iter().enumerate() {
                    let gap = match index {
                        0 => "",
                        _ if index + 1 == SCALARS.len() => ", or ",
                        _ => ", ",
                    };
                    write!(f, "{gap}{}", scalar.name())?;
                }
                return Ok(());
            }
            Self::Count => "expected a count of 0 to 4294967295, with no leading zero",
            Self::Lengths => "expected one or two array lengths",
            Self::Matrix => {
                "expected each matrix length of 0 to 65535, with no leading zero"
            }
        })
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::assert_stated;
    use proptest::prelude::*;

    fn types() -> impl Strategy<Value = Type> {
        let scalar = prop::sample::select(SCALARS);
        prop_oneof![
            scalar.clone().prop_map(Type::Scalar),
            (scalar.clone(), any::<u32>())
                .prop_map(|(element, len)| Type::Array { element, len }),
            (scalar.clone(), any::<u16>(), any::<u16>()).prop_map(
                |(element, rows, columns)| Type::Matrix {
                    element,
                    sides: Sides { rows, columns },
                }
            ),
            (scalar, any::<u32>())
                .prop_map(|(element, max)| Type::List { element, max }),
            Just(Type::String),
            Just(Type::Bytes),
        ]
    }

    #[test]
    fn writes_and_reads_each_form() {
        let cases = [
            (Type::Scalar(Scalar::F64), "f64"),
            (
                Type::Array {
                    element: Scalar::F32,
                    len: 3,
                },
                "f32[3]",
            ),
            (
                Type::Array {
                    element: Scalar::Uuid,
                    len: 0,
                },
                "uuid[0]",
            ),
            (matrix(Scalar::F32, 2, 3), "f32[2][3]"),
            (matrix(Scalar::U8, 0, 5), "u8[0][5]"),
            (matrix(Scalar::F32, 1, 3), "f32[1][3]"),
            (matrix(Scalar::U8, 65535, 65535), "u8[65535][65535]"),
            (matrix(Scalar::U8, 0, 0), "u8[0][0]"),
            (
                Type::List {
                    element: Scalar::U8,
                    max: u32::MAX,
                },
                "list<u8, 4294967295>",
            ),
            (Type::String, "string"),
            (Type::Bytes, "bytes"),
        ];
        for (sample, text) in cases {
            assert_eq!(sample.to_string(), text);
            assert_eq!(text.parse(), Ok(sample), "{text}");
        }
        let names: Vec<_> = SCALARS
            .iter()
            .map(|&scalar| Type::Scalar(scalar).to_string())
            .collect();
        let expected = [
            "bool",
            "i8",
            "i16",
            "i32",
            "i64",
            "u8",
            "u16",
            "u32",
            "u64",
            "f32",
            "f64",
            "timestamp",
            "duration",
            "uuid",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn refuses_each_other_text() {
        let cases = [
            ("", Error::Syntax),
            ("F64", Error::Syntax),
            ("float", Error::Syntax),
            (" f64", Error::Syntax),
            ("f32[3", Error::Syntax),
            ("f32 [3]", Error::Syntax),
            (" f32[3]", Error::Syntax),
            ("list<u8 , 16>", Error::Syntax),
            ("stamp", Error::Syntax),
            ("span", Error::Syntax),
            ("stamp[3]", Error::Element),
            ("list<span, 16>", Error::Element),
            ("f32 [2][3]", Error::Syntax),
            ("list<u8, 16>[2]", Error::Element),
            ("list<u8, 16>[2][3]", Error::Element),
            ("list<list<u8, 16>, 4>", Error::Element),
            ("f32 [03]", Error::Syntax),
            ("list<u8,16>", Error::Syntax),
            ("list<u8, 16> ", Error::Syntax),
            ("String", Error::Syntax),
            ("string[3]", Error::Element),
            ("[3]", Error::Element),
            ("list<f32[2], 4>", Error::Element),
            ("list<, 4>", Error::Element),
            ("f32[]", Error::Count),
            ("f32[03]", Error::Count),
            ("f32[ 3]", Error::Count),
            ("f32[-1]", Error::Count),
            ("f32[+1]", Error::Count),
            ("f32[4294967296]", Error::Count),
            ("list<u8, 016>", Error::Count),
            ("list<u8,  16>", Error::Count),
            ("f32[2][]", Error::Matrix),
            ("f32[2][03]", Error::Matrix),
            ("f32[2]][3]", Error::Matrix),
            ("f32[x][3]", Error::Matrix),
            ("f32[03][3]", Error::Matrix),
            ("f32[2] [3]", Error::Count),
            ("f32[2][4294967296]", Error::Matrix),
            ("f32[2][3", Error::Syntax),
            ("f32[2][3][", Error::Syntax),
            ("string[2][3]", Error::Element),
            ("u8[1][1][1]", Error::Lengths),
            ("u8[1][1][]", Error::Lengths),
            ("f32[70000][3]", Error::Matrix),
            ("u8[65535][65536]", Error::Matrix),
            ("u8[65536][0]", Error::Matrix),
            ("u8[4294967295][2]", Error::Matrix),
            ("u8[2][4294967296]", Error::Matrix),
            ("u8[99999999999999999999][1]", Error::Matrix),
        ];
        for (text, error) in cases {
            assert_eq!(text.parse::<Type>(), Err(error), "{text:?}");
        }
    }

    fn matrix(element: Scalar, rows: u16, columns: u16) -> Type {
        Type::Matrix {
            element,
            sides: Sides { rows, columns },
        }
    }

    #[test]
    fn reads_two_lengths() {
        let read = "f32[2][3]".parse::<Type>();
        assert_eq!(read, Ok(matrix(Scalar::F32, 2, 3)));
        assert_eq!(read.unwrap().width(), Some(24));
    }

    #[test]
    fn refuses_the_matrix_fuzz_inputs() {
        let over = include_str!("../../../oracles/fuzz/types_sample/matrix_over");
        let three = include_str!("../../../oracles/fuzz/types_sample/three_lengths");
        assert_eq!(over.parse::<Type>(), Err(Error::Matrix));
        assert_eq!(three.parse::<Type>(), Err(Error::Lengths));
    }

    #[test]
    fn gives_the_width_of_the_largest_matrix() {
        let max = matrix(Scalar::Uuid, u16::MAX, u16::MAX);
        assert_eq!(max.width(), Some(16 * 65_535 * 65_535));
        assert_eq!(matrix(Scalar::U8, 0, u16::MAX).width(), Some(0));
    }

    #[test]
    fn stays_eight_bytes() {
        assert_eq!(size_of::<Type>(), 8);
    }

    #[test]
    fn states_each_error() {
        let cases = [
            (
                Error::Syntax,
                "expected a sample type such as f64, f32[3], list<u8, 16>, string, or \
                 bytes",
                "Use one of the forms that the message names, with exact case and a \
                 space only after the comma of a list",
            ),
            (
                Error::Element,
                "expected a scalar element: bool, i8, i16, i32, i64, u8, u16, u32, \
                 u64, f32, f64, timestamp, duration, or uuid",
                "Use a scalar as the element, such as f32[3] or list<u8, 16>",
            ),
            (
                Error::Count,
                "expected a count of 0 to 4294967295, with no leading zero",
                "Write the count in plain digits, such as 16",
            ),
            (
                Error::Lengths,
                "expected one or two array lengths",
                "Use one or two array lengths, such as f32[3] or f32[2][3]",
            ),
            (
                Error::Matrix,
                "expected each matrix length of 0 to 65535, with no leading zero",
                "Write each matrix length in plain digits up to 65535, such as \
                 f32[2][3], or use an array",
            ),
        ];
        for (error, message, fix) in cases {
            assert_eq!(error.to_string(), message);
            assert_eq!(error.fix(), fix);
            assert_stated(message, fix);
        }
    }

    fn values() -> impl Strategy<Value = Value> {
        prop_oneof![
            any::<bool>().prop_map(Value::Bool),
            any::<i8>().prop_map(Value::I8),
            any::<i16>().prop_map(Value::I16),
            any::<i32>().prop_map(Value::I32),
            any::<i64>().prop_map(Value::I64),
            any::<u8>().prop_map(Value::U8),
            any::<u16>().prop_map(Value::U16),
            any::<u32>().prop_map(Value::U32),
            any::<u64>().prop_map(Value::U64),
            any::<f32>().prop_map(Value::F32),
            any::<f64>().prop_map(Value::F64),
            any::<i64>().prop_map(|nanos| Value::Stamp(Stamp::from_nanos(nanos))),
            any::<i64>().prop_map(|nanos| Value::Span(Span::from_nanos(nanos))),
            any::<[u8; 16]>().prop_map(Value::Uuid),
        ]
    }

    /// The little-endian bytes of `value`.
    fn little(value: Value) -> Vec<u8> {
        match value {
            Value::Bool(value) => vec![u8::from(value)],
            Value::I8(value) => value.to_le_bytes().to_vec(),
            Value::I16(value) => value.to_le_bytes().to_vec(),
            Value::I32(value) => value.to_le_bytes().to_vec(),
            Value::I64(value) => value.to_le_bytes().to_vec(),
            Value::U8(value) => value.to_le_bytes().to_vec(),
            Value::U16(value) => value.to_le_bytes().to_vec(),
            Value::U32(value) => value.to_le_bytes().to_vec(),
            Value::U64(value) => value.to_le_bytes().to_vec(),
            Value::F32(value) => value.to_le_bytes().to_vec(),
            Value::F64(value) => value.to_le_bytes().to_vec(),
            Value::Stamp(value) => value.nanos().to_le_bytes().to_vec(),
            Value::Span(value) => value.nanos().to_le_bytes().to_vec(),
            Value::Uuid(value) => value.to_vec(),
        }
    }

    #[test]
    fn gives_the_scalar_of_each_value() {
        let values = [
            Value::Bool(false),
            Value::I8(0),
            Value::I16(0),
            Value::I32(0),
            Value::I64(0),
            Value::U8(0),
            Value::U16(0),
            Value::U32(0),
            Value::U64(0),
            Value::F32(0.0),
            Value::F64(0.0),
            Value::Stamp(Stamp::EPOCH),
            Value::Span(Span::from_nanos(0)),
            Value::Uuid([0; 16]),
        ];
        assert_eq!(values.map(Value::scalar).as_slice(), SCALARS);
    }

    #[test]
    fn reads_a_bool_byte_of_0_as_false() {
        assert_eq!(Value::read(Scalar::Bool, &[0]), Value::Bool(false));
    }

    proptest! {
        #[test]
        fn writes_the_little_endian_bytes_and_reads_them_back(value in values()) {
            let mut sample = vec![0xAA; value.scalar().width()];
            value.write(&mut sample);
            prop_assert_eq!(&sample, &little(value));
            let read = Value::read(value.scalar(), &sample);
            prop_assert_eq!(read.scalar(), value.scalar());
            let mut again = vec![0; sample.len()];
            read.write(&mut again);
            prop_assert_eq!(again, sample);
        }

        #[test]
        fn panics_at_a_sample_of_another_width(value in values(), more in 1..8_usize) {
            let width = value.scalar().width();
            for len in [width - 1, width + more] {
                let write = std::panic::catch_unwind(|| value.write(&mut vec![0; len]));
                let read = std::panic::catch_unwind(|| {
                    Value::read(value.scalar(), &vec![0; len]).scalar()
                });
                let expected = format!("a sample of {width} bytes, not {len}");
                for payload in [write, read.map(|_| ())] {
                    let payload = payload.expect_err("a panic");
                    let message = payload.downcast_ref::<String>().map(String::as_str);
                    prop_assert_eq!(message, Some(expected.as_str()), "{:?}", value);
                }
            }
        }

        #[test]
        fn reads_a_bool_byte_other_than_0_as_true(byte in 1_u8..) {
            prop_assert_eq!(Value::read(Scalar::Bool, &[byte]), Value::Bool(true));
        }

        #[test]
        fn reads_back_the_text_of_each_type(sample in types()) {
            prop_assert_eq!(sample.to_string().parse(), Ok(sample));
        }

        #[test]
        fn shows_a_text_that_reads_as_that_text(
            text in "(list<)?[a-z]{0,2}[0-9]{0,2}(\\[|, )?[0-9]{0,3}(\\]|>)?\
                     (\\[[0-9]{0,3}\\]?)?"
        ) {
            if let Ok(sample) = text.parse::<Type>() {
                prop_assert_eq!(sample.to_string(), text);
            }
        }
    }
}
