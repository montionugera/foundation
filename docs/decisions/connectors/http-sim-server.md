- **HTTP SIM SERVER (#1151)** `connector::http::sim::serve(listener, tasks, until,
  scheme, answer)`, behind the `connector` cargo feature `sim`, off by default, is the
  one HTTP/1.1 server of the protocol simulators of HTTP connectors. It runs `hyper`'s
  server on each stream, on its own task, with keep-alive, and gives `answer` each
  request with its whole body. A client that closes its write side after a whole request
  still gets the answer. A request that breaks HTTP gets 400, or 414 when its URI is too
  long and 431 when its head is too long, and ends its stream; an HTTP/2 preface ends it
  with no answer. Each simulator answers a `content-encoding` in its own route. It runs
  until the token `until` is cancelled or the listener fails, then drops the listener
  and closes each stream that it accepted: it takes the stream back from `hyper` and
  shuts it down, with a close_notify under TLS, then a FIN. An answer in flight can be
  cut. The close waits on no peer: a shutdown that cannot finish at its first poll, and
  a stream in its TLS handshake, drop the stream, which resets it. Bytes of the peer
  that reach a stream after its close reset it, as on a TCP stack, so a request in
  flight at the stop can end with a reset (`laptop.architect-2`, 2026-10-17T17:46:18Z:
  https://github.com/synnaxlabs/foundation/pull/3047#issuecomment-6237944753). A crash
  of the node resets each stream (`laptop.architect-2`, 2026-10-17T16:58:24Z:
  https://github.com/synnaxlabs/foundation/pull/3047#issuecomment-6237139250, item 1).
  On each return, each stream that it accepted has ended, so a conformance scenario can
  stop a device in order. It returns the listener's error, so a test server that cannot
  accept fails loud (`laptop.architect-2`, 2026-10-17T16:05:31Z:
  https://github.com/synnaxlabs/foundation/issues/2641#issuecomment-6236306639, item 1).
  When the caller drops the future, the server accepts no more streams, and each stream
  that it accepted runs on until `until` (`laptop.architect-2`, 2026-10-17T16:49:55Z:
  https://github.com/synnaxlabs/foundation/pull/3047#issuecomment-6236993925). Each
  protocol simulator that a conformance scenario stops takes `until` as a
  `cancel::Token`, stops and closes at it as this server does, and returns the error
  of its listener. Its doc says what a drop of its future does. The modbus simulator
  moves to it in PR 3 of #2641 and the opcua simulator in PR 4 (6237139250, item 2;
  `laptop.architect-2`, 2026-10-18T02:15:38Z:
  https://github.com/synnaxlabs/foundation/issues/2641#issuecomment-6244627075).
  Supersedes the rule of a dropped future of
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042958763.
  `hyper`'s server reads OS wall time on each poll (hyper 1.12.0, `common/date.rs`)
  only for the `date` header, which is off. A timer reads its own `Instant` to arm
  the header read timeout, which is off too (`header_read_timeout(None)`). The person
  approved the server on the condition that it gets no timer. Lost: our own server on
  `httparse`, which the person refused; and a copy of the server in each kind crate.
  Decided by architect-2 (2026-10-07T16:33:22Z
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042291321,
  2026-10-07T16:41:29Z
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042446508,
  2026-10-07T17:08:29Z
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042839816,
  2026-10-07T17:14:34Z
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042958763,
  2026-10-07T17:22:22Z
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6043097777) and the
  person (2026-10-07T17:04:29Z
  https://github.com/synnaxlabs/foundation/issues/1151#issuecomment-6042756353).
  Supersedes: https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042446508
  (items 1 and 4, by the person's ruling and 6042839816; item 9, by 6043097777 item 5),
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042839816 (its 400
  and 415 sentences, by 6043097777 items 4 and 5; its `# Errors` section, by
  6042958763),
  https://github.com/synnaxlabs/foundation/pull/1473#issuecomment-6042291321 (the doc
  of finding 1, by 6042446508 item 1 and 6042839816).
  With `Scheme::Tls(certificate)`, it is the same server under TLS 1.2 or 1.3,
  with one of the fixed test certificates of `crates/connector/src/http/sim/tls/`,
  whose README gives the commands and each validity window; with `Scheme::Plain`, it
  serves plain HTTP. One `serve` replaces a `serve` and `serve_tls` pair
  (`laptop.architect-2`, 2026-10-17T16:58:24Z:
  https://github.com/synnaxlabs/foundation/pull/3047#issuecomment-6237139250, item
  3). The enum, not an `Option<Certificate>`, because each caller passes it as a
  literal (`docs/claude/rust.md` r16 6; round 1 of #3286:
  https://github.com/synnaxlabs/foundation/pull/3286#issuecomment-6256302557). Its
  variants are `Plain` and `Tls`, not `Http` and `Https`, which repeat the namespace
  (`laptop.architect-2`, 2026-10-18T23:07:37Z:
  https://github.com/synnaxlabs/foundation/pull/3286#issuecomment-6256311087).
  Supersedes the `serve_tls` of
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210550936 and of
  item 2 of https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210764044.
  Each certificate is for the host `server` or its IP address, so no kind is named
  (`laptop.architect-2`, 2026-10-16T07:51:33Z:
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210764044, item 2).
  `sim::ca()` gives roots that hold the test CA. A stream whose handshake fails ends
  with no answer. The server sends no session ticket and checks no client certificate,
  so it reads no time. Each key is Ed25519, so the size of each handshake packet is the
  same on each run of one simulation. Each test sets the mesh time of its client inside
  or outside the validity of a certificate, so no test depends on the date it runs
  (`laptop.architect-2`, 2026-10-16T03:30:01Z:
  https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6207437988,
  item 5).
