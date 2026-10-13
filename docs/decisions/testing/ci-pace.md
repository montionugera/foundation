- **CI PACE (2026-10-06)** The ARM pool must not hold up the agents. The ARM workflow
  runs no loom step: loom is a software model, so the x86 `loom` job gives the same
  result. A PR run is cancelled by a newer push. A run on main is never cancelled while
  it runs; of the commits that merge during it, only the newest runs next. Each runner
  keeps its build in `$HOME/target/<runner>`, outside the workspace, and deletes it
  past 25 GiB. Dependencies build at opt-level 2 in the dev profile; workspace crates
  stay at opt-level 0 with their checks. A PR tests only the changed crates and their
  reverse dependencies (`cargo xtask affected`); main tests the whole workspace.
  Decided by the advisor (#782). The person said: "We need to make the agentic
  engineering the bottleneck, not CI".
  Amended (the person, 2026-10-13T16:27:23Z, recorded in
  https://github.com/synnaxlabs/foundation/pull/2584#issuecomment-6150265221, to the
  proposal of `laptop.monitor` to split mutants across 4 runners: "That's fine."): a PR
  runs its mutants on four runners. The design of #2584: four jobs `mutants shard <k>`,
  each with `cargo mutants --shard <k>/4 --sharding round-robin` on its own ARM runner,
  and the required check `mutants` passes when each shard passed. In the queue no shard
  runs, and `mutants` carries the result at the head of the PR to the group, as before.
  Each shard checks the memory cap of its runner unit (#803), so four shards at once
  take four caps that the sum check already bounds. Cost: each shard that gets a mutant
  builds and tests the code without mutants, so a PR with four or more mutants spends
  four baselines in place of one.
