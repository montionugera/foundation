//! The test peers of secure channels.
//!
//! The fixtures in `secure/` are self-signed RSA 2048 certificates with SHA-256 and
//! their PKCS #1 keys, in DER from Python `cryptography` 41.0.7, each valid from
//! [`START`] to [`END`]. They are test keys only.

use std::ffi::CStr;

use types::time::{Span, Stamp};

use crate::client;
use crate::ffi::Mode;

/// 2001-01-01.
pub(crate) const START: Stamp = from_secs(978_307_200);

/// 2002-01-01.
pub(crate) const END: Stamp = from_secs(1_009_843_200);

/// The test server.
pub(crate) const SERVER: Identity = Identity {
    certificate: include_bytes!("secure/server.der"),
    key: include_bytes!("secure/server-key.der"),
};

/// The test client.
pub(crate) const CLIENT: Identity = Identity {
    certificate: include_bytes!("secure/client.der"),
    key: include_bytes!("secure/client-key.der"),
};

/// A client on the key of [`CLIENT`] whose subject alternative name holds no URI.
pub(crate) const ANONYMOUS: Identity = Identity {
    certificate: include_bytes!("secure/anonymous.der"),
    key: CLIENT.key,
};

/// A client on the key of [`CLIENT`] with no subject alternative name.
pub(crate) const UNNAMED: Identity = Identity {
    certificate: include_bytes!("secure/unnamed.der"),
    key: CLIENT.key,
};

/// A client on the key of [`CLIENT`] whose first URI is empty.
pub(crate) const EMPTY_URI: Identity = Identity {
    certificate: include_bytes!("secure/empty-uri.der"),
    key: CLIENT.key,
};

/// A client on the key of [`CLIENT`] with the URI of [`CLIENT`], then another.
pub(crate) const TWO_URIS: Identity = Identity {
    certificate: include_bytes!("secure/two-uris.der"),
    key: CLIENT.key,
};

/// A server with the URI of [`SERVER`] and another key.
pub(crate) const OTHER: Identity = Identity {
    certificate: include_bytes!("secure/other.der"),
    key: include_bytes!("secure/other-key.der"),
};

const fn from_secs(secs: i64) -> Stamp {
    Stamp::from_nanos(secs * Span::SECOND.nanos())
}

/// A certificate and its key.
#[derive(Clone, Copy)]
pub(crate) struct Identity {
    pub(crate) certificate: &'static [u8],
    pub(crate) key: &'static [u8],
}

/// The identity of a peer and the certificates that it trusts.
#[derive(Clone, Copy)]
pub(crate) struct Security {
    pub(crate) own: Identity,
    pub(crate) trusted: &'static [&'static [u8]],
}

impl Security {
    /// The security of a client of the peer that asks for `mode`.
    pub(crate) fn client(&self, mode: Mode) -> client::Security<'static> {
        client::Security {
            certificate: self.own.certificate,
            key: self.own.key,
            trust: self.trusted,
            mode,
        }
    }
}

/// The three policies that the copy compiles.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Policy {
    Basic256Sha256,
    Aes128Sha256RsaOaep,
    Aes256Sha256RsaPss,
}

impl Policy {
    pub(crate) const ALL: [Self; 3] = [
        Self::Basic256Sha256,
        Self::Aes128Sha256RsaOaep,
        Self::Aes256Sha256RsaPss,
    ];

    /// The URI of the policy.
    pub(crate) fn uri(self) -> &'static CStr {
        match self {
            Self::Basic256Sha256 => {
                c"http://opcfoundation.org/UA/SecurityPolicy#Basic256Sha256"
            }
            Self::Aes128Sha256RsaOaep => {
                c"http://opcfoundation.org/UA/SecurityPolicy#Aes128_Sha256_RsaOaep"
            }
            Self::Aes256Sha256RsaPss => {
                c"http://opcfoundation.org/UA/SecurityPolicy#Aes256_Sha256_RsaPss"
            }
        }
    }
}
