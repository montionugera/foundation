- **R7 starting points** OPC UA: open62541 compiled in, with our own crypto plugin on
  aws-lc or compiled-in mbedTLS. Modbus, MQTT with Sparkplug B (pass the TCK), and Kafka
  (pure Rust on `kafka-protocol`, rdkafka behind a flag): built sans-I/O. DAQmx and LJM:
  runtime-loaded bindings, NI functions declared by hand. Codecs: built. Crypto: rustls
  with aws-lc-rs and blake3. Tooling: clap, schemars, toml_edit, tracing. Our own thin
  MCP server, Prometheus text output, and InfluxDB line protocol. FIPS build later.
  HTTP: one client for all connectors, on `hyper` (HTTP/1.1 and HTTP/2) over the `env`
  network seam with `rustls`, in the connector component library. InfluxDB, a general
  HTTP connector, alarms, webhooks, and remote write use it. No HTTP parser of our own.
  Every clock read and name lookup of the client goes through `env`, and no Tokio
  feature of `hyper` or `hyper-util` is on. TLS takes a configured CA, and no setting
  turns verification off. Lost: a sans-I/O HTTP/1.1 module in `connector-influx`. The
  person decided on 2026-10-06 ("Approved." "Adding a bunch of crates is fine. Making a
  binary larger is fine." "we should be careful about writing raw HTTP transports.",
  relayed by `advisor`; "Yes I approve", to the coordinator) (#341). #983 (an `httparse`
  reader) closed: the person told `connector` to use the `hyper` client on 2026-10-06.
  `httparse` comes in only as a dependency of `hyper`. The client is HTTP/1.1 only for
  now: `h2` 0.4 reads the OS clock to expire a reset stream, so HTTP/2 turns on only
  when `h2` takes its clock through `env`, by an upstream change. Decided by the
  coordinator with `advisor` on 2026-10-06 (#341). The client keeps one idle connection
  for each origin. It does not reuse one that is idle longer than 90 s (the `hyper-util`
  default), read on the `env` clock, and the next send closes it: the client sends no
  keep-alive, and a firewall or NAT may drop the state of an idle stream. Decided by the
  coordinator with `advisor` on 2026-10-06
  (https://github.com/synnaxlabs/foundation/issues/341#issuecomment-6022322924). A
  request that fails on a reused connection before its response goes once more on a new
  connection, when the connection did not write it, or when its method is idempotent and
  no byte of a response came (RFC 9112, as in Go). The pool key is the origin, and it
  keeps the host name, because a TLS connection is verified for one name and must never
  carry a request for another. Decided by the architect on #1111
  (https://github.com/synnaxlabs/foundation/pull/1111#issuecomment-6031412223). The key
  is the host name in lower case and the port. `influx.` and `influx` are two keys,
  because a resolver may expand a name with no final dot. The key also holds the
  scheme, so an `http` request never goes on an `https` connection to the same host and
  port. A new connection looks up the host
  through `env` and tries each address in order, as Go does: each address gets an equal
  share of the time left to the deadline, but at least 2 s or all that is left. A
  refused address moves the dial to the next at once, and no connect starts once the
  time is up. A reused connection does no lookup. Lost: Happy Eyeballs (RFC 8305), which
  needs more code and streams; a separate error variant for a failed lookup, which a
  caller handles as a failed connect; no limit for each address, where one that drops
  the SYN uses the whole timeout. Proposed by `connector` in the plan on #341
  (https://github.com/synnaxlabs/foundation/issues/341#issuecomment-6031334051) and in
  the review of #1135
  (https://github.com/synnaxlabs/foundation/pull/1135#issuecomment-6031807435,
  https://github.com/synnaxlabs/foundation/pull/1135#issuecomment-6031903363,
  https://github.com/synnaxlabs/foundation/pull/1135#issuecomment-6031982713). The key
  text after the #1111 link, the dial rule, and the `Error::Connect` doc: approved by
  `laptop.architect-2` on 2026-10-07
  (https://github.com/synnaxlabs/foundation/pull/1135#issuecomment-6032674524).
  A refused URI gives one error for each cause: `Scheme`, `UserInfo`, `Host`, and
  `Port`, checked in that order.
  `Host` holds the `env::net::host::Error` of the host, which `env::net::host::uri`
  reads (ENV SEAMS), and gives "the URI has <phrase>" with that error as its source.
  A zone in brackets follows `%25`, as `[fe80::1%252]` for zone 2, and the `Host`
  header of the request has no zone (RFC 6874). Amended in the review of #2804
  (2026-10-16 04:05 UTC,
  https://github.com/synnaxlabs/foundation/pull/2804#issuecomment-6207830826; the
  move to `host::uri` and the header, 04:34 UTC,
  https://github.com/synnaxlabs/foundation/pull/2804#issuecomment-6208160301).
  Text after `]` is part of the host up to a `:`, so `http://[fd00::2]8086/` gives
  `Host` (https://github.com/synnaxlabs/foundation/issues/1179#issuecomment-6032542190).
  A `[` in a host that is not in brackets gives `Host`. A built URI with an empty path
  sends `/`. Lost: one `Uri` variant that holds the URI with its user info removed,
  because the message must name the cause; and a `Uri` whose `Display` drops the user
  info, because `Debug` and the field still hold the password. Decided by
  `laptop.architect-2` on #1159
  (https://github.com/synnaxlabs/foundation/issues/1159#issuecomment-6032370253).
  No error of a refused URI holds text from the URI: a `/` or `?` in a password ends
  the authority early and puts the password in the host or the port.
  `connector::http::uri` reads a config value as a URI that `send` takes, with no I/O.
  Each refusal, and a fragment, which `send` never sends, is `connector.bad-uri` at the
  value. Lost: a `check(&Uri)` and a code for each kind, which each HTTP kind repeats.
  Decided by `laptop.architect-2` on #1794
  (https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052684931,
  2026-10-08 05:03 UTC). Supersedes the `Host` and `Port` fields and messages of
  https://github.com/synnaxlabs/foundation/issues/1159#issuecomment-6032370253.
  `https` (#2702): the client runs rustls with the aws-lc-rs provider and its default
  versions, TLS 1.2 and 1.3, through `tokio-rustls` over an adapter of the `env`
  stream, and ends the handshake before `hyper` gets the stream. The port defaults to
  443. The TLS name is the host of the origin, with the final dot of a name and the
  zone of an IPv6 address dropped, so each host that `uri` takes has a name and
  `https` adds no `Host` error. Supersedes item 2 of
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210764044
  (2026-10-16T07:51:33Z), which gave `https` one more reason of `host::Error`: no host
  can give that reason (`laptop.architect-2`, 2026-10-16T09:11:38Z:
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6212137276). It trusts
  only `http::Config::tls`: a `Tls` with the roots, which `node` reads from the OS, and
  mesh time. A kind adds a CA with `Roots::with_pem`, which skips a UTF-8 byte order
  mark at the start (`laptop.architect-2`, 2026-10-16T16:17:47Z:
  https://github.com/synnaxlabs/foundation/pull/2888#issuecomment-6219779608, item 2).
  A `ca` takes a CA, or a self-signed certificate with `CA:FALSE`; a server certificate
  with `CA:TRUE` gives `Tls` with a text that names both fixes.
  This narrows design item 4 of #2702, "A self-signed server goes in `ca`"
  (2026-10-16T07:51:33Z:
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210764044).
  With no `Tls` or no root, a send gives `NoRoots` before the lookup, so a plain
  `http` client needs no hub. Each connect reads mesh time once, and rustls checks the
  certificate at that instant; the request timeout bounds its age. Each
  handshake failure is `Tls`, with the host and port, and a failure after it is
  `Protocol`. The client resumes no session. Lost: a
  `tls: Option<Arc<rustls::ClientConfig>>` from each kind, which repeats the roots and
  the time in each kind, puts a rustls type in the public surface, and lets a kind turn
  off the check; an `Unsynced` error, which the wait of `Hub::time` defines out of
  existence; a `hub::Time` in each `http::Config`, which makes each plain client open
  a hub; a TLS driver by hand inside the stream, which repeats what `tokio-rustls`
  does and which #1248 needs again in another layer; and trust of an end certificate
  that is equal to a root, which is a later step on #2894 when a user's server cannot
  get a new certificate. The influx kind reads its CA from `ca` (INFLUX KIND).
  The roots are `node::Config::roots`, an `os::Roots`, and main gives `os::roots()`:
  the certificate files on Linux and the keychain on macOS. When `SSL_CERT_FILE` is
  set, or `SSL_CERT_DIR` holds a path that is not empty, it reads only that file and
  those paths, on each OS. A path that is not there, or a `SSL_CERT_DIR` path that is
  not a directory, gives a failure and no root of the OS store. The person approved
  this override (2026-10-16T14:53:55Z:
  https://github.com/synnaxlabs/foundation/pull/2862#issuecomment-6218177938). An
  empty `SSL_CERT_DIR` names no directory, so the node reads the OS store. On Linux, a
  `SSL_CERT_DIR` of only `:` characters also reads the directory of that whole name in
  the working directory, when it is there. An empty `SSL_CERT_FILE` names a file that
  is not there, so the node trusts no OS root and logs the failure
  (`laptop.architect-2`, 2026-10-16T15:14:05Z:
  https://github.com/synnaxlabs/foundation/pull/2862#issuecomment-6218554197). Main
  reads the store once, so a change to it takes effect at the next start. Before
  `started`, shard 0 logs a warning "a part of the OS roots was not read" with the
  `error` of each of the first 16 failures, "more parts of the OS roots were not read"
  with the count `unlisted` of the rest and the `source` of the read, "OS roots that do
  not parse are skipped" with the count `skipped`, and "the OS roots hold no
  certificate, so an https connect with no CA of its connector fails". It logs them also
  with `Region::Absent`. The cap of 16 failures keeps the warnings from filling the log
  queue and dropping `started` (`laptop.architect-2`, 2026-10-16T20:40:05Z, #2920:
  https://github.com/synnaxlabs/foundation/pull/2812#issuecomment-6224274551). For the
  same reason, an `error` of more than 1024 bytes keeps at most its first and last 510
  bytes, cut at char boundaries and joined by `…`, as a line of a PEM file can be of any
  length (`laptop.architect-2`, 2026-10-16T22:26:26Z, #2920:
  https://github.com/synnaxlabs/foundation/pull/2941#issuecomment-6225870705).
  `os::roots()` gives the `source` as `os::roots::Source`: `the OS store`, or the paths
  of `SSL_CERT_FILE` and `SSL_CERT_DIR` that it read in place of the store. `os` owns
  this pick, so `node` never derives it from its `vars`, and `node` cuts the `source` as
  it cuts an `error` (`laptop.architect-2`, 2026-10-16T22:27:09Z, #2920:
  https://github.com/synnaxlabs/foundation/pull/2941#issuecomment-6225879413). This
  supersedes the warning for each failure that
  https://github.com/synnaxlabs/foundation/pull/2862#issuecomment-6213377062 approved,
  and `node::Config::roots: Vec<Vec<u8>>` of
  https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6207437988, which
  read the OS outside `os`, so its failures did not reach the node log
  (`laptop.architect-2`, 2026-10-16T10:09:09Z:
  https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6213160739; the
  order before `started`, 2026-10-16T10:22:32Z:
  https://github.com/synnaxlabs/foundation/pull/2862#issuecomment-6213377062).
  `laptop.architect-2` ruled `Tls`, `tokio-rustls`, the name `server` of the test
  host, the `CA:TRUE` text, and the refusal (2026-10-16T07:51:33Z:
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210764044).
  Decided by `laptop.architect-2` (2026-10-16T03:30:01Z:
  https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6207437988;
  2026-10-16T03:30:54Z:
  https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6207447936) and
  `laptop.architect` for `Hub::time` (2026-10-16T03:28:57Z:
  https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6207425675).
