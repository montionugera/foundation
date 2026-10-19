//! `foundation.Writer`: a writer of `sdk`, for Python.

use std::fmt::Display;
use std::mem;
use std::ops::Range;
use std::process;
use std::sync::{Mutex, PoisonError};

use numpy::datetime::{Datetime, Timedelta, units::Nanoseconds};
use numpy::{
    Element, PyArray1, PyArrayDescrMethods, PyArrayMethods, PyUntypedArray,
    PyUntypedArrayMethods,
};
use pyo3::exceptions::{
    PyKeyError, PyOverflowError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::types::{PyBool, PyDict, PyInt, PyString, PyTuple};
use sdk::write::{Outcome, Place, Refusal};
use types::frame;
use types::sample::{Scalar, Type, Value};
use types::time::{Span, Stamp};

use crate::{DataLost, Rejected, error, owned};

/// What a write does with a frame that the node took no room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loss {
    /// Log a warning on the `foundation` logger.
    Warn,
    /// Raise `DataLost`.
    Raise,
    /// Do nothing.
    Ignore,
}

impl Loss {
    /// The loss of the argument `on_loss`.
    pub(crate) fn parse(on_loss: &str) -> PyResult<Self> {
        match on_loss {
            "warn" => Ok(Self::Warn),
            "raise" => Ok(Self::Raise),
            "ignore" => Ok(Self::Ignore),
            other => Err(PyValueError::new_err(format!(
                "on_loss is {other:?}, not \"warn\", \"raise\", or \"ignore\""
            ))),
        }
    }
}

/// A writer of channels of one index at its home, which `Client.open_writer` opens. A
/// `with` block closes it at its end. Its calls take turns: a call waits, with other
/// Python threads running, while another call of the writer runs. A process that forks
/// from the program does not own the writer: in it, each call but a close raises
/// `RuntimeError`, and a close gives `None` and ends nothing.
#[pyclass(frozen, module = "foundation")]
pub(crate) struct Writer {
    state: Mutex<Option<State>>,
    /// The process that opened the writer.
    process: u32,
    places: Vec<Place>,
    index: usize,
    loss: Loss,
}

/// The writer of `sdk` and the buffers that its calls fill, kept so a warm call
/// allocates only the list of its series.
struct State {
    writer: sdk::write::Writer,
    values: Vec<(usize, Value)>,
    bytes: Vec<u8>,
    /// The place of each series of `write_series`, and its bytes in `bytes`.
    series: Vec<(usize, Range<usize>)>,
}

impl Writer {
    /// The Python writer of `writer`, opened in `process`.
    pub(crate) fn new(writer: sdk::write::Writer, process: u32, loss: Loss) -> Self {
        Self {
            places: writer.places().to_vec(),
            index: writer.index(),
            state: Mutex::new(Some(State {
                writer,
                values: Vec::new(),
                bytes: Vec::new(),
                series: Vec::new(),
            })),
            process,
            loss,
        }
    }

    /// Runs `call` on the state, which it waits for with other Python threads running.
    /// Raises `RuntimeError` in a process that the program forked, without the lock
    /// that the fork can copy as held, and after a close.
    fn with<T>(
        &self,
        py: Python<'_>,
        call: impl FnOnce(&mut State) -> PyResult<T>,
    ) -> PyResult<T> {
        owned(self.process)?;
        let mut state = self
            .state
            .lock_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner);
        let state = state
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("the writer is closed"))?;
        call(state)
    }

    /// The place of the key `key`: a channel name or a place.
    fn place(&self, key: &Bound<'_, PyAny>) -> PyResult<usize> {
        if let Ok(name) = key.cast::<PyString>() {
            let name = name.to_str()?;
            return self
                .places
                .iter()
                .position(|place| place.name.as_str() == name)
                .ok_or_else(|| {
                    let names: Vec<_> =
                        self.places.iter().map(|p| p.name.as_str()).collect();
                    PyKeyError::new_err(format!(
                        "the writer has no place {name}: its places are {}",
                        names.join(", ")
                    ))
                });
        }
        if let Ok(place) = key.cast::<PyInt>() {
            return place
                .extract::<usize>()
                .ok()
                .filter(|&place| place < self.places.len())
                .ok_or_else(|| {
                    PyValueError::new_err(format!(
                        "the writer has no place {place}: it has {} places",
                        self.places.len()
                    ))
                });
        }
        Err(PyTypeError::new_err(format!(
            "a key is a channel name or a place, not {}",
            key.get_type().name()?
        )))
    }

    /// The one place other than the index, for a value with no key.
    fn only(&self) -> PyResult<usize> {
        match self.places.len() {
            2 => Ok(1 - self.index),
            count => Err(PyTypeError::new_err(format!(
                "the writer has {} places other than the index: pass a dict from each \
                 channel name to its value",
                count - 1
            ))),
        }
    }

    /// Returns `None` for a frame that the node took, and raises for each other
    /// outcome, or the exception of `error`. `places` are the places of the frame.
    fn outcome(
        &self,
        py: Python<'_>,
        outcome: Result<Outcome, sdk::Error>,
        places: impl Iterator<Item = usize>,
    ) -> PyResult<()> {
        match outcome.map_err(|e| error(py, &e))? {
            Outcome::Taken { .. } => Ok(()),
            Outcome::Refused(refusal) => {
                let (code, text) = rejection(refusal);
                let rejected =
                    Rejected::new_err(format!("the node refused the frame: {text}"));
                rejected.value(py).setattr("code", code)?;
                Err(rejected)
            }
            Outcome::Lost { range } => self.lost(py, range, places),
        }
    }

    /// Does what [`Loss`] says with the loss of the samples `range` of the series of
    /// `places`.
    fn lost(
        &self,
        py: Python<'_>,
        range: frame::Range,
        places: impl Iterator<Item = usize>,
    ) -> PyResult<()> {
        if self.loss == Loss::Ignore {
            return Ok(());
        }
        let mut places: Vec<_> = places.collect();
        places.sort_unstable();
        places.dedup();
        let names: Vec<_> = places
            .iter()
            .map(|&place| format!("{} ({})", self.places[place].name, range.count))
            .collect();
        let text = format!("samples were lost: {}", names.join(", "));
        if self.loss == Loss::Warn {
            py.import("logging")?
                .call_method1("getLogger", ("foundation",))?
                .call_method1("warning", (text,))?;
            return Ok(());
        }
        let end = u128::from(range.seq) + u128::from(range.count);
        let seqs = py
            .import("builtins")?
            .getattr("range")?
            .call1((range.seq, end))?;
        let lost = PyDict::new(py);
        for place in places {
            lost.set_item(self.places[place].name.as_str(), &seqs)?;
        }
        let raised = DataLost::new_err(text);
        raised.value(py).setattr("seqs", lost)?;
        Err(raised)
    }
}

#[pymethods]
impl Writer {
    /// The name and the type of each place, in place order, the type as the spec
    /// writes it (`f64`, `timestamp`).
    #[getter]
    fn places<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let places = self
            .places
            .iter()
            .map(|place| (place.name.as_str(), place.data_type.to_string()));
        PyTuple::new(py, places)
    }

    /// The place of the index, whose samples are the stamps of each frame.
    #[getter]
    fn index(&self) -> usize {
        self.index
    }

    /// Writes one frame of one sample of each value, and blocks until the node took
    /// it. `values` is a `dict` from a channel name or a place to a value, or one value
    /// when the writer has one place other than the index. A value at the index is the
    /// stamp of the frame, in nanoseconds since the UNIX epoch; with none, the node
    /// stamps the frame. A value is a `bool` for `bool`, an `int` (or an object with
    /// `__index__`) for an integer type and for `timestamp` and `duration`
    /// (nanoseconds), a `float` or an `int` for `f32` and `f64`, and a `uuid.UUID` for
    /// `uuid`. Other Python threads run while it waits.
    ///
    /// Raises `TypeError` for a value that is not of its place's type, a place of a
    /// type that is not a scalar, a key that is not a name or a place, or one value
    /// with no key for a writer of more places; `OverflowError` for a number outside
    /// its type; `KeyError` for a name of no place; and `ValueError` for no value, a
    /// place past the places, or two values of one channel. With each of these, nothing
    /// was sent and the writer goes on. Raises `Rejected` when the node refused the
    /// frame, and `DataLost` for a lost frame with `on_loss="raise"`; the writer goes
    /// on after both. Raises `Refused` or `ConnectionError` when the stream or the
    /// session failed, after which each call raises it again; `RuntimeError` after a
    /// close or in a forked process; and `PanicException` when the client's thread
    /// panicked.
    fn write(&self, py: Python<'_>, values: &Bound<'_, PyAny>) -> PyResult<()> {
        self.with(py, |state| {
            state.values.clear();
            if let Ok(values) = values.cast::<PyDict>() {
                for (key, object) in values.iter() {
                    let place = self.place(&key)?;
                    state
                        .values
                        .push((place, value(&self.places[place], &object)?));
                }
            } else {
                let place = self.only()?;
                state
                    .values
                    .push((place, value(&self.places[place], values)?));
            }
            let State { writer, values, .. } = state;
            let outcome = py.detach(|| writer.write(values));
            let index = (!values.is_empty()).then_some(self.index);
            self.outcome(
                py,
                outcome,
                values.iter().map(|&(place, _)| place).chain(index),
            )
        })
    }

    /// Writes one frame of each series of `series`, a `dict` from a channel name or a
    /// place to a one-dimensional `numpy.ndarray`, and blocks until the node took it. Each
    /// array has the same length, and the series of the index holds the stamps. Each
    /// array has the dtype of its place in native byte order: `bool`, `int8` to
    /// `uint64`, `float32`, `float64`, and `int64` or `datetime64[ns]` for
    /// `timestamp`, `int64` or `timedelta64[ns]` for `duration`. The writer casts
    /// nothing, and copies each array before other Python threads run.
    ///
    /// Raises `TypeError` for a value that is not an array of one dimension of its
    /// place's dtype (its text names the place, the dtypes that it takes, the dtype
    /// given, and the fix, such as `pass a.astype("float64")`), or a place of `uuid` or
    /// of a type that is not a scalar, and `ValueError` for arrays of different
    /// lengths, no series, no series of the index, a frame over the largest block, or
    /// an array that another call writes to. With each of these, nothing was sent and
    /// the writer goes on. Raises each other error as `write` raises it.
    fn write_series(&self, py: Python<'_>, series: &Bound<'_, PyDict>) -> PyResult<()> {
        self.with(py, |state| {
            state.bytes.clear();
            state.series.clear();
            let mut count: Option<(usize, usize)> = None;
            for (key, array) in series.iter() {
                let place = self.place(&key)?;
                let start = state.bytes.len();
                let length = copy(&self.places[place], &array, &mut state.bytes)?;
                match count {
                    Some((first, samples)) if samples != length => {
                        return Err(PyValueError::new_err(format!(
                            "the series of {} holds {length} samples, not the {samples} \
                             of {}",
                            self.places[place].name, self.places[first].name
                        )));
                    }
                    Some(_) => {}
                    None => count = Some((place, length)),
                }
                state.series.push((place, start..state.bytes.len()));
            }
            let samples = count.map_or(0, |(_, samples)| samples);
            let samples = u32::try_from(samples).map_err(|_too_many| {
                PyValueError::new_err(format!(
                    "the series hold {samples} samples, over the {} of one frame",
                    u32::MAX
                ))
            })?;
            let State {
                writer,
                bytes,
                series,
                ..
            } = state;
            let parts: Vec<_> = series
                .iter()
                .map(|(place, range)| (*place, &bytes[range.clone()]))
                .collect();
            let outcome = py.detach(|| writer.write_series(samples, &parts));
            self.outcome(py, outcome, series.iter().map(|&(place, _)| place))
        })
    }

    /// Finishes the stream, and blocks until each frame that the node took is on disk
    /// and the node finished the stream. Control ends then. A close waits for a call
    /// that runs, with other Python threads running. A close after a close gives
    /// `None`, and in a forked process it gives `None` and ends nothing.
    ///
    /// Raises `Refused` or `ConnectionError` when the stream or the session failed,
    /// `RuntimeError` when the client closed, and `PanicException` when the client's
    /// thread panicked.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        if process::id() != self.process {
            return Ok(());
        }
        let state = self
            .state
            .lock_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        state.map_or(Ok(()), |state| {
            py.detach(|| state.writer.close())
                .map_err(|e| error(py, &e))
        })
    }

    fn __enter__(this: Py<Self>) -> Py<Self> {
        this
    }

    /// Closes the writer. When the block raised, an error of the close is a note on
    /// the error of the block; else it raises.
    #[pyo3(signature = (_kind, raised, _traceback))]
    fn __exit__(
        &self,
        py: Python<'_>,
        _kind: &Bound<'_, PyAny>,
        raised: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        crate::exit(self.close(py), raised)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if process::id() != self.process {
            let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
            #[expect(
                clippy::mem_forget,
                reason = "the drop of the writer sends to the parent's client thread, \
                          which this process does not have"
            )]
            mem::forget(state.take());
        }
    }
}

/// The code and the text of the frame refusal `refusal`.
fn rejection(refusal: Refusal) -> (&'static str, &'static str) {
    match refusal {
        Refusal::Waiting => ("waiting", "another writer holds control"),
        Refusal::Reserved => (
            "reserved",
            "control is held for a writer from before a restart",
        ),
        Refusal::Expired => (
            "expired",
            "the writer's control lease ran out: open the writer again",
        ),
        Refusal::Order => ("order", "a stamp is not after the stamp before it"),
        Refusal::Codec => ("codec", "a series is not a series of its channel's type"),
        Refusal::Full => (
            "full",
            "the frame found no room in the log or no block of the node's pool: write \
             it again later",
        ),
        Refusal::Large => ("large", "the frame is too large for one write: split it"),
    }
}

/// The value of `object` for `place`.
fn value(place: &Place, object: &Bound<'_, PyAny>) -> PyResult<Value> {
    let Type::Scalar(scalar) = place.data_type else {
        return Err(PyTypeError::new_err(format!(
            "{} holds {}, which the writer takes no value of",
            place.name, place.data_type
        )));
    };
    if scalar == Scalar::Bool {
        return match object.cast::<PyBool>() {
            Ok(object) => Ok(Value::Bool(object.is_true())),
            Err(_) => Err(wrong(place, object)?),
        };
    }
    if object.is_instance_of::<PyBool>() {
        return Err(wrong(place, object)?);
    }
    Ok(match scalar {
        Scalar::I8 => Value::I8(int(place, object, i8::MIN, i8::MAX)?),
        Scalar::I16 => Value::I16(int(place, object, i16::MIN, i16::MAX)?),
        Scalar::I32 => Value::I32(int(place, object, i32::MIN, i32::MAX)?),
        Scalar::I64 => Value::I64(int(place, object, i64::MIN, i64::MAX)?),
        Scalar::U8 => Value::U8(int(place, object, u8::MIN, u8::MAX)?),
        Scalar::U16 => Value::U16(int(place, object, u16::MIN, u16::MAX)?),
        Scalar::U32 => Value::U32(int(place, object, u32::MIN, u32::MAX)?),
        Scalar::U64 => Value::U64(int(place, object, u64::MIN, u64::MAX)?),
        Scalar::Stamp => {
            Value::Stamp(Stamp::from_nanos(int(place, object, i64::MIN, i64::MAX)?))
        }
        Scalar::Span => {
            Value::Span(Span::from_nanos(int(place, object, i64::MIN, i64::MAX)?))
        }
        Scalar::F32 | Scalar::F64 => {
            let float = match object.extract::<f64>() {
                Ok(float) => float,
                Err(e) if e.is_instance_of::<PyOverflowError>(object.py()) => {
                    return Err(outside(place, object, "its range"));
                }
                Err(_) => return Err(wrong(place, object)?),
            };
            if scalar == Scalar::F64 {
                Value::F64(float)
            } else {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the cast rounds to the nearest f32, and a finite value \
                              that it makes infinite raises"
                )]
                let narrow = float as f32;
                if narrow.is_infinite() && float.is_finite() {
                    return Err(outside(place, object, "its range"));
                }
                Value::F32(narrow)
            }
        }
        Scalar::Uuid => {
            let uuid = object.py().import("uuid")?.getattr("UUID")?;
            if !object.is_instance(&uuid)? {
                return Err(wrong(place, object)?);
            }
            Value::Uuid(object.getattr("int")?.extract::<u128>()?.to_be_bytes())
        }
        Scalar::Bool => unreachable!("a bool returns above"),
    })
}

/// The integer of `object`, from `min` to `max`.
fn int<T>(place: &Place, object: &Bound<'_, PyAny>, min: T, max: T) -> PyResult<T>
where
    T: TryFrom<i128> + Display + Copy,
{
    let wide = match object.extract::<i128>() {
        Ok(wide) => wide,
        Err(e) if e.is_instance_of::<PyOverflowError>(object.py()) => i128::MAX,
        Err(_) => return Err(wrong(place, object)?),
    };
    T::try_from(wide)
        .map_err(|_outside| outside(place, object, &format!("{min} to {max}")))
}

/// The `TypeError` of `object`, a value of another type, for `place`.
fn wrong(place: &Place, object: &Bound<'_, PyAny>) -> PyResult<PyErr> {
    Ok(PyTypeError::new_err(format!(
        "{} holds {}, not {}",
        place.name,
        place.data_type,
        object.get_type().name()?
    )))
}

/// The `OverflowError` of `object`, a number outside `range`, for `place`.
fn outside(place: &Place, object: &Bound<'_, PyAny>, range: &str) -> PyErr {
    let number = object
        .str()
        .map_or_else(|_| "the value".to_owned(), |s| s.to_string());
    PyOverflowError::new_err(format!(
        "{} holds {}, and {number} is outside {range}",
        place.name, place.data_type
    ))
}

/// Copies the array `array` for `place` into `bytes` in the raw form of the place's
/// type, and gives its length.
fn copy(
    place: &Place,
    array: &Bound<'_, PyAny>,
    bytes: &mut Vec<u8>,
) -> PyResult<usize> {
    let Ok(array) = array.cast::<PyUntypedArray>() else {
        return Err(PyTypeError::new_err(format!(
            "{} takes a numpy.ndarray, not {}",
            place.name,
            array.get_type().name()?
        )));
    };
    if array.ndim() != 1 {
        return Err(PyTypeError::new_err(format!(
            "{} takes an array of one dimension, not {}",
            place.name,
            array.ndim()
        )));
    }
    let Type::Scalar(scalar) = place.data_type else {
        return Err(PyTypeError::new_err(format!(
            "{} holds {}, which the writer takes no series of",
            place.name, place.data_type
        )));
    };
    let copied = match scalar {
        Scalar::Bool => raw::<bool>(array, bytes)?,
        Scalar::I8 => raw::<i8>(array, bytes)?,
        Scalar::I16 => raw::<i16>(array, bytes)?,
        Scalar::I32 => raw::<i32>(array, bytes)?,
        Scalar::I64 => raw::<i64>(array, bytes)?,
        Scalar::U8 => raw::<u8>(array, bytes)?,
        Scalar::U16 => raw::<u16>(array, bytes)?,
        Scalar::U32 => raw::<u32>(array, bytes)?,
        Scalar::U64 => raw::<u64>(array, bytes)?,
        Scalar::F32 => raw::<f32>(array, bytes)?,
        Scalar::F64 => raw::<f64>(array, bytes)?,
        Scalar::Stamp => {
            raw::<i64>(array, bytes)? || raw::<Datetime<Nanoseconds>>(array, bytes)?
        }
        Scalar::Span => {
            raw::<i64>(array, bytes)? || raw::<Timedelta<Nanoseconds>>(array, bytes)?
        }
        Scalar::Uuid => {
            return Err(PyTypeError::new_err(format!(
                "{} holds uuid, which the writer takes no series of",
                place.name
            )));
        }
    };
    if copied {
        return Ok(array.len());
    }
    let dtype = array.dtype();
    let (takes, fix) = match (scalar, dtype.kind()) {
        (Scalar::Stamp, b'M') => ("int64 or datetime64[ns]", "datetime64[ns]"),
        (Scalar::Stamp, _) => ("int64 or datetime64[ns]", "int64"),
        (Scalar::Span, b'm') => ("int64 or timedelta64[ns]", "timedelta64[ns]"),
        (Scalar::Span, _) => ("int64 or timedelta64[ns]", "int64"),
        (Scalar::Bool, _) => ("bool", "bool"),
        (Scalar::I8, _) => ("int8", "int8"),
        (Scalar::I16, _) => ("int16", "int16"),
        (Scalar::I32, _) => ("int32", "int32"),
        (Scalar::I64, _) => ("int64", "int64"),
        (Scalar::U8, _) => ("uint8", "uint8"),
        (Scalar::U16, _) => ("uint16", "uint16"),
        (Scalar::U32, _) => ("uint32", "uint32"),
        (Scalar::U64, _) => ("uint64", "uint64"),
        (Scalar::F32, _) => ("float32", "float32"),
        (Scalar::F64, _) => ("float64", "float64"),
        (Scalar::Uuid, _) => unreachable!("a uuid returns above"),
    };
    Err(dtyped(place, takes, fix, &dtype))
}

/// The `TypeError` of an array of `dtype` for `place`, which takes the dtypes `takes`,
/// with the cast to `fix`.
fn dtyped(place: &Place, takes: &str, fix: &str, dtype: &Bound<'_, PyAny>) -> PyErr {
    PyTypeError::new_err(format!(
        "{} holds {} and takes an array of {takes}, not {dtype}: pass \
         a.astype(\"{fix}\")",
        place.name, place.data_type
    ))
}

/// Copies `array` into `bytes` when it is an array of `T`, and gives whether it is.
/// Raises `ValueError` when another call writes to the array.
fn raw<T: Raw>(
    array: &Bound<'_, PyUntypedArray>,
    bytes: &mut Vec<u8>,
) -> PyResult<bool> {
    let Ok(array) = array.cast::<PyArray1<T>>() else {
        return Ok(false);
    };
    let array = array
        .try_readonly()
        .map_err(|e| PyValueError::new_err(format!("cannot read the array: {e}")))?;
    bytes.reserve(array.len() * size_of::<T>());
    for &sample in array.as_array() {
        sample.put(bytes);
    }
    Ok(true)
}

/// An element of a `numpy` array in the raw form of a scalar.
trait Raw: Element + Copy {
    /// Adds the sample's raw form to `bytes`.
    fn put(self, bytes: &mut Vec<u8>);
}

macro_rules! raw {
    ($($kind:ty),*) => {$(
        impl Raw for $kind {
            fn put(self, bytes: &mut Vec<u8>) {
                bytes.extend_from_slice(&self.to_le_bytes());
            }
        }
    )*};
}

raw!(i8, i16, i32, i64, u8, u16, u32, u64, f32, f64);

impl Raw for bool {
    fn put(self, bytes: &mut Vec<u8>) {
        bytes.push(u8::from(self));
    }
}

impl Raw for Datetime<Nanoseconds> {
    fn put(self, bytes: &mut Vec<u8>) {
        i64::from(self).put(bytes);
    }
}

impl Raw for Timedelta<Nanoseconds> {
    fn put(self, bytes: &mut Vec<u8>) {
        i64::from(self).put(bytes);
    }
}
