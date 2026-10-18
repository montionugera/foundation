//! The parts of `http` that a benchmark times. Not part of the contract of the crate.

use http::Uri;

/// The origin of `uri`, which `Client::send` reads on each request, and which the
/// pool of connections compares.
///
/// # Errors
///
/// [`super::Error::Scheme`], [`super::Error::UserInfo`], [`super::Error::Host`], or
/// [`super::Error::Port`], as [`super::Client::send`] checks them.
pub fn origin(uri: &Uri) -> Result<impl Ord + use<>, super::Error> {
    super::origin(uri)
}
