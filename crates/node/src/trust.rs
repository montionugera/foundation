//! The roots that the connectors of a node trust, and the warnings of their read.

#[cfg(test)]
mod tests;

use std::borrow::Cow;

/// The failures of the OS roots that each get a warning. A bound keeps the warnings,
/// all in one poll, from filling the log queue before `started`.
const LISTED: usize = 16;

/// The most bytes of the text of a failure in its warning. A line of a PEM file can be
/// of any length, and a failure gives the whole line.
const LONGEST: usize = 1024;

/// The mark in place of the middle of a failure that is too long.
const MARK: &str = "…";

/// The roots of `roots`. Logs a warning for each of the first `LISTED` failures of the
/// read, its text cut to `LONGEST` bytes, for the count of the rest with the source of
/// the read, cut the same way, for the count of certificates that do not parse, and
/// when no certificate is left.
pub(crate) fn roots(roots: &os::roots::Roots) -> connector::http::Roots {
    for failure in roots.failures.iter().take(LISTED) {
        let error = cut(failure);
        tracing::warn!(%error, "a part of the OS roots was not read");
    }
    let unlisted = roots.failures.len().saturating_sub(LISTED);
    if unlisted > 0 {
        let source = roots.source.to_string();
        let source = cut(&source);
        tracing::warn!(unlisted, %source, "more parts of the OS roots were not read");
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

/// `text` when it fits in `LONGEST` bytes. Else its head and its tail, cut at char
/// boundaries and joined by `MARK`, so a failure keeps both its reason and its path.
fn cut(text: &str) -> Cow<'_, str> {
    if text.len() <= LONGEST {
        return Cow::Borrowed(text);
    }
    let half = (LONGEST - MARK.len()) / 2;
    let head = &text[..text.floor_char_boundary(half)];
    let tail = &text[text.ceil_char_boundary(text.len() - half)..];
    Cow::Owned([head, MARK, tail].concat())
}
