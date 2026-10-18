- **BQ4 + ADAPTIVE COMPRESSION (r10)** Per 1024-value vector, one stats pass gives exact
  sizes, and the encoder picks the smallest. The ALP pass runs only when an estimate on
  32 samples at equal steps is under the best exact size. So a vector whose picked
  samples are exceptions keeps the integer codecs, also when ALP is smaller: `f64`
  decimals with a NaN at each 32nd sample stay raw at 8200 bytes, where ALP takes 1616
  bytes. Without the gate, encode takes 25% to 52% more instructions where ALP loses
  (box2, #3055) (`laptop.architect`,
  https://github.com/synnaxlabs/foundation/pull/3055#issuecomment-6238641797,
  2026-10-17T18:33:13Z). Raw is always a candidate. The minimum saving is a fixed 1/8.
  No sampling and no hysteresis, except ALP's top-5 exponent pairs and the gate of the
  ALP pass, with the pairs refreshed about every 100 vectors. Codec set: integers raw,
  FFOR, delta (natural order), RLE; timestamps add stride; floats raw, ALP, fdelta,
  FFOR, RLE; `max` mode adds pco. FFOR over the sortable bits of floats comes with
  fdelta (`laptop.architect`,
  https://github.com/synnaxlabs/foundation/issues/2997#issuecomment-6230146181,
  2026-10-17T05:24:27Z). The sortable integers of -1.0 and 1.0 are about 2^(b - 1)
  apart, where b is the bit count of a sample, so fdelta and FFOR leave a vector that
  crosses zero raw, other than one of values near zero, and ALP is its codec
  (`laptop.architect`,
  https://github.com/synnaxlabs/foundation/pull/3002#issuecomment-6230559602,
  2026-10-17T06:13:06Z). No zstd and no ALP_rd. A simple ALP search first (key choice
  8): the top 5 pairs come from 8 runs of 32 samples of one vector, not of 8 vectors, at
  the first vector whose best exact size is over the ALP header, the least that an ALP
  vector takes, and again after each 102,400 values of such vectors (`laptop.architect`,
  https://github.com/synnaxlabs/foundation/pull/3055#issuecomment-6241287258,
  2026-10-17T21:21:47Z). So a slow channel can keep the pairs of its first vector for
  hours. An `Encoder` keeps the pairs, and `home` keeps one encoder for each channel of
  a writer. The 8-vector search and a skip rule wait until a Pi 4 run shows over about 2
  ns a value (`laptop.architect`,
  https://github.com/synnaxlabs/foundation/issues/2997#issuecomment-6231723563,
  2026-10-17T08:26:13Z).
  Policy: `compression { select, mode = auto, raw, or max }`, default `auto`. The
  validator is the top fuzz target.
