//! The stamps of the frames that a connector writes.

use hub::writer::Writer;
use types::time::Stamp;

#[cfg(test)]
mod tests;

/// The stamps of the frames of one index. Each is the midpoint of the request that
/// read its samples, or the time of the hub, or the nanosecond after the stamp before
/// it when that is later, so no two are equal and none goes back.
#[derive(Debug, Default)]
pub struct Stamps {
    last: Option<Stamp>,
}

impl Stamps {
    /// Runs `request`, and gives its output with its stamp: the midpoint of the times
    /// that `now` gives before and after it, or 1 ns after the last stamp when that is
    /// later. `now` is mesh time, such as `hub::writer::Writer::now`.
    pub async fn request<T>(
        &mut self,
        now: impl Fn() -> Stamp,
        request: impl Future<Output = T>,
    ) -> (T, Stamp) {
        let sent = now();
        let output = request.await;
        let answered = now();
        let midpoint = Stamp::from_nanos(sent.nanos().midpoint(answered.nanos()));
        (output, self.next(midpoint))
    }

    /// Gives the time of the hub of `writer`, or the nanosecond after the last stamp
    /// when that is later.
    pub(crate) fn now(&mut self, writer: &Writer) -> Stamp {
        self.next(writer.now())
    }

    /// Gives the nanosecond after `before`, for a frame that the home refused as
    /// `Backwards` with `before` as the stamp before it, and makes each later stamp
    /// after it.
    pub fn after(&mut self, before: Stamp) -> Stamp {
        let stamp = after(before);
        self.last = Some(stamp);
        stamp
    }

    /// Gives `stamp`, or the nanosecond after the last stamp when that is later.
    fn next(&mut self, stamp: Stamp) -> Stamp {
        let stamp = self.last.map_or(stamp, |last| stamp.max(after(last)));
        self.last = Some(stamp);
        stamp
    }
}

fn after(stamp: Stamp) -> Stamp {
    Stamp::from_nanos(stamp.nanos().strict_add(1))
}
