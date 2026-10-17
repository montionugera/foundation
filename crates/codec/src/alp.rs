//! ALP: floats held as the integers `round(v * 10^e / 10^f)` of an exponent `e` and a
//! factor `f`, with the floats that do not round-trip held raw as exceptions.

use std::cmp::Reverse;

use crate::vector::{self, Plan};
use crate::{bits, int, word};

/// `10^i` for floats of 8 bytes. Each is exact.
const POWERS_64: [f64; 19] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14,
    1e15, 1e16, 1e17, 1e18,
];
/// The floats of 8 bytes nearest to `10^-i`.
const INVERSES_64: [f64; 19] = [
    1e0, 1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 1e-9, 1e-10, 1e-11, 1e-12,
    1e-13, 1e-14, 1e-15, 1e-16, 1e-17, 1e-18,
];
/// `10^i` for floats of 4 bytes. Each is exact.
const POWERS_32: [f32; 11] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10];
/// The floats of 4 bytes nearest to `10^-i`.
const INVERSES_32: [f32; 11] = [
    1e0, 1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 1e-9, 1e-10,
];

/// Adding then subtracting it rounds a float under half of it to an integer, ties to
/// even: `1.5 * 2^52` and `1.5 * 2^23`.
const MAGIC_64: f64 = 6_755_399_441_055_744.0;
const MAGIC_32: f32 = 12_582_912.0;

/// Pairs that an encoder keeps.
const TOP: usize = 5;
/// Samples in one run of a search.
const RUN: usize = 32;
/// Runs of a search of all pairs.
const RUNS: usize = 8;
/// Values after which an encoder searches all pairs again.
const SEARCH: usize = 102_400;

/// The largest exponent for floats of `width` bytes: `10^e` fits their integer.
pub(crate) fn max_exponent(width: usize) -> u8 {
    if width == 4 { 10 } else { 18 }
}

/// An exponent `e` and a factor `f`, with `f <= e`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pair {
    pub(crate) exponent: u8,
    pub(crate) factor: u8,
}

impl Pair {
    /// Each pair for floats of `W` bytes, in search order.
    fn all<const W: usize>() -> impl Iterator<Item = Self> {
        (0..=max_exponent(W)).flat_map(|exponent| {
            (0..=exponent).map(move |factor| Self { exponent, factor })
        })
    }

    /// The place of the pair in [`Pair::all`].
    fn index(self) -> usize {
        let exponent = usize::from(self.exponent);
        exponent
            .strict_mul(exponent.strict_add(1))
            .strict_div(2)
            .strict_add(usize::from(self.factor))
    }
}

/// Converts between the floats of `W` bytes and the integers of one pair.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Coder<const W: usize> {
    /// `10^e` and `10^-f`, widened to `f64`.
    encode: (f64, f64),
    /// `10^f` and `10^-e`, widened to `f64`.
    decode: (f64, f64),
}

impl<const W: usize> Coder<W> {
    /// The coder of `pair`.
    ///
    /// # Panics
    ///
    /// Panics when the exponent of `pair` is over [`max_exponent`].
    pub(crate) fn new(pair: Pair) -> Self {
        let power = |i: u8| {
            let i = usize::from(i);
            let entry = if W == 4 {
                POWERS_32
                    .get(i)
                    .zip(INVERSES_32.get(i))
                    .map(|(power, inverse)| (f64::from(*power), f64::from(*inverse)))
            } else {
                POWERS_64.get(i).copied().zip(INVERSES_64.get(i).copied())
            };
            entry.expect("invariant: the exponent is at most its maximum")
        };
        let (exponent, factor) = (power(pair.exponent), power(pair.factor));
        Self {
            encode: (exponent.0, factor.1),
            decode: (factor.0, exponent.1),
        }
    }

    /// The integer of the float with bits `bits` when it decodes to the same bits.
    #[inline]
    pub(crate) fn exact(self, bits: u64) -> Option<i64> {
        let n = self.integer(bits);
        (self.float(n) == bits).then_some(n)
    }

    /// The integer nearest to the float with bits `bits` times `10^e / 10^f`,
    /// saturated to a signed integer of `W` bytes. Far from an integer, it is any
    /// integer: [`Coder::exact`] then refuses it.
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "saturating float to integer casts; the f32 values were f32"
    )]
    #[inline]
    fn integer(self, bits: u64) -> i64 {
        if W == 4 {
            let value = f32::from_bits(bits as u32);
            let scaled = value * self.encode.0 as f32 * self.encode.1 as f32;
            i64::from((scaled + MAGIC_32 - MAGIC_32) as i32)
        } else {
            let scaled = f64::from_bits(bits) * self.encode.0 * self.encode.1;
            (scaled + MAGIC_64 - MAGIC_64) as i64
        }
    }

    /// The bits of the float `n * 10^f * 10^-e`, two multiplications in the float type
    /// of `W` bytes. `n` is a signed integer of `W` bytes.
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "n holds W bytes; the f32 values were f32; integers round to floats"
    )]
    #[inline]
    pub(crate) fn float(self, n: i64) -> u64 {
        if W == 4 {
            let scaled = n as i32 as f32 * self.decode.0 as f32 * self.decode.1 as f32;
            scaled.to_bits().into()
        } else {
            (n as f64 * self.decode.0 * self.decode.1).to_bits()
        }
    }
}

/// The signed integer of the low `W` bytes of `value`.
pub(crate) fn signed<const W: usize>(value: u64) -> i64 {
    let shift = 64_u32.strict_sub(u32::from(word::bits(W)));
    value.wrapping_shl(shift).cast_signed().wrapping_shr(shift)
}

/// The integers and exceptions of some samples under one pair.
struct Stats {
    min: i64,
    max: i64,
    exceptions: usize,
}

impl Stats {
    fn of<const W: usize>(samples: impl Iterator<Item = u64>, pair: Pair) -> Self {
        let coder = Coder::<W>::new(pair);
        let mut stats = Self {
            min: i64::MAX,
            max: i64::MIN,
            exceptions: 0,
        };
        for bits in samples {
            match coder.exact(bits) {
                Some(n) => {
                    stats.min = stats.min.min(n);
                    stats.max = stats.max.max(n);
                }
                None => stats.exceptions = stats.exceptions.strict_add(1),
            }
        }
        stats
    }

    /// The smallest integer, or 0 when each sample is an exception, as `W` bytes.
    fn reference<const W: usize>(&self) -> u64 {
        if self.min > self.max {
            0
        } else {
            self.min.cast_unsigned() & word::mask(W)
        }
    }

    /// The bits of the integers less the reference.
    fn bits(&self) -> u8 {
        if self.min > self.max {
            0
        } else {
            int::width(
                self.max
                    .cast_unsigned()
                    .wrapping_sub(self.min.cast_unsigned()),
            )
        }
    }

    /// The bits of the samples in an ALP body, without padding.
    fn size<const W: usize>(&self, count: usize) -> usize {
        let exception = usize::from(word::bits(W)).strict_add(16);
        count
            .strict_mul(usize::from(self.bits()))
            .strict_add(self.exceptions.strict_mul(exception))
    }
}

/// The pairs that the float series of one encoder pick, best first.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Pairs {
    top: [Pair; TOP],
    len: usize,
    /// Values since the last search of all pairs.
    values: usize,
}

impl Pairs {
    /// The plan of a vector of floats of `W` bytes: ALP when it is smaller than `best`,
    /// the plan of the integer codecs, and saves 1/8 of raw, else `best`.
    pub(crate) fn plan<const W: usize>(&mut self, chunk: &[u8], best: Plan) -> Plan {
        let count = chunk.len().div_euclid(W);
        if count < 2 {
            return best;
        }
        if self.len == 0 || self.values >= SEARCH {
            self.search::<W>(chunk);
            self.values = 0;
        }
        self.values = self.values.strict_add(count);
        let limit = best.len(count, W);
        let mut picked = [0; RUN];
        let picked = pick::<W>(chunk, &mut picked);
        let (pair, size) = self.best::<W>(picked);
        let estimate = vector::header_len(W.strict_add(4), W)
            .strict_add(size.strict_mul(count).div_ceil(picked.len().strict_mul(8)));
        if estimate >= limit {
            return best;
        }
        let stats = Stats::of::<W>(word::samples::<W>(chunk), pair);
        let alp = Plan::Alp {
            reference: stats.reference::<W>(),
            bits: stats.bits(),
            pair,
            exceptions: stats.exceptions,
        };
        let len = alp.len(count, W);
        if len < limit && len.strict_mul(8) <= chunk.len().strict_mul(7) {
            alp
        } else {
            best
        }
    }

    /// The pair of the top pairs with the smallest size on `picked`, and that size. It
    /// stops after two pairs in a row that are not smaller.
    fn best<const W: usize>(&self, picked: &[u64]) -> (Pair, usize) {
        let (first, rest) = self
            .top
            .split_at(self.len)
            .0
            .split_first()
            .expect("invariant: a search keeps a pair");
        let size =
            |pair| Stats::of::<W>(picked.iter().copied(), pair).size::<W>(picked.len());
        let mut best = (*first, size(*first));
        let mut worse = 0_u8;
        for pair in rest {
            let size = size(*pair);
            if size < best.1 {
                best = (*pair, size);
                worse = 0;
            } else {
                worse = worse.strict_add(1);
                if worse == 2 {
                    break;
                }
            }
        }
        best
    }

    /// Keeps the pairs that win most runs of `chunk`, each run the smallest of all
    /// pairs on [`RUN`] samples. Ties go in search order.
    fn search<const W: usize>(&mut self, chunk: &[u8]) {
        let mut picked = [0; RUNS * RUN];
        let picked = pick::<W>(chunk, &mut picked);
        let mut wins = [(Pair::default(), 0_usize); RUNS];
        let mut len = 0;
        for run in picked.chunks(RUN) {
            let winner = Pair::all::<W>()
                .min_by_key(|pair| {
                    Stats::of::<W>(run.iter().copied(), *pair).size::<W>(run.len())
                })
                .expect("invariant: there is a pair");
            let known = wins.split_at_mut(len).0.iter_mut();
            if let Some((_, count)) =
                known.into_iter().find(|(pair, _)| *pair == winner)
            {
                *count = count.strict_add(1);
                continue;
            }
            *wins.get_mut(len).expect("invariant: one winner a run") = (winner, 1);
            len = len.strict_add(1);
        }
        let wins = wins.split_at_mut(len).0;
        wins.sort_unstable_by_key(|(pair, count)| (Reverse(*count), pair.index()));
        self.len = len.min(TOP);
        for (slot, (pair, _)) in self.top.iter_mut().zip(wins.iter()) {
            *slot = *pair;
        }
    }
}

/// Fills the front of `out` with the bits of samples of `chunk` at equal steps, as many
/// as fit, and returns them.
fn pick<'a, const W: usize>(chunk: &[u8], out: &'a mut [u64]) -> &'a [u64] {
    let samples = chunk.as_chunks::<W>().0;
    let len = samples.len().min(out.len());
    let step = samples.len().div_euclid(len);
    let out = out.split_at_mut(len).0;
    for (slot, sample) in out.iter_mut().zip(samples.iter().step_by(step)) {
        *slot = word::load(sample);
    }
    out
}

/// Writes the floats of `W` bytes of an ALP vector into `out`, which holds exactly
/// them. `body` is the vector's body, which [`vector::read`] checked.
pub(crate) fn decode<const W: usize>(
    reference: u64,
    bits: u8,
    pair: Pair,
    exceptions: usize,
    body: &[u8],
    out: &mut [u8],
) {
    let coder = Coder::<W>::new(pair);
    let samples = out.as_chunks_mut::<W>().0;
    let packed = bits::len(samples.len(), bits).next_multiple_of(W);
    let (packed, rest) = body.split_at(packed);
    if bits == 0 {
        samples.fill(word::store(coder.float(signed::<W>(reference))));
    } else {
        for (sample, residual) in samples.iter_mut().zip(bits::unpack(packed, bits)) {
            let n = signed::<W>(reference.wrapping_add(residual));
            *sample = word::store(coder.float(n));
        }
    }
    let (values, positions) = rest.split_at(exceptions.strict_mul(W));
    let positions = positions.as_chunks::<2>().0;
    for (value, position) in values.as_chunks::<W>().0.iter().zip(positions) {
        let position = usize::from(u16::from_le_bytes(*position));
        *samples
            .get_mut(position)
            .expect("invariant: read checks each position") = *value;
    }
}
