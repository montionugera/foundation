- **BQ4 + ADAPTIVE COMPRESSION (r10)** Per 1024-value vector, one stats pass gives exact
  sizes, and the encoder picks the smallest. Raw is always a candidate. The minimum
  saving is a fixed 1/8. No sampling and no hysteresis, except ALP's top-5 exponent
  pairs, refreshed about every 100 vectors. Codec set: integers raw, FFOR, delta
  (natural order), RLE; timestamps add stride; floats raw, ALP, fdelta, FFOR, RLE; `max`
  mode adds pco. FFOR over the sortable bits of floats comes with fdelta
  (`laptop.architect`,
  https://github.com/synnaxlabs/foundation/issues/2997#issuecomment-6230146181,
  2026-10-17T05:24:27Z). The keys of -1.0 and 1.0 are about 2^(b - 1) apart, where b is
  the bit count of a sample, so fdelta and FFOR leave a vector that crosses zero raw,
  other than one of values near zero, and ALP is its codec. No zstd and no ALP_rd.
  Policy: `compression { select, mode = auto, raw, or max }`, default `auto`. The
  validator is the top fuzz target.
