- **NODE NAME (2026-10-09)** A data directory holds one node, by name. `Config::name`
  gives it. Under `lock` (DATA DIRECTORY LOCK), after the claim of the shard count,
  shard 0 removes a file `name.new` that a crash left, writes the name to a new
  `name.new`, and renames it to `name`, which syncs the file first, then syncs the data
  directory. A file `name` with the same name changes nothing. The claim has synced the
  data directory at this start, which makes good a failed sync of an earlier start:
  that sync can leave a name that a read sees but a crash loses. A read sees no file
  `name` or a whole one, also during a first start. A file `name` that does not hold a
  whole name, also one of zeros or with no bytes, is `Error::Name`: the start stops,
  and the node keeps the file. Another name stops the start with `Error::Renamed {
  stored, given }` (the CLI code is `node.renamed`, #1732). The node never writes
  another name over a name.
  A name with a segment that starts with `@` is reserved, and a region refuses it in a
  card. So `Node::start` stops with `Error::Reserved { name }` before the claim, also
  with `Region::Absent`: it starts no shard, and the data directory keeps no name that
  its founding refuses (the CLI code is `node.reserved`). `node` checks only
  `Name::reserved`, the one test of `mesh::region::fits` that the card of a root
  founding can fail. Trigger: a join by ticket (#1157), where `fits` also refuses a name
  outside the region, makes `mesh` give one check of a card name against a region, which
  `fits` calls and `node` calls before the claim. No release kept a reserved name. Only
  a build of `main` before this check could, and a start of its data directory with
  another name gives `node.renamed`, whose second fix, another data directory, is the
  way out (`--name {stored}` gives `node.reserved`). Lost: a check in `ops::cli`, which
  covers one caller and leaves `node` free to keep the name; a write of the name only
  after the region opens, which founds the region before the file `name` exists, so a
  crash between them leaves a region whose card name no file holds; a start that writes
  over a kept reserved name, a branch in `name::keep`, in three public docs, and in this
  record, for a state that no release has. Proposed in round 1 of #2331
  (2026-10-11T01:19:28Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104132104); approved
  by `laptop.architect` at `172d1b6e0` (2026-10-11T01:22:34Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104154087). The one
  test, the trigger, the way out of a kept reserved name, the reason of the write after
  the region opens, and the lost write over a kept reserved name: proposed in rounds 2
  to 4 of #2331 (round 2: 2026-10-11T01:39:35Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104275190; round 3:
  2026-10-11T01:57:16Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104399766; round 4:
  2026-10-11T02:43:09Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104716716); approved
  by `laptop.architect` at `1fc0193fe` (2026-10-11T02:45:55Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104734826).
  Until the node stores a founding, the file is the only copy of the name on the node,
  and the source of the name that the node puts in its card when it founds a region
  (#1744) or joins one by ticket (#1157). Once a founding is stored, the card of the
  node in it holds the name too. Then the stored name is the one in the file `name`,
  else the one in the card, so a start with a file `name` reads no founding: a start
  with another name gives `Error::Renamed` and writes no file, and a start whose file
  `name` is gone writes it again from the card. Decided by `laptop.architect-2`, #2355
  (2026-10-11T07:44:40Z,
  https://github.com/synnaxlabs/foundation/issues/2355#issuecomment-6106801760; the file
  first, 2026-10-11T10:19:18Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108005495).
  Supersedes "The file is the only copy of the name on the node" of
  https://github.com/synnaxlabs/foundation/pull/2172#issuecomment-6088201970.
  `name::keep` reads the founding under the lock, after the file `name` and before the
  write, through `mesh::founding` with a pool of its own (the shard's buffer is not open
  yet), and takes the card of the key in `node.key`. A founding with no card of that key
  gives no stored name. So does a data directory with no `node.key`. Lost: a check of
  the name after `Mesh::open`, through `Mesh::member`, which a node that joins by ticket
  can also use; it is a second source beside the founding that `node::name` reads, and
  it adds a mesh that opens and closes at once. A file `name` that holds a whole name
  other than the card name comes only from a write by hand; the start gives
  `Error::Renamed` with the file name. Proposed in the revised plan of #2355
  (2026-10-11T08:11:24Z,
  https://github.com/synnaxlabs/foundation/issues/2355#issuecomment-6107004445);
  approved by `laptop.architect-2` at `0dfab9514` (2026-10-11T10:19:18Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108005495) and at
  `35b06649b` (2026-10-11T10:36:03Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108134993) and at
  `b8788d740` (2026-10-11T10:47:57Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108225390); the
  sentences that state what `mesh` does, also approved by `laptop.architect` at
  `b8788d740` (2026-10-11T10:58:48Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108309120). In the
  region, `card.name` is the one copy (MEMBER RECORD). Trigger: the join by ticket
  (#1157), and the first operation that gives a member a card with a new name. From
  either, the stored name comes from the node's member in the region, not from the
  founding, and that operation also writes this file, or refuses the rename.
  Decided by `laptop.architect-2` (text 2, 2026-10-11T10:19:18Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108005495; approved
  at `35b06649b`, 2026-10-11T10:36:03Z,
  https://github.com/synnaxlabs/foundation/pull/2387#issuecomment-6108134993).
  Supersedes the trigger "the first operation that renames a node in the region also
  writes this file, or refuses the rename" of
  https://github.com/synnaxlabs/foundation/pull/2172#issuecomment-6088201970.
  The file is one sector: the tag `foundation/name/1`, the length of the name in one
  byte, the name with zeros after it up to 255 bytes, and the CRC32C of those 273 bytes
  (little-endian).
  `node::name(files, given)` gives the name before the start: the given one, else the
  stored name (the file, else the card above, #2355), else `Error::Unnamed`. It does
  not check `given`: the start does (above; approved by `laptop.architect` at
  `9deea732c`, 2026-10-11T03:18:10Z,
  https://github.com/synnaxlabs/foundation/pull/2331#issuecomment-6104953177). It reads
  only when no name is given, and opens each file to read only, so it makes nothing
  and waits for no lock. So the first start needs a name, and a later one does not
  (`laptop.architect-2`, 2026-10-08T05:00:46Z,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6052649826).
  Lost: `main` owns the name, and reads and writes the file before `Node::start`. The
  write is outside the lock, so two first starts with two names can leave the name of
  the node that lost, and #1744 founds the region with the name, which `node` must
  then have. Also lost: `Config::name: Option<Name>`, which shard 0 alone resolves.
  The line that `main` prints needs the name, and a task of `Node::spawn` gets only
  the hub, so the node needs a new way to give the name back. Also lost: the same check
  also in `node::name`, because its read is outside the lock, so only the check under
  the lock holds. Also lost: a write in place of the file, which a read during a first
  start can see in part, and a write back at each start. Also lost: a doc that says
  `node::name` can give `Error::Name` during a first start; then `foundation start`
  races to `node.name` in place of `node.busy`. Also lost: `node::name` opens `lock`
  first; that takes the lock that a start needs, and a second start fails.
  Supersedes "A file with no bytes, or with a sector of zeros, is no file", and the
  read of the stored name before the given one, of
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6086905545.
  Decided by `laptop.architect-2`, #1732: 2026-10-09T18:30:45Z, the file and the
  errors,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6086905545;
  2026-10-09T19:54:21Z, the only copy on the node, the source of the card's name, the
  trigger, the given name first, and the first lost designs,
  https://github.com/synnaxlabs/foundation/pull/2172#issuecomment-6088201970;
  2026-10-09T20:09:33Z, the write by rename, a file of zeros or with no bytes, and the
  last two lost designs,
  https://github.com/synnaxlabs/foundation/pull/2172#issuecomment-6088435494;
  2026-10-09T20:42:29Z, a file `name` with the same name,
  https://github.com/synnaxlabs/foundation/pull/2172#issuecomment-6088917341.
