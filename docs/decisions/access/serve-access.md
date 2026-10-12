- **SERVE ACCESS (#1744, 2026-10-12)** `ops::serve` checks the request of a subject with
  `access::Rules`, built from the definitions of the spec in use: the same spec that the
  operation then compares, so `ops` holds no rules of its own. A name of the check is
  the tree key of a definition, such as `plant.@access` for an access policy and the
  label for a channel or a connector. `plan` needs the action `plan` on each name of the
  files, and on each name of the spec in use that `plan` compares
  (`config::plan::planned`). A founding definition, which `plan` leaves out (FIRST
  ADMIN), is not in the check. Lost: a check on the changed names alone, with which the
  refusal depends on the content of the spec, so a subject learns if a definition that
  it has no right on is there, or equals its guess. `apply` needs `apply` on the tree
  key of each definition that the plan adds, changes, or removes. `serve` reads those
  names from the plan alone, so the refusal depends only on the request and the rules.
  The apply gives a home of the plan only to an index on which the subject has `apply`,
  and drops each other home with no error. So a subject with a grant on part of the tree
  applies the files of the whole mesh. Lost: `apply` on each index of the homes, with
  which such a subject applies no plan, because the homes list each placed index; and a
  check of only the indexes with no home, with which the refusal depends on the state
  (`laptop.architect`, 2026-10-12T18:28:19Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6130192057). The
  rules are those of the spec in use, which is the base of each plan that applies, so a
  plan that removes the subject's own policy passes when that policy gives the subject
  `apply` and `admin` on its key. The check runs before each other check that reads the
  spec in use (`ops.behind`, `ops.stale-plan`), so a subject with no grant does not
  learn from another error what the spec holds. An error of the files alone
  (`Documents::read`, the names of `config::check`) comes before the check. A refusal is
  `ops.denied`, status 1, with the fix `Ask an admin of the region for an access policy
  that allows it`. It names a name only when the request holds it. For `plan`, it names
  the first name of the files with no grant, in the order of the tree key. When only a
  name of the spec in use has no grant, it names the action and no name. For `apply`, it
  names the first changed name with no grant. `apply` also needs `admin` on the key of
  each definition that the plan adds, changes, or removes and that is not a channel, and
  the refusal names the first such key with no grant. A policy of each kind governs its
  whole region whatever its key, a subject's keys decide who acts as it, a region record
  makes a region, and a connector runs on a node that it names and reaches channels by
  name and by `select`, so with `apply` alone a grant on a subtree gives each name of
  the region. `apply` also needs `apply` on the channel at each edge of each channel
  that the plan adds or changes, before the `admin` check, and the refusal names the
  first such channel with no grant. An edge makes the channel at its end hold the time,
  the quality, the error bound, or the control handoffs of the changed channel. A kind
  that a later PR adds needs `admin` until a ruling adds it to the kinds that `apply`
  alone covers. `admin` also governs a join ticket and the creation of a region (K5).
  `plan` needs no `admin` (C8 has `plan` list access changes apart). Limits: `admin` on
  the key of a policy or a subject gives each name of its region. A policy whose
  `subjects` select names under a subtree also reaches a connector that a holder of
  `admin` adds there. Lost: no `admin` check, with which a grant of `apply` on any
  subtree is a grant on the region; and a check of each name that the old and the new
  `select` reach, because `**` reaches names that the spec does not hold yet; and a
  check that the `select` of a policy stays under the subtree of its key, with which a
  subtree keeps its own retention and compression policies, because a policy reaches its
  whole region by design; and `apply` on each channel that a connector reads or writes,
  with which a subtree keeps its own connectors, because the `select` of a connector
  reaches names that the spec does not hold yet, and its node and its server are under
  no grant (`laptop.architect`, 2026-10-12T19:56:48Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6131596199, and
  2026-10-12T20:47:24Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6132338060, and
  2026-10-12T20:59:14Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6132506754, which
  supersede "`admin` is no part of the check" of
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106994274). #1213
  checks the `apply` names, the edges, the `admin` keys, and the index of each home of
  the change record on each member and then removes that check from `ops`. `plan` makes
  no change record, so its check stays in `ops`. Decided by `laptop.architect`
  (2026-10-11T08:10:05Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106994274), on
  decision 5 of plan revision 4
  (https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913). A
  request with no name to check is refused for each subject, with `ops.denied` and no
  name: an `apply` whose plan has no change and, after the drop, no home, and a `plan`
  with no files on a spec in use with no compared name. Such a request changes nothing.
  A node that uses no spec has no rules, so it refuses each request of a remote subject,
  also of an admin. The local `plan` and `apply` on that node give `ops.behind`. Lost: a
  pass, with which a subject with no grant learns the pointer in use; the action on the
  whole region, because `access` gives the grant of one name, and a check of the whole
  region would serve only this case; and `ops.behind` before the check, because its
  cause names a key of the newest spec (`laptop.architect`, 2026-10-12T19:54:00Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6131554034). Limits:
  an admin gets the refusal too, with the fix to ask an admin. The rules take the root
  prefix, since a node founds only a root region. Trigger: the PR that lets a node found
  or join a region below the root gives `serve` the prefix of that region.
