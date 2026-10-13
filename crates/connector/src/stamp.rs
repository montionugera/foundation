//! The stamps of the frames that a connector writes.

use hub::writer::Writer;
use types::time::{Span, Stamp};

#[cfg(test)]
mod tests;

/// The stamps of the frames of one index. Each is the midpoint of the request that
/// read its samples, or the nanosecond after the stamp before it when that is later,
/// so no two are equal and none goes back.
#[derive(Debug, Default)]
pub struct Stamps {
    last: Option<Stamp>,
}

impl Stamps {
    /// Runs `request`, and gives its output with its stamp: the midpoint of the mesh
    /// times of `writer` before and after it, or 1 ns after the last stamp when that
    /// is later.
    ///
    /// # Panics
    ///
    /// When the last stamp is `i64::MAX` ns.
    pub async fn request<T>(
        &mut self,
        writer: &Writer,
        request: impl Future<Output = T>,
    ) -> (T, Stamp) {
        let sent = writer.now();
        let output = request.await;
        let answered = writer.now();
        let midpoint = Stamp::from_nanos(sent.nanos().midpoint(answered.nanos()));
        (output, self.next(midpoint))
    }

    /// Gives the nanosecond after `before`, for a frame that the home refused as
    /// `Backwards` with `before` as the stamp before it, and makes each later stamp
    /// after it.
    ///
    /// # Panics
    ///
    /// When `before` is under the last stamp, which no refusal gives, or is
    /// `i64::MAX` ns.
    pub fn after(&mut self, before: Stamp) -> Stamp {
        if let Some(last) = self.last {
            assert!(
                before >= last,
                "refusal under the last stamp: {} ns < {} ns",
                before.nanos(),
                last.nanos()
            );
        }
        let stamp = before + Span::NANOSECOND;
        self.last = Some(stamp);
        stamp
    }

    /// Gives `stamp`, or the nanosecond after the last stamp when that is later.
    ///
    /// # Panics
    ///
    /// When the last stamp is `i64::MAX` ns.
    pub(crate) fn next(&mut self, stamp: Stamp) -> Stamp {
        let stamp = self
            .last
            .map_or(stamp, |last| stamp.max(last + Span::NANOSECOND));
        self.last = Some(stamp);
        stamp
    }
}
