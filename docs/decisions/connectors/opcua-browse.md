- **OPCUA BROWSE (#2705)** The OPC UA client browses a server and describes its
  variables for an import (`client::browse`, crate-private). `Client::browse` and
  `Client::browse_next` give the forward `HierarchicalReferences` of a node and their
  subtypes, of the classes Object, Variable, and View, a page at a time, with the type,
  the names, and the class of each reference. The node of a reference is a `Foreign`
  when its key is a GUID or opaque, or a name that is empty or not UTF-8, or it is a
  node of another server: its text in the form of OPC 10000-6, 5.3.1.10, and the cause.
  A page asks for at most the references whose answer fits one message that the client
  takes, but for at least 1. A request that does not fit one message
  that the client sends gives `BadRequestTooLarge` and sends nothing, so the session
  stays (`laptop.architect-2`, 2026-10-16T20:51:36Z:
  https://github.com/synnaxlabs/foundation/pull/2902#issuecomment-6224462055). A
  continuation point lives in its session. `Client::describe` gives the base type, the
  `ValueRank`, the `UserAccessLevel`, and the `EngineeringUnits` of each variable. The
  base is the first built-in type of namespace 0 up the inverse `HasSubtype` references;
  a subtype of `Enumeration` is Int32, as OPC 10000-3, 5.8 encodes it; `Structure`,
  `BaseDataType`, the abstract numbers, a loop, a walk of more than 16 links, and a type
  on the walk whose id is not a number or a name give none. Each step of the walk is one
  Browse of each type of the call that the client does not know yet, for at most one
  supertype each, and the client keeps the supertype of each Good result and releases
  the continuation point of a type with more. The client keeps at most 1024 supertypes
  past a call, and clears them at the end of a call with more, and when a call drops in
  its walk (`laptop.architect-2`, 2026-10-16T20:51:36Z:
  https://github.com/synnaxlabs/foundation/pull/2902#issuecomment-6224462055). A result
  that is not Good gives no base in that call, and the next call browses the type again
  (`laptop.architect-2`, 2026-10-16T17:48:08Z:
  https://github.com/synnaxlabs/foundation/pull/2902#issuecomment-6221380182). The unit
  comes from a Translate of `HasProperty` `0:EngineeringUnits`, then a Read of the Value
  of each target. The client sends one request at a time, and splits each by the limit
  of its service on the server (`MaxNodesPerRead`, `MaxNodesPerBrowse`, or
  `MaxNodesPerTranslateBrowsePathsToNodeIds`), by item (a node and an attribute) for a
  Read, and so that the request and its answer each fit one message of the session. The
  client estimates an answer with each text and byte string at 256 bytes, so a larger
  text can still make an answer past one message, and then the server closes the channel
  (`laptop.architect-2`, 2026-10-16T19:25:06Z:
  https://github.com/synnaxlabs/foundation/pull/2902#issuecomment-6223049465). A fact
  whose request alone, or its answer, does not fit is `None`, and the rest of the call
  goes on. It reads the limits of a Browse and a Translate once, at the first
  `describe`, not in the connect of a read task. The strategy and the base type:
  approved by `laptop.monitor` (2026-10-16T16:40:53Z:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6220213470). The
  limit of each service, the batched walk, the reference type, and `None` for a type
  that is not a number or a name: `laptop.architect-2` (2026-10-16T16:43:06Z:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6220251690). Unit
  code: `EUInformation.unitId` is a UN/CEFACT Recommendation 20 code only when its
  `namespaceUri` is `http://www.opcfoundation.org/UA/units/un/cefact` (OPC 10000-8,
  5.6.4), else the code is `None` and the unit comes from the display name
  (`laptop.monitor`, same link). The code is the 2 or 3 bytes of `unitId` after its zero
  bytes, each an ASCII capital letter or digit, so `P1` (`%`) reads, else `None`
  (`laptop.architect-2`, same link). The kind gives the plain name of a node to its
  first reference in the order of the server, and the skip of a cursor assumes that the
  server keeps the order of the references of a folder (OPCUA KIND). Later step, in PR
  5b2 of #2705 (#2969): bound the continuation points of a walk
  (`MaxBrowseContinuationPoints`), and test it against the sim server with the bound at
  1 and at 2. It merges before the CLI or MCP gives browse to a user (the split of PR 5,
  2026-10-16T22:54:58Z:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6226218426). The
  one table of the built-in types that this record asked for became the decode by match
  of OPCUA KIND (`laptop.architect-2`,
  2026-10-16T22:17:22Z:
  https://github.com/synnaxlabs/foundation/pull/2931#issuecomment-6225757093).
