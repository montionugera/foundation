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

/// The roots of `roots`. When the read has a failure, logs a warning with the count of
/// its failures and its source, cut to `LONGEST` bytes, then one for each of the first
/// `LISTED` failures, its text cut the same way. When `served`, the node serves the
/// command that gives each failure, and the first warning names it. Logs a warning for
/// the count of certificates that do not parse, and when no certificate is left.
pub(crate) fn roots(roots: &os::roots::Roots, served: bool) -> connector::http::Roots {
    let failed = roots.failures.len();
    if failed > 0 {
        let source = roots.source.to_string();
        let source = cut(&source);
        if served {
            tracing::warn!(
                failed,
                %source,
                "parts of the OS roots were not read, and `foundation status` gives each"
            );
        } else {
            tracing::warn!(failed, %source, "parts of the OS roots were not read");
        }
    }
    for failure in roots.failures.iter().take(LISTED) {
        let error = cut(failure);
        tracing::warn!(%error, "a part of the OS roots was not read");
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
