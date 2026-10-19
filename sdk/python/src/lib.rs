//! The native module `foundation._native` of the Python SDK: the Rust SDK `sdk`, for
//! Python.

use std::mem;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Mutex, PoisonError};

use pyo3::exceptions::{
    PyConnectionError, PyOSError, PyRuntimeError, PyTypeError, PyValueError,
};
use pyo3::panic::PanicException;
use pyo3::prelude::*;
use types::ed25519::{PrivateKey, PublicKey};
use types::name::Name;

#[cfg(feature = "sim")]
mod sim;
#[cfg(test)]
mod tests;

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(connect, module)?)?;
    module.add_class::<Client>()?;
    #[cfg(feature = "sim")]
    sim::add(module)?;
    Ok(())
}

/// Dials the node at `address` (`ip:port`, UDP), and blocks until it admits the first
/// hello of `subject`. The client then renews the hello until it closes.
///
/// `node` is the `uuid.UUID` key of the node, `public_key` the 32 bytes of the key
/// that the node proves, and `private_key` the 32 bytes of a key that the spec lists
/// for `subject`. Each of `address`, the pair (`node`, `public_key`), and the pair
/// (`subject`, `private_key`) comes from the arguments, else from the node that runs on
/// the data directory `data`: that node gives its address and keys, and its first admin
/// gives the subject. `data` is a `str` or an `os.PathLike`, the `--data` of
/// `foundation start`, by default `foundation-data`. With an `address`, the data
/// directory counts only when its node is at that address: `127.0.0.1:p` matches a node
/// bound on `0.0.0.0:p`. Other Python threads run while it waits.
///
/// Raises `TypeError` for an argument of a wrong type, `ValueError` for an argument
/// that does not parse or a pair given in half, `RuntimeError` when a thread of the
/// client or of the read of `data` cannot start, `OSError` when the local UDP socket
/// cannot bind, `ConnectionError` when no source gives a value, the data directory
/// cannot be read, the client cannot reach the node, or the node refuses the subject,
/// and `PanicException` when a thread of the client or of the read of `data` panicked.
#[pyfunction]
#[pyo3(signature = (
    address=None, *, node=None, public_key=None, subject=None, private_key=None,
    data=None,
))]
fn connect(
    py: Python<'_>,
    address: Option<&str>,
    node: Option<&Bound<'_, PyAny>>,
    public_key: Option<&[u8]>,
    subject: Option<&str>,
    private_key: Option<&[u8]>,
    data: Option<PathBuf>,
) -> PyResult<Client> {
    let keys = pair(["node", "public_key"], node, public_key)?;
    let subject = pair(["subject", "private_key"], subject, private_key)?;
    let address = address.map(parse_address).transpose()?;
    let data = data.map(parse_data).transpose()?;
    let keys = keys.map(|(node, public_key)| node_keys(node, public_key));
    let subject = subject.map(|(name, key)| parse_subject(name, key));
    let given = Given {
        address,
        keys: keys.transpose()?,
        subject: subject.transpose()?,
    };
    let (address, node, subject) = match given {
        Given {
            address: Some(address),
            keys: Some((key, public_key)),
            subject: Some(subject),
        } => (address, (key, public_key), subject),
        given => {
            let data = data.unwrap_or_else(|| PathBuf::from("foundation-data"));
            given.sourced(&data, local(py, &data)?)?
        }
    };
    let (key, public_key) = node;
    let node = sdk::Node {
        addresses: vec![address],
        key,
        public_key,
    };
    let client = py
        .detach(|| sdk::Client::connect(&node, subject))
        .map_err(|e| error(&e))?;
    Ok(Client {
        session: Mutex::new(Some(client)),
        process: process::id(),
        address,
    })
}

/// The key and the public key of a node.
type Keys = (types::node::Key, PublicKey);

/// What the arguments of [`connect`] give.
struct Given {
    address: Option<SocketAddr>,
    keys: Option<Keys>,
    subject: Option<sdk::Subject>,
}

impl Given {
    /// The address, the node's keys, and the subject, each from the arguments, else
    /// from `local`, the node that runs on the data directory `data`, if any.
    fn sourced(
        self,
        data: &Path,
        local: Option<(sdk::Node, Option<sdk::Subject>)>,
    ) -> PyResult<(SocketAddr, Keys, sdk::Subject)> {
        let (local, other) = match local {
            Some((node, _))
                if self
                    .address
                    .is_some_and(|address| !node.addresses.contains(&address)) =>
            {
                (None, node.addresses.first().copied())
            }
            local => (local, None),
        };
        let at = self.address.or_else(|| {
            let (node, _) = local.as_ref()?;
            node.addresses.first().copied()
        });
        let Some(address) = at else {
            return Err(PyConnectionError::new_err(format!(
                "no node runs in {}: pass the address of the node, or data= with the \
                 --data of foundation start",
                data.display()
            )));
        };
        let unsourced = |what: &str, pass: &str| {
            PyConnectionError::new_err(match other {
                Some(other) => format!(
                    "no source gives {what} at {address}: {} holds the node at \
                     {other}; pass {pass}, or connect to {other}",
                    data.display()
                ),
                None => format!(
                    "no source gives {what} at {address}: pass {pass}, or data= with \
                     the --data of foundation start on the node's host"
                ),
            })
        };
        let (node, admin) = local.unzip();
        let keys = self
            .keys
            .or_else(|| node.map(|node| (node.key, node.public_key)))
            .ok_or_else(|| {
                unsourced("the keys of the node", "node= and public_key=")
            })?;
        let subject = match (self.subject, admin) {
            (Some(subject), _) | (None, Some(Some(subject))) => subject,
            (None, Some(None)) => {
                return Err(PyConnectionError::new_err(format!(
                    "{} holds no admin.key that a node wrote",
                    data.display()
                )));
            }
            (None, None) => {
                return Err(unsourced(
                    "the subject for the node",
                    "subject= and private_key=",
                ));
            }
        };
        Ok((address, keys, subject))
    }
}

/// The node that runs on the data directory `data` and its first admin, or `None`
/// when no node runs on it. Other Python threads run while it reads.
fn local(
    py: Python<'_>,
    data: &Path,
) -> PyResult<Option<(sdk::Node, Option<sdk::Subject>)>> {
    match py.detach(|| sdk::Node::local(data)) {
        Ok(local) => Ok(Some(local)),
        Err(sdk::Error::Local(sector::local::Error::NotRunning)) => Ok(None),
        Err(sdk::Error::Local(sector::local::Error::Version {
            file,
            found,
            expected,
        })) => Err(PyConnectionError::new_err(format!(
            "{} holds format version {}, and this package reads version {}: install \
             the package of the node's version",
            data.join("data").join(file).display(),
            char::from(found).escape_default(),
            char::from(expected).escape_default(),
        ))),
        Err(sdk::Error::Local(sector::local::Error::Files(e))) => {
            Err(PyConnectionError::new_err(format!(
                "cannot read {}: {e}",
                data.join("data").display()
            )))
        }
        Err(e) => Err(error(&e)),
    }
}

/// The values of a pair of arguments `names`, or `None` when neither is given.
/// Raises `ValueError` when only one is.
fn pair<A, B>(
    names: [&str; 2],
    a: Option<A>,
    b: Option<B>,
) -> PyResult<Option<(A, B)>> {
    match (a, b) {
        (Some(a), Some(b)) => Ok(Some((a, b))),
        (None, None) => Ok(None),
        _ => Err(PyValueError::new_err(format!(
            "{} and {} go together: give both or neither",
            names[0], names[1]
        ))),
    }
}

/// The socket address of `address`.
fn parse_address(address: &str) -> PyResult<SocketAddr> {
    address.parse().map_err(|e| {
        PyValueError::new_err(format!("the address {address:?} is not ip:port: {e}"))
    })
}

/// The data directory of the argument `data`, which names a file only with no NUL byte.
fn parse_data(data: PathBuf) -> PyResult<PathBuf> {
    if data.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(PyValueError::new_err("data: the path holds a NUL byte"));
    }
    Ok(data)
}

/// The node's keys of the arguments `node` and `public_key`.
fn node_keys(node: &Bound<'_, PyAny>, public_key: &[u8]) -> PyResult<Keys> {
    let public_key = PublicKey::new(bytes("public_key", public_key)?)
        .map_err(|e| PyValueError::new_err(format!("public_key: {e}")))?;
    Ok((key(node)?, public_key))
}

/// The subject of the arguments `subject` and `private_key`.
fn parse_subject(subject: &str, private_key: &[u8]) -> PyResult<sdk::Subject> {
    Ok(sdk::Subject {
        name: subject
            .parse::<Name>()
            .map_err(|e| PyValueError::new_err(format!("subject: {e}")))?,
        private_key: PrivateKey(bytes("private_key", private_key)?),
    })
}

/// A program's session with one node. A `with` block closes it at its end. A client
/// that the program drops without a close ends its session and does not wait. A
/// process that forks from the program does not own the client: in it, a close gives
/// `None`, and neither a close nor a drop ends the session.
#[pyclass(frozen, module = "foundation")]
struct Client {
    session: Mutex<Option<sdk::Client>>,
    /// The process that connected. A child that it forks has no thread of the client,
    /// and can copy the lock as held.
    process: u32,
    /// The address that `connect` dialed.
    address: SocketAddr,
}

#[pymethods]
impl Client {
    /// The address that `connect` dialed, as `ip:port`.
    #[getter]
    fn address(&self) -> String {
        self.address.to_string()
    }

    /// Closes the session, and blocks until the close goes out, the transport drains
    /// each close (up to 3 s), and the client's thread ends. A close during another
    /// close waits for it to end, and gives `None`, as a close after it does.
    ///
    /// Raises `ConnectionError` with the cause when the session ended before the close,
    /// and `PanicException` when the client's thread panicked. Only the close that ends
    /// the session raises.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        if process::id() != self.process {
            return Ok(());
        }
        // The lock is taken without the GIL, which the closing thread needs to return.
        py.detach(|| {
            let mut client =
                self.session.lock().unwrap_or_else(PoisonError::into_inner);
            client.take().map_or(Ok(()), sdk::Client::close)
        })
        .map_err(|e| error(&e))
    }

    fn __enter__(this: Py<Self>) -> Py<Self> {
        this
    }

    /// Closes the client. When the block raised, an error of the close is a note on
    /// the error of the block; else it raises.
    #[pyo3(signature = (_kind, raised, _traceback))]
    fn __exit__(
        &self,
        py: Python<'_>,
        _kind: &Bound<'_, PyAny>,
        raised: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        match self.close(py) {
            Err(close) if !raised.is_none() => {
                raised
                    .call_method1("add_note", (format!("the close raised {close}"),))?;
                Ok(())
            }
            closed => closed,
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if process::id() != self.process {
            // `get_mut` takes no lock, so a lock that the fork copied as held does not
            // block.
            let session = self
                .session
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner);
            #[expect(
                clippy::mem_forget,
                reason = "the drop of the session wakes the parent's runtime and \
                          detaches its thread, which this process does not have"
            )]
            mem::forget(session.take());
        }
    }
}

/// The node key of the `uuid.UUID` `node`.
fn key(node: &Bound<'_, PyAny>) -> PyResult<types::node::Key> {
    let uuid = node.py().import("uuid")?.getattr("UUID")?;
    if !node.is_instance(&uuid)? {
        let kind = node.get_type().name()?;
        return Err(PyTypeError::new_err(format!(
            "node is {kind}, not uuid.UUID"
        )));
    }
    Ok(types::node::Key::from_u128(node.getattr("int")?.extract()?))
}

/// The 32 bytes of the argument `name`.
fn bytes(name: &str, value: &[u8]) -> PyResult<[u8; 32]> {
    value.try_into().map_err(|_short| {
        PyValueError::new_err(format!("{name} is {} bytes, not 32", value.len()))
    })
}

/// The Python exception of `error`.
fn error(error: &sdk::Error) -> PyErr {
    let text = error.to_string();
    match error {
        sdk::Error::Thread { .. } | sdk::Error::Closed => PyRuntimeError::new_err(text),
        sdk::Error::Bind(_) => PyOSError::new_err(text),
        sdk::Error::Session(_)
        | sdk::Error::Stream(_)
        | sdk::Error::Local(_)
        | sdk::Error::Decode { .. } => PyConnectionError::new_err(text),
        sdk::Error::Panicked => PanicException::new_err(text),
        sdk::Error::Value { .. } => PyTypeError::new_err(text),
    }
}
