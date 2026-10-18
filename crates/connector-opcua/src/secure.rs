//! The test peers of secure channels.
//!
//! The fixtures in `secure/` are self-signed RSA 2048 certificates with SHA-256 and
//! their PKCS #1 keys, in DER from Python `cryptography` 41.0.7, each valid from
//! [`START`] to [`END`]. They are test keys only.

use std::ffi::CStr;

use types::time::{Span, Stamp};

use crate::ffi::{self, Bytes};

/// 2001-01-01.
pub(crate) const START: Stamp = from_secs(978_307_200);

/// 2002-01-01.
pub(crate) const END: Stamp = from_secs(1_009_843_200);

/// The test server.
pub(crate) const SERVER: Identity = Identity {
    certificate: include_bytes!("secure/server.der"),
    key: include_bytes!("secure/server-key.der"),
    uri: c"urn:foundation:test:server",
};

/// The test client.
pub(crate) const CLIENT: Identity = Identity {
    certificate: include_bytes!("secure/client.der"),
    key: include_bytes!("secure/client-key.der"),
    uri: c"urn:foundation:test:client",
};

/// A server with the URI of [`SERVER`] and another key.
pub(crate) const OTHER: Identity = Identity {
    certificate: include_bytes!("secure/other.der"),
    key: include_bytes!("secure/other-key.der"),
    uri: c"urn:foundation:test:server",
};

/// The stamp `secs` seconds after the Unix epoch.
pub(crate) const fn from_secs(secs: i64) -> Stamp {
    Stamp::from_nanos(secs * Span::SECOND.nanos())
}

/// A certificate, its key, and the application URI in it.
#[derive(Clone, Copy)]
pub(crate) struct Identity {
    pub(crate) certificate: &'static [u8],
    pub(crate) key: &'static [u8],
    pub(crate) uri: &'static CStr,
}

/// The identity of a peer and the one certificate that it trusts.
#[derive(Clone, Copy)]
pub(crate) struct Security {
    pub(crate) own: Identity,
    pub(crate) trusted: &'static [u8],
}

impl Security {
    /// The `shim_security` of the peer, which borrows its bytes.
    pub(crate) fn raw(&self) -> ffi::test::Security {
        ffi::test::Security {
            certificate: bytes(self.own.certificate),
            key: bytes(self.own.key),
            trusted: bytes(self.trusted),
            uri: self.own.uri.as_ptr(),
        }
    }
}

/// A `UA_ByteString` that borrows `der`.
pub(crate) fn bytes(der: &[u8]) -> Bytes {
    Bytes {
        length: der.len(),
        data: der.as_ptr().cast_mut(),
    }
}

/// The three policies that the copy compiles, in the order of `POLICIES` of
/// `server.c`.
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
}
