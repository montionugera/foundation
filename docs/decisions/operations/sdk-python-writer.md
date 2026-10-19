- **SDK PYTHON WRITER** `Client.open_writer(channels, *, authority=0, lease=None,
  on_loss="warn")` of `sdk-python` opens a `foundation.Writer` over `sdk::write::Writer`
  (SDK WRITER), with the place order of `sdk`. `Writer.write` takes a `dict` from a
  channel name or a place to one value, or one value when the writer has one place
  other than the index. `Writer.write_series` takes a `dict` to one-dimensional
  `numpy.ndarray`s of the exact dtype of each place, in native byte order, and casts
  nothing. A stamp is an `int` of nanoseconds since the UNIX epoch, and
  `write_series` also takes `datetime64[ns]` for `timestamp` and `timedelta64[ns]` for
  `duration`. The binding checks each value and array against its place, then calls
  `sdk` with the GIL released. A writer of one process raises `RuntimeError` in a child
  that the process forks, and its close there gives `None` and ends nothing. The plan
  (https://github.com/synnaxlabs/foundation/issues/3320#issuecomment-6257542875) was
  approved by `laptop.architect-2` at 2026-10-19T01:19:25Z
  (https://github.com/synnaxlabs/foundation/issues/3320#issuecomment-6257620825), with
  these terms:
  1. The error map. `sdk::Error::Session` with `program::Error::Refused` raises
     `Refused`, a subclass of `ConnectionError`: the session ended, so connect again.
     `Stream` with `Refused` raises `Stopped`, a subclass of `Exception`: at the open,
     no writer opened and the session goes on, and a writer that raises it has ended.
     Each has the lower-case `code` of the refusal, and its docstring names the codes
     that it gives. The text of a `Stopped` with `unknown` from `open_writer` names
     each channel of the open, as the refusal holds only a code. Changed in round 1 of
     PR 3325 by `laptop.architect-2` at 2026-10-19T02:21:46Z
     (https://github.com/synnaxlabs/foundation/pull/3325#issuecomment-6258290968), with
     the OK of `laptop.monitor` at 2026-10-19T02:22:22Z
     (https://github.com/synnaxlabs/foundation/pull/3325#issuecomment-6258297079):
     before, `Refused` came for both, so `except ConnectionError` took a typo in a
     channel name for a lost connection. Lost: one `Refused(Exception)` for both,
     since a caller of `connect` that catches `ConnectionError` then misses a refused
     session; and a `session` attribute on `Refused`, since `except ConnectionError`
     then still catches a typo. `Stream` with `Layout`, `Unsent`, `Pool`, `Body`,
     `Open`, or `Unnamed` raises `ValueError`: nothing was sent, and the writer goes
     on, or, at the open, no writer opened. Each other `Stream` or `Session` error
     raises `ConnectionError`.
  2. `sdk-python` takes `program` as a normal dependency and matches
     `program::Error::Refused` itself. Lost: a re-export of `program::Error` and
     `program::Refusal` from `sdk`, two public items that no Rust caller names yet. The
     later step `sdk::Refusal` of #2177 replaces both when one does. The edge crosses
     the two lists. `laptop.architect` gave its OK at 2026-10-19T01:20:17Z
     (https://github.com/synnaxlabs/foundation/issues/3320#issuecomment-6257630032):
     the binding names only the error types of `program`, never a `Session`,
     `Reader`, or `Writer`.
  3. After `DataLost`, the writer goes on, because a later frame can find room.
  4. One `Mutex` holds the `sdk` writer and the kept buffers, so no call takes two
     locks. Each call checks the process before the lock, because a lock that another
     thread held at a fork stays held in the child, and then takes the lock with
     `MutexExt::lock_py_attached`, which waits with the GIL released. So a second
     Python thread that writes on the same writer cannot hold the GIL while the first
     waits for it. No Python code runs while a call holds the lock: a call takes the
     kept buffers under the lock and fills them with no lock held, takes the lock again
     for the `sdk` call and to keep the buffers, and raises or logs after it lets the
     lock go. So a value (`__float__`, `__index__`), a logging handler, or a `__del__`
     can call the same writer. A call that finds the buffers taken fills new ones.
     Changed in round 1 of PR 3325 by `laptop.architect-2` at 2026-10-19T02:21:46Z
     (https://github.com/synnaxlabs/foundation/pull/3325#issuecomment-6258290968):
     before, a call held the lock while it converted values and logged, so Python code
     that called the writer hung. Lost: a second lock for the buffers, since a call
     then takes two locks; and a re-entrant lock, since an inner call then changes the
     buffers that the outer call fills.
  5. `DataLost.seqs` is a `dict` from each channel name of the lost frame, the index
     included, to the `range` of its lost seqs, the shape that the reader of PR 7b
     gives. The text names each channel and its count.
  6. `Stopped` and `Rejected` are two classes, as their code sets differ. A writer has
     ended after `Stopped`, and each later call raises it again. It goes on after
     `Rejected`, the refusal of one frame (`waiting`, `reserved`, `expired`, `order`,
     `codec`, `full`, or `large`). Changed with item 1 (same links): `Stopped` was
     `Refused`.
  7. `write_series` copies each array into the kept buffer with the GIL held, because
     another Python thread can change an array while the GIL is free. Its cost is
     measured against the round trip, below.
  The key choices of the plan went ahead with `laptop.monitor` at 2026-10-19T01:13:34Z
  (https://github.com/synnaxlabs/foundation/issues/3320#issuecomment-6257555546), with
  one condition: the `TypeError` of an array of another dtype names the place, the
  dtype that it needs, the dtype given, and the fix, such as `plant.a holds f64 and
  takes an array of float64, not int64: pass a.astype("float64")`. The wheel requires
  the Python package `numpy>=1.26`.
  The numbers below come from `sdk/python/bench/write.py`, with the `wheel` profile
  and a release node on box2 (Xeon Platinum 8488C, 32 CPUs, load near 20) at
  c502e295f, in µs. The cost of the copy, p50 of 3000 calls of a frame of `f64` and
  its stamps: 0.18 at 100 samples, 0.96 at 1000, and 2.85 at 3000. The round trip,
  p50 of 400 calls: 51, 89, and 320. The copy is a call that copies the same arrays
  and then raises at a third array of another length, less that call with arrays of
  one sample. Lost: a split of `sdk::Writer::write_series` into a fill with the GIL
  held and a send with it released, a new public item of `sdk` for one copy fewer.
  The limit of 2.5 µs p99 applies only to the part of a call before
  `sdk::write::Writer::write`: the key lookup, the conversion, and the copy (ruling
  of the director, 2026-10-19T02:28:28Z,
  https://github.com/synnaxlabs/foundation/pull/3325#issuecomment-6258363742). That
  part, p99 of 3000 calls that raise before the send, less the p50 of a call that
  only raises:

  | Places | `write` | `write_series` 100 | 1000 | 3000 |
  | ------ | ------- | ------------------ | ---- | ---- |
  | 1      | 0.20    | 0.71               | 1.84 | 3.24 |
  | 16     | 0.94    | 3.75               | 9.19 | 24.7 |
  | 64     | 3.21    | 17.6               | 34.0 | 142  |
  | 255    | 13.7    | 72.9               | 187  | 694  |
