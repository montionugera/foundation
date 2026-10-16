- **LINE TEXT (#1098)** `connector_influx::line::Measurement::new`, and
  `Measurement::field` for a field key, accept only text that InfluxDB 1, 2, and 3 each
  store as written, in each part (measurement name, tag key, tag value, field key).
  They refuse the rest at construction, so the error reaches the config diagnostic in
  place of a partial write that InfluxDB answers with 204 or drops. `laptop.architect-2`
  moved the field key to `Measurement::field` on 2026-10-13T07:36:47Z
  (https://github.com/synnaxlabs/foundation/pull/2552#issuecomment-6140691273). One set
  of refused characters holds for every part: a backslash, a newline, a carriage return,
  a tab, NUL, U+FFFD, and each character outside the general categories L, M, N, P, and
  S other than U+0020. The last two are what InfluxDB 1 and 2 with `validate-keys` drop
  (`unicode.IsPrint` false, or `unicode.ReplacementChar`). The categories come from
  `unicode-properties` at Unicode 17.0.0, which a test pins. InfluxDB reads them from
  the Unicode tables of the Go release that built each server (Unicode 15.0.0 for Go
  1.26 today), so a code point assigned after that version passes here and that server
  drops it. No client closes this gap exactly. NUL in a tag value is refused, though
  InfluxDB 3 keeps it. No user needs it, and a user learns one rule, not four.
  Foundation names hold only ASCII letters, digits, `_`, `-`, `.`, and `@`, so the rule
  applies only to text that a user writes in the connector's config. Lost: a rule for
  each part. It keeps NUL in tags for no caller, and the set a user may write then
  depends on the part. Decided by the architect (`laptop.architect-2`) on
  2026-10-07T06:26:41Z
  (https://github.com/synnaxlabs/foundation/issues/1098#issuecomment-6032314177); the
  class by the architect on 2026-10-07T20:31:16Z
  (https://github.com/synnaxlabs/foundation/issues/1098#issuecomment-6046284871), and
  the crate by the person on 2026-10-08T01:48:32Z
  (https://github.com/synnaxlabs/foundation/issues/1098#issuecomment-6050501586).
  A `u8`, `u16`, or `u32` sample is an `i` integer. A quality channel
  (`DataType::Quality`) is an `i` field, as each `u32` channel is (`laptop.architect-2`
  on 2026-10-16T10:04:20Z:
  https://github.com/synnaxlabs/foundation/pull/2855#issuecomment-6213080870). The
  person has this choice: sent to `laptop.monitor` as a key choice of kind (e) on
  2026-10-16T09:45:11Z (message `xixh5fcx`), as finding 5 of
  https://github.com/synnaxlabs/foundation/pull/2855#issuecomment-6212747967 asks. The
  link of a quality field to its data channel waits on
  https://github.com/synnaxlabs/foundation/issues/2510#issuecomment-6212750985. A `u64`
  sample is a `u` integer (`line::Value::Unsigned`) to a `bucket`, as InfluxDB 2 and 3
  store it. To a `database`, the kind refuses a `u64` channel with `Error::Config` and
  `influx.unsigned` where the run first learns the type, before it writes a line of that
  frame: InfluxDB 1 OSS refuses `u`, and an `i64` holds no value above `i64::MAX`. The
  kind does not know the server version, so the target stands for it: a `database` is
  InfluxDB 1, and a `bucket` is InfluxDB 2 or 3 (INFLUX KIND). So a `database` on
  InfluxDB 2 or 3 keeps the InfluxDB 1 rule, and the fix of `influx.unsigned` names a
  `bucket`. One InfluxDB 2 or 3 server thus has two rules, one for each target. A
  `bucket` on InfluxDB 1.8, which also serves `/api/v2/write`, is outside INFLUX KIND,
  which takes a `bucket` for InfluxDB 2 and 3 only: it gets `u` lines that it refuses,
  the run sends the batch again until a person acts, and the person writes to InfluxDB 1
  with a `database`. The rule depends on the target, never on the value. When
  `connector::Kind::check` gets the types of the channels that a reader selects, the
  refusal moves to plan. Lost: each unsigned value as `i`, with an error at a sample
  above `i64::MAX`, as one value then stops the ack until a person acts, also on
  InfluxDB 2 and 3; a `u64` as a float or as its bits in an `i64`, as each changes the
  value with no error; a `u64` as a string field, as its queries then differ from each
  other integer; and an `unsigned` setting, as the target already gives the rule. The
  refusal to a `database` and each Lost item before this sentence, other than the reason
  of the last: `laptop.architect-2` on 2026-10-08T08:21Z
  (https://github.com/synnaxlabs/foundation/issues/1210#issuecomment-6055802089). The
  reason of the last, which supersedes "as it gives one server two rules" of that
  comment: `laptop.architect-2` on 2026-10-16T10:04:20Z
  (https://github.com/synnaxlabs/foundation/pull/2855#issuecomment-6213080870). The `u`
  integer to a `bucket`, and the rule by target: the type table of `laptop.architect-2`
  on 2026-10-15T16:13:56Z
  (https://github.com/synnaxlabs/foundation/issues/2555#issuecomment-6196722652),
  approved by the person without arrays on 2026-10-15T16:51:53Z
  (https://github.com/synnaxlabs/foundation/issues/2555#issuecomment-6197478869). For a
  `bucket`, it supersedes three parts of
  https://github.com/synnaxlabs/foundation/issues/1210#issuecomment-6055802089: "the
  writer writes no `u` integer", the refusal of `u64` to each target, and the removal of
  `line::Value::Unsigned`. Lost: the HTTP front of `sim` refusing `u` on `/write` only,
  as InfluxDB 2 takes `u` on both paths, so the server version, not the path, gives the
  rule (`laptop.architect-2` on 2026-10-16T09:09:18Z:
  https://github.com/synnaxlabs/foundation/issues/2555#issuecomment-6212098264). Lost:
  the run learns the server version at its start, from the `X-Influxdb-Version` header
  of `GET /ping`, and the rule follows the server, as it adds a request and a failure
  before the first write, and a proxy can leave out the header, so the target still
  gives the rule then. It comes back when a user writes to a `bucket` on InfluxDB 1.8,
  or a `u64` channel to a `database` on InfluxDB 2 or 3. This Lost item and its trigger,
  the text on a `bucket` on InfluxDB 1.8, and the two rules of one InfluxDB 2 or 3
  server: `laptop.architect-2` on 2026-10-16T10:18:37Z
  (https://github.com/synnaxlabs/foundation/pull/2855#issuecomment-6213313128).
