//! The stamps of the samples that a connector reads by request.

use types::time::Stamp;

#[cfg(test)]
mod tests;

/// The stamps of the frames of one index. Each is the midpoint of the request that
/// read its samples, or 1 ns after the stamp before it when that is later, so no two
/// are equal and none goes back.
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
        let stamp = self.last.map_or(midpoint, |last| midpoint.max(after(last)));
        self.last = Some(stamp);
        (output, stamp)
    }

    /// Gives the stamp 1 ns after `before`, the last stamp of the index that the home
    /// gives when it refuses the frame of the last stamp as backwards, to write that
    /// frame again. Each later stamp is after it.
    pub fn after(&mut self, before: Stamp) -> Stamp {
        let stamp = after(before);
        self.last = Some(stamp);
        stamp
    }
}

fn after(stamp: Stamp) -> Stamp {
    Stamp::from_nanos(stamp.nanos().strict_add(1))
}
