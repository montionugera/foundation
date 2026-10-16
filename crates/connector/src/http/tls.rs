//! What an `https` connect trusts, and the rustls config of each connect.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::Resumption;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::time_provider::TimeProvider;
use rustls::{CertificateError, ClientConfig, RootCertStore};
use types::time::Stamp;

/// The certificate authorities that a TLS connect trusts. Clones share one store.
/// The default trusts none.
#[derive(Clone)]
pub struct Roots(Arc<RootCertStore>);

impl Roots {
    /// The roots of `certificates`, each one DER bytes. It skips a certificate that
    /// does not parse, as the OS store can hold one, and gives the count it skipped.
    pub fn from_der<'a>(
        certificates: impl IntoIterator<Item = &'a [u8]>,
    ) -> (Self, usize) {
        let mut store = RootCertStore::empty();
        let certificates = certificates.into_iter().map(CertificateDer::from);
        let (_, skipped) = store.add_parsable_certificates(certificates);
        (Self(Arc::new(store)), skipped)
    }

    /// These roots plus each certificate of the PEM text `pem`. It skips each PEM
    /// section that is not a certificate.
    ///
    /// # Errors
    ///
    /// [`Pem::Empty`] when `pem` holds no certificate, [`Pem::Invalid`] when a
    /// certificate of it does not parse.
    pub fn with_pem(&self, pem: &str) -> Result<Self, Pem> {
        let mut store = (*self.0).clone();
        let mut added = false;
        for certificate in CertificateDer::pem_slice_iter(pem.as_bytes()) {
            let certificate = certificate.map_err(|_pem| Pem::Invalid)?;
            store.add(certificate).map_err(|_der| Pem::Invalid)?;
            added = true;
        }
        if !added {
            return Err(Pem::Empty);
        }
        Ok(Self(Arc::new(store)))
    }

    /// True when the store holds no root.
    pub(crate) fn empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for Roots {
    fn default() -> Self {
        Self(Arc::new(RootCertStore::empty()))
    }
}

impl fmt::Debug for Roots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Roots")
            .field("count", &self.0.len())
            .finish()
    }
}

/// Why a PEM text gave no roots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pem {
    /// The text holds no certificate.
    Empty,
    /// A certificate of the text does not parse.
    Invalid,
}

impl fmt::Display for Pem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the PEM text holds no certificate"),
            Self::Invalid => write!(f, "a certificate of the PEM text does not parse"),
        }
    }
}

impl std::error::Error for Pem {}

/// The config of one connect: it trusts `roots`, and checks each certificate at
/// `now`, a stamp of mesh time. It keeps no session to resume, because each connect
/// gets its own config.
pub(super) fn client(
    provider: &Arc<CryptoProvider>,
    roots: &Roots,
    now: Stamp,
) -> Arc<ClientConfig> {
    #[expect(clippy::disallowed_methods, reason = "it passes mesh time")]
    let builder =
        ClientConfig::builder_with_details(Arc::clone(provider), Arc::new(At(now)));
    let mut config = builder
        .with_safe_default_protocol_versions()
        .expect("invariant: aws-lc-rs has suites of each default version")
        .with_root_certificates(Arc::clone(&roots.0))
        .with_no_client_auth();
    config.resumption = Resumption::disabled();
    Arc::new(config)
}

/// Whether `error` of a handshake is webpki's refusal of a server certificate with
/// `CA:TRUE`, which a self-signed certificate of `openssl req -x509` has by default.
pub(super) fn ca_as_server(error: &io::Error) -> bool {
    let Some(rustls::Error::InvalidCertificate(CertificateError::Other(other))) =
        error.get_ref().and_then(|error| error.downcast_ref())
    else {
        return false;
    };
    // The downcast finds the error only when this `rustls-webpki` is the version that
    // `rustls` uses.
    matches!(
        other.0.downcast_ref(),
        Some(webpki::Error::CaUsedAsEndEntity)
    )
}

/// One stamp of mesh time, as rustls reads the time. A stamp before the Unix epoch
/// gives no time, which fails the check.
#[derive(Debug)]
pub(super) struct At(pub(super) Stamp);

impl TimeProvider for At {
    fn current_time(&self) -> Option<UnixTime> {
        let nanos = u64::try_from(self.0.nanos()).ok()?;
        Some(UnixTime::since_unix_epoch(Duration::from_nanos(nanos)))
    }
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests;
