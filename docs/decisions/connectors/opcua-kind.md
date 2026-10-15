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
