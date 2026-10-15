- **OPCUA READ (#2524)** The `opcua` kind reads the Value of all its nodes in one Read
  request per tick, built once for a session (`client::Nodes`). The limit of one message
  is known only after the connect: the server gives its buffer sizes, MaxMessageSize,
  and MaxChunkCount in the ACK, and the server picks the token whose encoded size each
  request carries. open62541 sends the first chunks of a message before it finds that
  the message passes a limit, and the channel then closes. So the client checks the
  request, and the answer of a Double for each node, against the limits of the channel
  right after the connect, and refuses a list past them. The copy of open62541 gives
  those limits and the encoded size of the token through one more key of
  `UA_Client_getConnectionAttributeCopy` (`docs/dependencies.md`, "Local patches"). A
  lost session ends the run, so a session is a run, and the check is the first step of
  each run. Its error is a config error (`kind::Error::Config`, #435 PR 4): the
  connector stops until the next change of its spec. A read of N nodes allocates a fixed
  count plus one per node, as the decoder of open62541 allocates the scalar of each
  value; zero per node is deferred to #2537 (trigger: exit test 1 of #1737 passes).
  `read` gives a slice of a buffer of the client, so the late answer of a dropped read
  writes only into memory of the client. Lost: a check at config time against the floor
  of 8192 bytes (it refuses about 400 nodes by number on each server); a check during
  the encode (the channel closes); a check before each send (it sizes each request on
  each tick); a split of the nodes into as many requests as one message takes (trigger:
  a user with more nodes per connector than one message of the server takes). The check
  sizes each answer value as a Double and each chunk header for the security mode None:
  the change that reads a value larger than a Double also changes `shim_read_answer` and
  `SPLIT`, and the change that adds a mode also changes `HEADER`. open62541 moves a
  value of a fixed size that does not fit whole to the next chunk, and splits a string.
  The largest such value of a Read today is a Double (8 bytes), so the check takes 7
  bytes off each chunk but the last (`SPLIT`). The answer takes it on trust: a server
  that leaves more gives a read near the bound `BadSecureChannelClosed`. The change that
  adds a GUID key also changes `SPLIT`, to 18: a test of the client fails to build until
  it gives the size of each form of a key. Decided by `laptop.architect-2`
  (2026-10-13T03:51:25Z:
  https://github.com/synnaxlabs/foundation/issues/2524#issuecomment-6137826207, the
  token size at 2026-10-13T03:52:06Z:
  https://github.com/synnaxlabs/foundation/issues/2524#issuecomment-6137834758, and the
  trust of the answer in `SPLIT` at 2026-10-13T05:07:37Z:
  https://github.com/synnaxlabs/foundation/pull/2541#issuecomment-6138683801). The
  `SPLIT` of a GUID key: `laptop.architect-2` at 2026-10-13T05:31:13Z,
  https://github.com/synnaxlabs/foundation/pull/2541#issuecomment-6138961019, and the
  rule of `SPLIT` and the test of each form of a key at 2026-10-13T05:45:02Z,
  https://github.com/synnaxlabs/foundation/pull/2541#issuecomment-6139125974. The
  trigger of a value larger than a Double: `laptop.architect-2` at
  2026-10-13T22:01:26Z,
  https://github.com/synnaxlabs/foundation/pull/2613#issuecomment-6156112949.
  Supersedes the trigger of another type, approved in item 2 of
  https://github.com/synnaxlabs/foundation/pull/2541#issuecomment-6138683801.
  A close or reset of the stream by the server ends the connect, or the session, with
  an error status and no new dial, and the supervisor paces the next run
  (`laptop.architect-2`, 2026-10-13T16:56:50Z:
  https://github.com/synnaxlabs/foundation/issues/2578#issuecomment-6150745226). The
  status is `BadConnectionClosed`, which the copy of open62541 sets
  (`docs/dependencies.md`, "Local patches"). The client still dials again for a switch
  of the endpoint that open62541 makes in one connect (the URL of FindServers, or the
  security mode, the URL, or the certificate of GetEndpoints), and falls back to the
  given URL when that dial fails. The status and this reading of the switch:
  `laptop.architect-2`, 2026-10-13T20:30:54Z,
  https://github.com/synnaxlabs/foundation/pull/2593#issuecomment-6154599442. Each
  dial, the fallback too, goes to the address of the given URL. A message that the
  client cannot process on an open channel also ends the connect, or the session, with
  no new dial; its status is #2592 (`laptop.architect-2`, 2026-10-13T19:25:56Z:
  https://github.com/synnaxlabs/foundation/issues/2592#issuecomment-6153426912).
  The read gives each value, or its status, or the OPC UA type of a value that is not
  a scalar of a sample type; the kind leaves out each value that is not Good or not of
  the type of its channel (OPCUA KIND; `laptop.architect-2`, 2026-10-14T02:31:51Z:
  https://github.com/synnaxlabs/foundation/issues/2622#issuecomment-6159574954).
