- **NARROW POLL (#2652, 2026-10-16)** A poll of many channels, one sample each, is the
  common write of a connector. For the cost of such a poll, a fix with no public change
  and no change to a disk or wire form goes ahead, and its PR states the cause and both
  figures: the ring bytes of one frame of 256 channels for each sample type, and the
  time of a live write of 256 i16 channels against 256 f64 channels on a quiet host,
  before and after. A change to a form, or a target that a narrow sample does not meet,
  waits for the architect. Decided by `laptop.architect` (2026-10-14T02:05:47Z):
  https://github.com/synnaxlabs/foundation/issues/2652#issuecomment-6159271013.
  In #2929, the integer planner of BQ4 + ADAPTIVE COMPRESSION skips its statistics for
  a vector of one sample, so a narrow poll costs no more time than an f64 poll; no
  encoded byte changes. The `hub` bench `write` times one live write of 256 channels
  for each of six sample types. The `home` tests
  `stores_a_poll_of_256_channels_in_26_bytes_a_series_and_its_vector` and
  `fills_a_new_ring_at_the_polls_of_256_channels_of_each_type` state the stored body
  and the ring bytes of that poll for each type. A narrow poll misses the P1 target of
  under 4 B a sample; #2928 holds that work.
