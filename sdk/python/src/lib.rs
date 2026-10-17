//! The native module `foundation._native` of the Python SDK: the Rust SDK `sdk`, for
//! Python.

use std::net::SocketAddr;
use std::sync::{Mutex, PoisonError};

use pyo3::exceptions::{PyConnectionError, PyValueError};
use pyo3::panic::PanicException;
use pyo3::prelude::*;
use types::ed25519::{PrivateKey, PublicKey};
use types::name::Name;

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(connect, module)?)?;
    module.add_class::<Client>()
}

/// Dials the node at `address` (`ip:port`, UDP), and blocks until it admits the first
/// hello of `subject`. The client then renews the hello until it closes.
///
/// `node` is the 128-bit key of the node, `public_key` the 32 bytes of the key that the
/// node proves, and `key` the 32 bytes of a private key that the spec lists for
/// `subject`. Other Python threads run while it waits.
///
/// Raises `ValueError` for an argument that does not parse, `ConnectionError` when the
/// client cannot start, bind, or reach the node, or when the node refuses the subject,
/// and `PanicException` when the client's thread panicked.
#[pyfunction]
#[pyo3(signature = (address, *, node, public_key, subject, key))]
fn connect(
    py: Python<'_>,
    address: &str,
    node: u128,
    public_key: &[u8],
    subject: &str,
    key: &[u8],
) -> PyResult<Client> {
    let address: SocketAddr = address.parse().map_err(|e| {
        PyValueError::new_err(format!("the address {address:?} is not ip:port: {e}"))
    })?;
    let public_key = PublicKey::new(bytes("public_key", public_key)?)
        .map_err(|e| PyValueError::new_err(format!("public_key: {e}")))?;
    let node = sdk::Node {
        addresses: vec![address],
        key: types::node::Key::from_u128(node),
        public_key,
    };
    let subject = sdk::Subject {
        name: subject
            .parse::<Name>()
            .map_err(|e| PyValueError::new_err(format!("subject: {e}")))?,
        private_key: PrivateKey(bytes("key", key)?),
    };
    let client = py
        .detach(|| sdk::Client::connect(&node, subject))
        .map_err(error)?;
    Ok(Client(Mutex::new(Some(client))))
}

/// A program's session with one node. A `with` block closes it at its end. A client
/// that the program drops without a close ends its session and does not wait.
#[pyclass(frozen, module = "foundation")]
struct Client(Mutex<Option<sdk::Client>>);

#[pymethods]
impl Client {
    /// Closes the session, and blocks until the close goes out and the client's thread
    /// ends. A second close does nothing.
    ///
    /// Raises `ConnectionError` with the cause when the session ended before the close,
    /// and `PanicException` when the client's thread panicked.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let client = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        match client {
            Some(client) => py.detach(|| client.close()).map_err(error),
            None => Ok(()),
        }
    }

    fn __enter__(this: Py<Self>) -> Py<Self> {
        this
    }

    /// Closes the client, as `close` does. An error of the close raises, with the
    /// exception of the block as its context.
    #[pyo3(signature = (*_exception))]
    fn __exit__(&self, py: Python<'_>, _exception: &Bound<'_, PyAny>) -> PyResult<()> {
        self.close(py)
    }
}

/// The 32 bytes of the argument `name`.
fn bytes(name: &str, value: &[u8]) -> PyResult<[u8; 32]> {
    value.try_into().map_err(|_short| {
        PyValueError::new_err(format!("{name} is {} bytes, not 32", value.len()))
    })
}

/// The Python exception of `error`.
fn error(error: sdk::Error) -> PyErr {
    match error {
        sdk::Error::Panicked => PanicException::new_err(error.to_string()),
        error => PyConnectionError::new_err(error.to_string()),
    }
}
