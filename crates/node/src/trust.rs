//! The roots that the connectors of a node trust, and the warnings of their read.

/// The failures of the OS roots that each get a warning. A bound keeps the warnings,
/// all in one poll, from filling the log queue before `started`.
const LISTED: usize = 16;

/// The roots of `roots`. Logs a warning for each of the first `LISTED` failures of the
/// read, for the count of the rest, for the count of certificates that do not parse,
/// and when no certificate is left.
pub(crate) fn roots(roots: &os::Roots) -> connector::http::Roots {
    for error in roots.failures.iter().take(LISTED) {
        tracing::warn!(%error, "a part of the OS roots was not read");
    }
    let unlisted = roots.failures.len().saturating_sub(LISTED);
    if unlisted > 0 {
        tracing::warn!(unlisted, "more parts of the OS roots were not read");
    }
    let (trusted, skipped) =
        connector::http::Roots::from_der(roots.certificates.iter().map(Vec::as_slice));
    if skipped > 0 {
        tracing::warn!(skipped, "OS roots that do not parse are skipped");
    }
    if trusted.empty() {
        tracing::warn!(
            "the OS roots hold no certificate, so an https connect with no CA of its \
             connector fails"
        );
    }
    trusted
}
