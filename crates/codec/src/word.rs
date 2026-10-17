//! Little-endian integer samples of `W` bytes, held as `u64`.

/// The bits a sample of `width` bytes holds.
pub(crate) fn mask(width: usize) -> u64 {
    u64::MAX.unbounded_shr(64_u32.strict_sub(bits(width).into()))
}

/// The bit count of a sample of `width` bytes.
pub(crate) fn bits(width: usize) -> u8 {
    u8::try_from(width.strict_mul(8)).expect("invariant: a sample is at most 16 bytes")
}

/// The samples in `bytes`, zero-extended.
pub(crate) fn samples<const W: usize>(
    bytes: &[u8],
) -> impl Iterator<Item = u64> + Clone + '_ {
    bytes.as_chunks::<W>().0.iter().map(load)
}

/// One sample, zero-extended.
pub(crate) fn load<const W: usize>(sample: &[u8; W]) -> u64 {
    let mut word = [0; 8];
    word.split_at_mut(W).0.copy_from_slice(sample);
    u64::from_le_bytes(word)
}

/// The low `W` bytes of `value`.
pub(crate) fn store<const W: usize>(value: u64) -> [u8; W] {
    *value
        .to_le_bytes()
        .first_chunk()
        .expect("invariant: a sample is at most 8 bytes")
}

/// The integer that the codecs see for the bits of a float of `W` bytes. A float with
/// its sign bit set has each bit flipped, and any other only its sign bit, so the
/// integers keep the order of the floats in unsigned order. It is a bijection on the
/// bits.
pub(crate) fn sortable<const W: usize>(bits: u64) -> u64 {
    let mask = mask(W);
    let high = mask ^ mask.wrapping_shr(1);
    let negative = 0_u64.wrapping_sub(u64::from(bits & high != 0));
    bits ^ (negative & mask | high)
}

/// The bits of the float of `W` bytes whose [`sortable`] integer is `key`.
pub(crate) fn unsortable<const W: usize>(key: u64) -> u64 {
    let mask = mask(W);
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
        fn sortable_keeps_the_order_of_floats(a in any::<f64>(), b in any::<f64>()) {
            let key = |value: f64| sortable::<8>(value.to_bits());
            prop_assert_eq!(a.total_cmp(&b), key(a).cmp(&key(b)));
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "an `f32` sample"
            )]
            let (a, b) = (a as f32, b as f32);
            let key = |value: f32| sortable::<4>(value.to_bits().into());
            prop_assert_eq!(a.total_cmp(&b), key(a).cmp(&key(b)));
        }

        #[test]
        fn unsortable_inverts_sortable(bits in any::<u64>()) {
            prop_assert_eq!(unsortable::<8>(sortable::<8>(bits)), bits);
            let narrow = bits & mask(4);
            prop_assert_eq!(sortable::<4>(narrow) & !mask(4), 0);
            prop_assert_eq!(unsortable::<4>(sortable::<4>(narrow)), narrow);
        }
    }
}
