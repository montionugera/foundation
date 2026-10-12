- **SHARD POOLS (2026-10-06)** `Node::start` makes one `block::Pool` for each shard and
  moves it into the shard, which drops it (M4). Each of `n` shards gets `budget / n`,
  and shard 0 also gets the remainder, so the parts add up to the node's budget (MEMORY
  BOUNDS). The memory comes from `node::Config::memory`, a closure that `node` calls
  once per shard, in order of core: production passes `os::memory::Memory::new`, and
  `sim` tests pass `block::Heap`. A shard with no memory is a start failure: later
  shards do not start, the node stops, and `join` gives `Error::Memory` with the core
  and the `os::memory::Error`. Lost: making the pool on the shard's thread, which needs
  a second path for the error and a `Send + Sync` seam. Each shard runs one task that
  calls `Pool::purge` each 10 s of the node's monotonic clock, from before the open of
  its buffer to the end of the shard, so a size gives its pages back 10 to 20 s after
  the last of its blocks returned. `node` holds the interval as a constant. Its value is
  not measured: it is the purge policy of `docs/decisions/open/parameters.md`. A shorter
  interval gives memory back sooner, and a longer one bounds how often a size that comes
  back commits its range again. No shard calls `Pool::reclaim` on each loop turn: the
  loop is the driver's (`env::shards`), and `alloc` and `purge` take back the returned
  blocks. Lost: a field of `node::Config` for the interval, which no caller gives a
  second value; a purge from `alloc`, which gives `block` a clock; a hook that each
  driver calls on each turn, which moves the walk of the returned blocks and does not
  shorten it. Look again at the hook when a measurement shows that walk in the latency
  of an `alloc` (`laptop.architect-2`, #2443, 2026-10-11 21:30 UTC:
  https://github.com/synnaxlabs/foundation/issues/2443#issuecomment-6113992879).
  Proposed by `ops` in #410; approved by the coordinator on #806. `block::Config::new`
  takes the budget as a `u64` and gives `block::Unfit` when the pool's reservation is
  more than `usize::MAX` bytes, so `node` passes each part as it is and keeps no check
  of its own. Decided by `laptop.architect` (2026-10-07 10:27 UTC):
  https://github.com/synnaxlabs/foundation/issues/1317#issuecomment-6036013562, and made
  `const fn` by `laptop.architect` (2026-10-10 06:37 UTC):
  https://github.com/synnaxlabs/foundation/issues/1317#issuecomment-6094721967, and
  stated as more than `usize::MAX` bytes by `laptop.architect` (2026-10-10 09:23 UTC):
  https://github.com/synnaxlabs/foundation/pull/2269#issuecomment-6096088015.
  `Node::start` builds each `block::Config` before any shard starts, and for an `Unfit`
  starts no shard and gives `Error::Pool`. Decided by `laptop.architect` (2026-10-10
  15:27 UTC):
  https://github.com/synnaxlabs/foundation/pull/2269#issuecomment-6099086353, and
  `Error::Pool` approved by `laptop.architect-2` (2026-10-10 15:32 UTC):
  https://github.com/synnaxlabs/foundation/issues/1317#issuecomment-6099130173. Shard 0
  binds the port (NODE START), so `Error::Pool` wins over `Error::Port`. By
  `laptop.architect-2` (#2438, 2026-10-11T20:43:59Z:
  https://github.com/synnaxlabs/foundation/pull/2438#issuecomment-6113549763) and
  `laptop.architect` (2026-10-11T20:45:21Z:
  https://github.com/synnaxlabs/foundation/pull/2438#issuecomment-6113562774), which
  supersede "after the port binds" of the first link.
