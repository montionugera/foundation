- **OPCUA NODE (#2401)** A node of the `opcua` kind names its namespace by its index in
  the server's NamespaceArray, and that index goes on the wire as the spec gives it
  (`ns=3` in a `node_id`). A client keeps no namespace table. So the copy of open62541
  has a change: a client reads no NamespaceArray after its session activates, and its
  channel has no namespace mapping. The release reads the array once and sends each
  NodeId through a table that it builds from it. A read of a namespace that the server
  adds later then goes to another index, and an array with one URI twice moves each
  later index. The kind uses no local index: it sets no namespace and no custom data
  type in the client config. Lost: the URI as the only form (each OPC UA tool shows the
  index, and it needs a read of the array and a lookup before the first read); the
  mapping with a new read of the array after a failed read (a second guard for a table
  that the kind does not use, and an array with one URI twice still moves a read);
  `shim.c` sets the private flag `haveNamespaces` with no change of the copy (`shim.c`
  then depends on a private struct, and the table of `docs/dependencies.md` does not
  show the change). Trigger for the form `nsu=<uri>` of a `node_id`: a user whose server
  gives a namespace another index after a restart. That form is a lookup above
  `Client::read`: it reads the array for each session and gives the server's index, so
  the read and the wire do not change. Decided by `laptop.architect-2`
  (2026-10-12T16:12:25Z:
  https://github.com/synnaxlabs/foundation/issues/2401#issuecomment-6128082633). A
  `node_id` is `ns=<index>;i=<number>` or `ns=<index>;s=<name>`, with no `ns=<index>;`
  for namespace 0, and the name is the rest of the text, of 1 to 4096 bytes, so that
  a read fits in the chunks of one message. The kind refuses a GUID
  (`g=`) and an opaque key (`b=`) with an exact error. Trigger for them: a user whose
  server has a variable with no number or name key.
