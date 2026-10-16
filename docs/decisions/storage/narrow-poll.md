- **NARROW POLL (#2652, 2026-10-16)** A poll of many channels, one sample each, is the
  common write of a connector. The `hub` bench `write` times one live write of 256
  channels for each of six sample types, and the `home` test
  `stores_a_poll_of_256_channels_in_26_bytes_a_series_and_its_vector` states the body
  bytes of that poll for each type. `codec::int::plan` gives `Raw` for a vector of one
  sample before it reads any statistics, since no codec saves 1/8 of one sample. The
  encoded bytes do not change. A fix with no public change and no change to a disk or
  wire form goes ahead; a change to a form, or a target that a narrow sample does not
  meet, waits for the architect. Decided by `laptop.architect` (2026-10-14T02:05:47Z):
  https://github.com/synnaxlabs/foundation/issues/2652#issuecomment-6159271013.
