- **CANCELLED SEND (#68, 2026-10-07)** A `stream::Sender::send` or `send_parts` future
  that drops after the stream sent a byte of its message (its header counts), and
  before it completes, resets the stream with `Code(0)`. One that drops before that
  sends nothing and changes nothing, and the stream stays open. That includes a drop
  before its first poll, while it waits behind an earlier message, while it waits for
  its turn, and while it waits for room in the send budget. The core takes the message
  out and gives back its room in the send budget and its place in the turn, as a
  completed write does. After a reset, each `send`, `try_send`, `send_parts`,
  `try_send_parts`, and `finish` on the sender gives `Error::Reset { code: Code(0) }`
  after the checks below, and the `Error::Reset` doc names both causes: the peer, or a
  dropped `send` future. A dropped future is a normal cancel in async code, such as a
  timeout in a select, so it must not panic; in both cases the caller opens a new
  stream. Rejected: a panic, as after `finish` (a timeout the caller handles would
  become a crash). Proposed by `box2.builder-5`, decided by the architect, #68
  (https://github.com/synnaxlabs/foundation/issues/68#issuecomment-6030986313). Amended
  by the architect
  (https://github.com/synnaxlabs/foundation/issues/68#issuecomment-6035156093): reset
  only when bytes of the message may have gone. Amended again
  (https://github.com/synnaxlabs/foundation/issues/68#issuecomment-6035820204): the
  rule names the fact, a byte went, and not the proxy, the stream took it. `send`,
  `try_send`, `send_parts`, and `try_send_parts` check in this order: the range panic
  (`*_parts`), the panic after `finish`, `Error::TooLarge`, then the state errors
  (`Reset` after a dropped send future, `Stopped`, or the error that ended the
  session). The limit is fixed for the session, so a size defect shows in every state
  of the stream
  (https://github.com/synnaxlabs/foundation/issues/68#issuecomment-6035220831).
  The stream decides what a dropped future cancels, and the caller passes no flag:
  only the message of that future. A message of `try_send` stays. Decided by
  `laptop.architect-2`
  (https://github.com/synnaxlabs/foundation/pull/1490#issuecomment-6043749494,
  2026-10-07T17:59:23Z), #1533. Two cases follow. A `send` or `send_parts` that
  panics after `finish` drops its future, and that drop changes nothing. A future
  that leaks, as with `mem::forget`, cancels nothing: the stream writes the rest of
  its message when the caller uses the sender again, and the drop of a later send
  that waits behind that message changes nothing. Lost: a reset of the stream at
  that drop. No byte of the dropped send went, so the rule above says that it
  changes nothing (`laptop.architect-2`, 2026-10-11T12:39:23Z:
  https://github.com/synnaxlabs/foundation/pull/2399#issuecomment-6109111453).
  One case is outside the rule for a future that leaks. In a build that unwinds, a
  `send_parts` that panics on a range drops its future. When it is the first use of
  the sender after the leak, that drop cancels the leaked message as its own. The
  panic is a defect of the caller, and a release build aborts at it. Lost: a lookup
  of the stream ahead of the range check on each send, and state in the future
  (`laptop.architect-2`, 2026-10-11T15:05:16Z:
  https://github.com/synnaxlabs/foundation/pull/2399#issuecomment-6110385022). A `send`,
  `try_send`, `send_parts`, or `try_send_parts` that gives `Error::TooLarge` is a use of
  the sender: it sends nothing of its message, and the stream writes the rest of a
  leaked message from then, with no later use. Lost: a rest that waits for the next use
  that passes the size check. The use shows that the future is gone, and only the path
  of the error pays for the hand-over (`laptop.architect-2`, 2026-10-11T17:14:00Z:
  https://github.com/synnaxlabs/foundation/pull/2399#issuecomment-6111576412).
