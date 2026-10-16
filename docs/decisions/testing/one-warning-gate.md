- **ONE WARNING GATE (2026-10-16)** The `check` job, on the pinned stable (`cargo
  clippy --workspace --all-targets --all-features -- -D warnings`), is the one gate
  that fails on a warning. The jobs on the pinned nightly (`miri`, `sanitizers`,
  `fuzz`) check what they exist for (undefined behavior, memory errors, crashes) and do
  not fail on a warning: a builder cannot run that nightly on the laptop, so such a
  failure would not be reproducible, and those jobs already end before `check`. A bump
  of the stable pin fixes each warning that the new release gives, in the bump PR. The
  Clippy step of `fuzz` keeps `-D warnings` for the `fuzz/` workspace, which `check`
  does not see, and the Clippy step of `loom` keeps it for `--cfg loom`. Decided by
  `laptop.monitor` under the CI delegation of the person, with the person's agreement
  at 2026-10-16T02:17Z ("Ok, that's fine"):
  https://github.com/synnaxlabs/foundation/issues/2798.
