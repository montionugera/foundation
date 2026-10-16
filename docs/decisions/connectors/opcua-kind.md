- **OPCUA KIND (#435)** Five choices of the `opcua` kind, each approved by the person
  as `box2.builder-6` recommended (2026-10-13T20:14:06Z, "All 5 decisions are approved
  for OPC UA":
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6154297722).
  Security: the kind connects with the mode None and an anonymous user now, so it is
  only for an isolated network or a test. SignAndEncrypt is the next part of #435,
  before a release, with user names, passwords, and certificates (question:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6150341828). Tests:
  the tests of the kind and exit test 1 run against the open62541 test server on the
  simulated clock and network. One more test on real I/O against `opc-plc` runs in CI
  with #1733, before a release (question:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6150342217). Types:
  the kind reads Double first, then Boolean and each numeric scalar type, each into
  the sample type of the same width and sign (Float to `f32`, Int32 to `i32`, Boolean
  to `bool`), with no conversion. A channel of another type is a config error. The
  kind reads each of these types since
  https://github.com/synnaxlabs/foundation/pull/2635. The `influx` kind writes the same
  types (#2555). String, DateTime, and arrays come when a user needs them (question:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6150342865). A
  lossless widening (an `i16` value into an `f64` channel) comes when a user's node
  types do not match the channel types that the user wants (`laptop.architect-2`,
  2026-10-14T00:36:21Z:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6158211076). Time:
  each frame has the midpoint of the request on mesh time (STAMP). The SourceTimestamp
  comes with the device clock and its fit to mesh time, when a user needs the time of
  the measurement (question:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6150343218). Status:
  a Good with a subcode counts as Good. A frame leaves out each value that is not Good
  and keeps the others: an Uncertain value adds 1 to the count `uncertain`, and each
  other one, also no value (`BadNoData`) and a `BadTypeMismatch` of the server, adds 1
  to `bad`. A tick that leaves out each value writes no frame. A quality channel comes
  with the quality model of the MVP (question:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6150343529).
  Approved by the person (2026-10-15T16:34:12Z:
  https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6197131223). A
  Good value of another type than the sample type of its channel is also left out,
  and adds 1 to `mistyped`: the user fixes it in the spec or on the server, so it is
  not in `bad`. The note names the first such node of the latest tick that has one,
  and the OPC UA type of its value, such as String, Range, or array of Double. A tick
  with none clears it (`laptop.architect-2`, 2026-10-14T02:31:51Z:
  https://github.com/synnaxlabs/foundation/issues/2622#issuecomment-6159574954, the
  type of an array or a String at 2026-10-15T19:32:35Z:
  https://github.com/synnaxlabs/foundation/issues/2622#issuecomment-6200602125, and
  the `BadTypeMismatch` of the server, the note of the latest tick, and the name of
  the type at 2026-10-15T22:16:48Z:
  https://github.com/synnaxlabs/foundation/pull/2766#issuecomment-6203414079).
  Close: after a cancel or an error, the run closes its session. It sends
  CloseSession, waits at most 1 s for the answer, then closes the connection, so a run
  ends within 1 s of a cancel. The connect races the cancel, and the close does not.
  The 1 s also holds the wait for a CreateSession in flight, so a server that answers
  it later can keep that session until it times out (`laptop.architect-2`,
  2026-10-16T02:08:17Z:
  https://github.com/synnaxlabs/foundation/issues/2622#issuecomment-6206438915, and
  its amendment at 2026-10-16T02:44:43Z:
  https://github.com/synnaxlabs/foundation/issues/2622#issuecomment-6206868666).
  Keep-alive: between ticks, the run waits through `Client::idle`, which drives the
  client. When 10 s pass with no read, `idle` reads `Server_ServerStatus_State`
  (ns=0;i=2259) to check the connection, so a run that reads more often than each 10 s
  sends no check. A check fails when its answer has a bad service result, when it gets
  no answer in the request timeout (5 s), or when the close of the channel cuts it.
  The status and the value of the server state are not judged: each answer came over
  the channel and the session (`laptop.architect-2`, 2026-10-16T05:10:17Z,
  https://github.com/synnaxlabs/foundation/pull/2809#issuecomment-6208565249). The
  first failed check ends the run with `Error::Device` and the text "the connectivity
  check failed with <status>", so the supervisor connects again with its backoff. A
  channel or a session that the client loses between two checks ends the run at once
  with the same text, from the state of the client: the status of the connect, else
  `BadConnectionClosed` or `BadSessionClosed`. The check is in Rust: open62541 calls its
  inactivity callback only for `BadTimeout` and drops each other status of its own
  check, such as the `BadSessionIdInvalid` of a lost session, so
  `connectivityCheckInterval` stays 0. The retry of a failed read stays. Approved by the
  person (2026-10-16T03:44:54Z, "Yes, approved", on "Keep the 10-second keep-alive, and
  add one rule: if a keep-alive fails, reconnect right away instead of waiting for the
  next read":
  https://github.com/synnaxlabs/foundation/pull/2809#issuecomment-6207602232). The count
  of one failed check, what fails a check, the check in Rust, the text, and the tests:
  decided by `laptop.architect-2` (2026-10-16T03:46:32Z,
  https://github.com/synnaxlabs/foundation/pull/2809#issuecomment-6207619278). The end
  at once on a lost channel or session: decided by `laptop.architect-2`
  (2026-10-16T04:04:38Z,
  https://github.com/synnaxlabs/foundation/pull/2809#issuecomment-6207818990).
  Import: the block of a point is `read "<channel>" { node_id = "<tag>" }`, which
  `parse` reads. A tag that is not a node ID that `parse` reads gives
  `opcua.bad-node-id`, and a point that a connector can only write gives
  `opcua.no-write`, since the kind does not write. One table of the built-in types
  that a read decodes, Boolean (1) to Double (11), gives the decode of a read, so the
  type of a browse reads the same table (plan of C1 PR 5, approval owed:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6224806107).
