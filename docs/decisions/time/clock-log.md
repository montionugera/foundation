- **CLOCK LOG (2026-10-14)** `clock::log::Reader` gives the times of each log line of a
  node: `Reader::new(wall: env::wall::Wall, mesh: clock::Reader)`, and `now() -> Time {
  wall: Stamp, mesh: Mesh }`. `wall` is the OS wall time, the stamp of the line.
  `Mesh` is `Near` when the best guess of mesh time is at most 2 ms from `wall`,
  `Apart(best guess)` when it is further, and `Unsynced` before a majority of the
  node's sources first agree. The best guess is `estimate::Measurement::time`, not the
  midpoint of the interval, which moves far from it when an edge stops at the end of a
  stamp. `now` reads the two clocks one after the other, so the time between the reads
  counts toward the 2 ms. Each line is for a person who reads the log, never for a data
  stamp (BQ20), so `clock::log::Reader::now` is in `disallowed-methods`, and its one
  call site, in the log of `node`, has an `#[expect]` with the reason "only the log
  stamps lines".
  (1) The reader is in `clock` because only `clock` reads `env::wall::Wall::now`
  (`clippy.toml:48`), so `node` reads no OS clock and only formats the line. (2) 2 ms
  is above 200 ppm, the most drift that the clock allows for, times the age of the
  oldest measurement that the filter keeps while an OS bound grows (1.6 ms), and far
  below what a person sees when they place a log line against data. A `sim` test of a
  lone node at 200 ppm, whose OS bound grows 500 µs each second, checks that each read
  is `Near`. `laptop.architect-2` owns it. Lost: a method on `clock::Reader` that also
  reads the OS wall clock, because each crate that stamps data holds a
  `clock::Reader`, and a wall read on it invites a data stamp from the OS clock; `node`
  reads and compares both clocks, which needs an `#[expect]` of BQ20 in `node`; `Apart`
  with the whole `Interval`, which the line does not show; a log reader with no lint,
  because a crate that holds a `Wall` reads an OS wall stamp through it; a read of mesh
  time on each side of the wall read, because it only narrows the gap between the
  reads and `sim` cannot test a thread switch (trigger: a report of a false `Apart`
  line on a loaded host). Decided by `laptop.architect-2` after the person's words
  "Yeah I agree. System clock first" (2026-10-13T21:17:21Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155377794), in
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155519701
  (2026-10-13T21:25:00Z), item 3. The lint, `Unsynced`, and the best guess:
  `laptop.architect-2`, 2026-10-14T03:28:33Z,
  https://github.com/synnaxlabs/foundation/pull/2666#issuecomment-6160221127. 2 ms and
  the gap between the reads: `laptop.architect-2`, 2026-10-14T03:30:45Z,
  https://github.com/synnaxlabs/foundation/pull/2666#issuecomment-6160247773.
