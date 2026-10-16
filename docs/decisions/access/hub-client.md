- **HUB CLIENT (2026-10-08)** `program::Session` is the session of a program with one
  node (the crate `program` and its names, `laptop.architect`, 2026-10-15T18:23:47Z,
  https://github.com/synnaxlabs/foundation/pull/2724#issuecomment-6199257488).
  `Session::connect` dials on a `transport::Client`, sends the subject's hello, and
  gives the session once the node admits it, so a refused hello is an error of
  `connect`. A hello expires at the latest mesh time of its challenge, plus the time
  since the challenge came on the monotonic clock, plus `program::LIFE` (10 minutes), or
  at the end of mesh time when the sum passes it, as mesh time is the node's. Lost: an
  error of `connect` for a challenge near the end of mesh time, a second guard of the
  node's own checks of the expiry (`EXPIRED`). A task renews the hello at half of `LIFE`
  after each admission, on the hello stream, until the session ends. A renewal waits for
  a block as each message does, and the node closes the session if the hello ends first.
  A challenge that `wire` refuses ends the renewal and closes the session with
  `MALFORMED`. From then on, each request that fails gives the error that ended the
  renewal, also a request that was in flight, in place of the error of the close
  (`laptop.architect`, 2026-10-09 01:14 UTC,
  https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6072238837).
  `request(body)` signs the body, sends it on its own stream, and gives the body of the
  response. Requests of one session go one at a time, in the order they began, by a turn
  in the session, as a link holds one open request (HUB LINK). A request dropped before
  its response began keeps the turn until the response begins or the stream ends,
  because the node holds it open until then. `Config` holds its own `pool`, which a
  program may share with the transport. Each message that the session sends takes a
  block from it, which the stream holds until the node acknowledges it. A message that
  finds the pool full waits for a block, so a small pool makes the session slower, not
  fail. It tries again every 10 ms, the span of the transport's reads, and the wait ends
  when the session closes, with the error of the close. Only `Exhausted` and `Refused`
  wait: `TooLarge` never succeeds, so it gives `Error::Pool` at once, which means that
  no block of `pool` can hold a message that the session sends. Lost: a bound of two
  windows from the stream credit, which ack loss can exceed; a `Bytes` body with no
  copy; a wait in `block::Pool` that wakes at each free. Three callers in a program
  poll a pool now: the transport's reads, the session's sends on `pool`, and the frames
  of a read on `frames`. The trigger fired with the third (#2802; `laptop.architect`,
  2026-10-16T03:39:42Z,
  https://github.com/synnaxlabs/foundation/pull/2802#issuecomment-6207544466): #2811
  adds the wait that a free wakes to `block`.
  `Session` is `Clone`, and a clone is the same session. When the last clone and the
  last `Reader` drop, it closes the session with `Code(0)`. A node's stop and close with
  a code are one error: `Error::Refused(wire::hub::client::Refusal)`, a closed set in
  `wire` with `from_code` and `code`, of each code that a node stops a client stream or
  closes a client session with (`MALFORMED`, `BUSY`, and `REFUSED` to `CHANGED`). Each
  meaning of a code is stated once, in `wire`, and its `Display` gives the cause. A
  close with 0 or a code outside the set stays `Error::Transport`. Lost: one dial for
  each request (a handshake for each request, and a read stream of HUB LINK needs a
  session that lives; `laptop.architect`, 2026-10-13T02:42:31Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137032724); `&mut
  self` with the renewal inside `request` (an idle program loses its session at the
  expiry); a queue in a task that owns the session; a lock of a library
  (`tokio::sync::Semaphore` is FIFO, but it is built for many threads, with atomics and
  an `Arc` for an owned permit, where `program` needs one lock on one thread, and
  `futures::lock::Mutex` wakes the first waiter of its slab, not the oldest, and lets a
  new `lock` take the lock ahead of it, which starves a waiter); `Error::Stopped` with a
  `u32` code, which gives `program` a meaning for each code. The plan
  (https://github.com/synnaxlabs/foundation/issues/1748#issuecomment-6069186434) was
  approved with the `Refusal` change by `laptop.architect` at 2026-10-08T21:25:05Z
  (https://github.com/synnaxlabs/foundation/issues/1748#issuecomment-6069345374).
  `laptop.architect` approved the surface and the shared `pool` at 2026-10-08T21:32:05Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6069455056) and
  21:36:08Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6069514013), and the
  order "in the order they began" and the dropped request at 21:38:44Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6069553753). It
  approved the `MALFORMED` close at 22:04:03Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6069924128). It gave
  the reason against `tokio::sync::Semaphore` at 22:15:33Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6070115889). It
  approved the wait for a block at 22:56:10Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6070683319). It
  approved the expiry at the end of mesh time at 23:27:59Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6071068010) and its
  Lost clause at 23:29:46Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6071087807).
  Supersedes the sentence "Any pool works" of
  https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6069455056, and the
  room of the pool approved at 22:04:03Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6069924128), at
  22:34:24Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6070406709), at
  22:43:35Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6070527522), and at
  22:46:20Z
  (https://github.com/synnaxlabs/foundation/pull/2009#issuecomment-6070562330).
  `Session::reader(select, mode)` opens a read stream as the subject and gives a
  `Reader`, whose `next` gives each frame that the node sends, in order. A read takes no
  turn: it runs beside the requests. A reader holds a clone of its session. It keeps
  the error that ended it and gives it again at each `next`, and the error that ended
  the renewal replaces it, as for `request`. The key set of a reader is its own:
  `wire::hub::client::Reader` builds it with `KeySet::ordered` at the last place, so
  entry `n` is place `n`, its slots are not the node's, and places that name a channel
  twice give `client::Error::Repeated` (#2808, item 1 of
  https://github.com/synnaxlabs/foundation/pull/2802#issuecomment-6207214002). Its key
  is `u32::MAX`, and `Interner::intern` panics before it gives that key, so a lens that
  `hub` matches by `set.key()` never takes it for a node's set (`laptop.architect`,
  2026-10-16T06:47:02Z,
  https://github.com/synnaxlabs/foundation/issues/2808#issuecomment-6209795527). The
  decoder and `Frames::open` take the end of the stream as `None`, and a message that
  the decoder refuses leaves it as it was (round 1 of #2839). The plan
  (https://github.com/synnaxlabs/foundation/issues/1749#issuecomment-6204280445) was
  approved by `laptop.architect` at 2026-10-15T23:19:38Z
  (https://github.com/synnaxlabs/foundation/issues/1749#issuecomment-6204306531).
  A frame takes a block of `Config::frames`, a pool apart from `pool` and from the
  transport's, and holds it until it drops. So frames that the program holds never stop
  the renewal of the hello (`laptop.architect`, 2026-10-16T03:12:49Z,
  https://github.com/synnaxlabs/foundation/pull/2802#issuecomment-6207214002). A frame
  that finds `frames` full waits as a message does (#2810 ends a wait that the carve of
  another size blocks), and a frame that no block of `frames` can hold gives
  `Error::Pool` at once. Lost: one
  reserved block for the renewal, a second pool that the caller cannot see.
