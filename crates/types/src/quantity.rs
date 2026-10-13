//! The number grammar that spans, rates, and byte sizes share: digits, then an
//! optional `.` and digits, then a unit.

use std::iter;

/// A decimal number and the unit text after it, as in `1.5s` or `200GiB`.
pub(crate) struct Quantity<'a> {
    /// The digits before the point. Never empty.
    pub(crate) whole: &'a str,
    /// The digits after the point, with no trailing `0`. Empty for a whole number.
    pub(crate) fraction: &'a str,
    /// The ASCII letters after the number. Never empty.
    pub(crate) unit: &'a str,
}

/// Splits `text` at its first byte that is not a digit or `.`. Returns `None` when the
/// number is not digits, then an optional `.` and digits, or when the rest is not one
/// or more ASCII letters.
pub(crate) fn split(text: &str) -> Option<Quantity<'_>> {
    let at = text.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = text.split_at(at);
    let (whole, fraction) = number.split_once('.').unwrap_or((number, "0"));
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let letters = unit.bytes().all(|b| b.is_ascii_alphabetic());
    (digits(whole) && digits(fraction) && letters).then(|| Quantity {
        whole,
        fraction: fraction.trim_end_matches('0'),
        unit,
    })
}

impl Quantity<'_> {
    /// Gives the number times 10^`places`, with each fraction digit past `places` cut
    /// off, or `None` when it does not fit in a `u128`.
    pub(crate) fn scaled(&self, places: usize) -> Option<u128> {
        let kept = self.fraction.get(..places).unwrap_or(self.fraction);
        let zeros = iter::repeat_n(b'0', places - kept.len());
        self.whole
            .bytes()
            .chain(kept.bytes())
            .chain(zeros)
            .try_fold(0_u128, |n, b| {
                n.checked_mul(10)?.checked_add(u128::from(b - b'0'))
            })
    }
}
