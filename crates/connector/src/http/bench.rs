//! The parts of `http` that a benchmark times. Not part of the contract of the crate.

use http::Uri;

/// The origin of `uri`, which `Client::send` reads on each request, and which the
/// pool of connections compares.
///
/// # Errors
///
/// As [`super::uri`] gives for `uri`.
pub fn origin(uri: &Uri) -> Result<impl Ord + use<>, super::Error> {
    super::origin(uri)
}
