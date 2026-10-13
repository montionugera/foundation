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
  the change that reads another type or adds a mode also changes `shim_read_answer` or
  `HEADER`. Decided by `laptop.architect-2` (2026-10-13T03:51:25Z:
  https://github.com/synnaxlabs/foundation/issues/2524#issuecomment-6137826207, and the
  token size at 2026-10-13T03:52:06Z:
  https://github.com/synnaxlabs/foundation/issues/2524#issuecomment-6137834758).
