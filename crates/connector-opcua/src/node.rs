//! The nodes of an OPC UA server, and their text form in a `node_id`.

use std::fmt;
use std::str::FromStr;

#[cfg(test)]
mod tests;

/// A node of an OPC UA server: a namespace by its index in the server's
/// `NamespaceArray`, and a key in that namespace. Its text form is
/// `ns=<index>;i=<number>` or `ns=<index>;s=<name>`, with no `ns=<index>;` for
/// namespace 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) namespace: u16,
    pub(crate) key: Key,
}

/// The key of a [`Node`] in its namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Number(u32),
    Name(Name),
}

/// The name of a node: 1 to [`NAME`] bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Name(String);

/// The most bytes in the name of a node. A request with a name past the chunks of one
/// message closes its channel.
pub(crate) const NAME: usize = 4096;

impl Name {
    /// Gives `text` as a name, or `None` when it is empty or has more than [`NAME`]
    /// bytes.
    pub(crate) fn new(text: &str) -> Option<Self> {
        (1..=NAME)
            .contains(&text.len())
            .then(|| Self(text.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a text is not a [`Node`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// It names its namespace by URI (`nsu=`).
    Uri,
    /// Its key is a GUID (`g=`) or opaque (`b=`).
    Form,
    /// Another text, or an index, a number, or a name out of range.
    Syntax,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Uri => {
                "a namespace by URI (nsu=) is not supported: give its index (ns=)"
            }
            Self::Form => {
                "a GUID (g=) or opaque (b=) key is not supported: give a number (i=) \
                 or a name (s=)"
            }
            Self::Syntax => {
                "a node ID is ns=<index>;i=<number> or ns=<index>;s=<name>, with an \
                 index to 65535, a number to 4294967295, and a name of 1 to 4096 bytes"
            }
        })
    }
}

impl std::error::Error for Error {}

impl FromStr for Node {
    type Err = Error;

    /// Reads the text form of a node. The name of `s=` is the rest of the text, also
    /// when it holds a `;`.
    fn from_str(text: &str) -> Result<Self, Error> {
        if text.starts_with("nsu=") {
            return Err(Error::Uri);
        }
        let (namespace, rest) = match text.strip_prefix("ns=") {
            Some(rest) => {
                let (index, rest) = rest.split_once(';').ok_or(Error::Syntax)?;
                (digits(index)?, rest)
            }
            None => (0, text),
        };
        let key = if let Some(number) = rest.strip_prefix("i=") {
            Key::Number(digits(number)?)
        } else if let Some(name) = rest.strip_prefix("s=") {
            Key::Name(Name::new(name).ok_or(Error::Syntax)?)
        } else if rest.starts_with("g=") || rest.starts_with("b=") {
            return Err(Error::Form);
        } else {
            return Err(Error::Syntax);
        };
        Ok(Self { namespace, key })
    }
}

/// Reads `text` as decimal digits only, so with no sign.
fn digits<T: FromStr>(text: &str) -> Result<T, Error> {
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Syntax);
    }
    text.parse().map_err(|_out_of_range| Error::Syntax)
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.namespace != 0 {
            write!(f, "ns={};", self.namespace)?;
        }
        match &self.key {
            Key::Number(number) => write!(f, "i={number}"),
            Key::Name(name) => write!(f, "s={}", name.as_str()),
        }
    }
}
