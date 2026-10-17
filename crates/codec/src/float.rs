//! The integers that the codecs see for float samples.

use crate::word;

/// The integer that the codecs see for the bits of a float of `W` bytes. A float with
/// its sign bit set has each bit flipped, and any other only its sign bit, so the
/// integers keep the order of the floats in unsigned order. It is a bijection on the
/// bits.
pub(crate) fn key<const W: usize>(bits: u64) -> u64 {
    let mask = word::mask(W);
    let high = mask ^ mask.wrapping_shr(1);
    let negative = 0_u64.wrapping_sub(u64::from(bits & high != 0));
    bits ^ (negative & mask | high)
}

/// The bits of the float of `W` bytes whose [`key`] is `key`.
pub(crate) fn bits<const W: usize>(key: u64) -> u64 {
    let mask = word::mask(W);
    let high = mask ^ mask.wrapping_shr(1);
    let negative = 0_u64.wrapping_sub(u64::from(key & high == 0));
    key ^ (negative & mask | high)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn key_keeps_the_order_of_floats(a in any::<f64>(), b in any::<f64>()) {
            let order = |value: f64| key::<8>(value.to_bits());
            prop_assert_eq!(a.total_cmp(&b), order(a).cmp(&order(b)));
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "an `f32` sample"
            )]
            let (a, b) = (a as f32, b as f32);
            let order = |value: f32| key::<4>(value.to_bits().into());
            prop_assert_eq!(a.total_cmp(&b), order(a).cmp(&order(b)));
        }

        #[test]
        fn bits_inverts_key(value in any::<u64>()) {
            prop_assert_eq!(bits::<8>(key::<8>(value)), value);
            let narrow = value & word::mask(4);
            prop_assert_eq!(key::<4>(narrow) & !word::mask(4), 0);
            prop_assert_eq!(bits::<4>(key::<4>(narrow)), narrow);
        }
    }
}
