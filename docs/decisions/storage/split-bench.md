- **SPLIT BENCH (#1827, 2026-10-12)** `home::bench` (STORED BENCH) also holds `Scratch`
  and `split`, which only the bench `benches/split.rs` (`test = true`) uses. `split`
  checks and splits a draft, walks every stamp of each group's index, then makes the
  index frame of each group and gives it to a closure, and returns the number of
  stamps walked. The timed call passes `drop`. The bench times it per series for 16
  and 1000 series, raw and encoded, of `f32` and of each type of `MIXED`. Its binary
  holds a `counting::Allocator` (COUNTING ALLOCATOR). Before it times a case, it
  asserts the stamps walked, the bytes of each series of the index frame, and that a
  warm call makes no allocation. So `home` has `counting` as a dev-dependency. Lost: a
  line of the `hub` `reader` bench, which hides the split in the cost of the whole
  write; a hand-made harness with a table of percentiles, a third copy of that table;
  the cases in `benches/stored.rs`, whose `entry` figures the counting allocator would
  move; and a return of the bytes of the index frames, which times a walk of each
  series that a shard's write does not make.
