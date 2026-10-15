- **CI PACE (2026-10-06)** The ARM pool must not hold up the agents. The ARM workflow
  runs no loom step: loom is a software model, so the x86 `loom` job gives the same
  result. A PR run is cancelled by a newer push. A run on main is never cancelled while
  it runs; of the commits that merge during it, only the newest runs next. Each job runs
  on a fresh GitHub-hosted VM, and `Swatinem/rust-cache` keeps the build between runs
  (#2727, ARM RUNNER). Dependencies build at opt-level 2 in the dev profile; workspace
  crates stay at opt-level 0 with their checks. A PR tests only the changed crates and
  their reverse dependencies (`cargo xtask affected`); main tests the whole workspace.
  Decided by the advisor (#782). The person said: "We need to make the agentic
  engineering the bottleneck, not CI".
  Amended (the person, 2026-10-13T16:27:23Z, recorded in
  https://github.com/synnaxlabs/foundation/pull/2584#issuecomment-6150265221, to the
  proposal of `laptop.monitor` to split mutants across 4 runners: "That's fine."): a PR
  runs its mutants on four runners. The design of #2584: four jobs `mutants shard <k>`,
  each with `cargo mutants --shard <k>/4 --sharding round-robin` on its own runner, and
  the required check `mutants` passes when each shard passed. In the queue no shard
  runs, and `mutants` carries the result at the head of the PR to the group, as before.
  Each shard runs on its own hosted VM (`ubuntu-latest`, #2727). `cargo mutants` runs in
  a systemd scope with a memory cap, no swap, and `OOMPolicy=continue`, so a mutant that
  allocates in a loop kills only its test, and the shard reports the mutant. Cost: each
  shard that gets a mutant builds and tests the code without mutants, so a PR with eight
  or more mutants spends eight baselines in place of one.
  Amended (`laptop.monitor`, 2026-10-15, for the person: "Each mutant shard is taking
  45 min + in some cases."): eight shards in place of four, `--shard <k>/8`, as hosted
  runners are free. The test timeout is 3 times the test time of the baseline
  (`--timeout-multiplier 3`), not the default 5, so a mutant that hangs costs about 14
  min, not 23.5. Four jobs on 4 vCPU can run slower than the baseline, so not lower.
