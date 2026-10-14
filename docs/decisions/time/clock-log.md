- **CLOCK LOG (2026-10-14)** `clock::log::Reader` gives the times of each log line of a
  node: `Reader::new(wall: env::wall::Wall, mesh: clock::Reader)`, and `now() -> Time {
  wall: Stamp, mesh: Mesh }`. `wall` is the OS wall time, the stamp of the line.
  `Mesh` is `Near` when the midpoint of mesh time is at most 1 ms from `wall`,
  `Apart(midpoint)` when it is further, and `Unknown` before the node's first mesh
  time. Each line is for a person who reads the log, never for a data stamp (BQ20).
  (1) The reader is in `clock` because only `clock` reads `env::wall::Wall::now`
  (`clippy.toml:48`), so `node` reads no OS clock and only formats the line. (2) 1 ms
  is above the slew noise of a lone node and far below what a person sees when they
  place a log line against data. `laptop.architect-2` owns it. Lost: a method on
  `clock::Reader` that also reads the OS wall clock, because each crate that stamps
  data holds a `clock::Reader`, and a wall read on it invites a data stamp from the OS
  clock; `node` reads and compares both clocks, which needs an `#[expect]` of BQ20 in
  `node`; `Apart` with the whole `Interval`, which the line does not show. Decided by
  `laptop.architect-2` after the person's words "Yeah I agree. System clock first"
  (https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155377794), in
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155519701, item 3.
