- **LINUX CI (2026-10-05, rewritten 2026-10-17)** Each PR runs Clippy and the tests of
  the workspace on Linux (x86-64), macOS (arm64), and Windows (x86-64), in the required
  checks `check`, `macos`, and `windows`: the matrix of the job `check` in `ci.yaml`.
  Linux on ARM runs in `arm.yaml`. Each job names its runner image (`ubuntu-24.04`,
  `macos-15`, `windows-2025`), never `-latest`, so that only a PR changes the OS
  release of a runner (GitHub still updates the tools of each image each week). Only
  Linux runs the format, layer, global, oracle, feature, and dependency checks, loom,
  shuttle, Miri, the sanitizers, fuzzing, mutants, and the root tests.
  - Each OS lists what fails on it in its entry of the matrix of the job `check`, each
    item with the open issue that removes it: the crates that do not build with all
    their targets, the crates whose tests fail, and the tests that fail. An item is only
    a failure whose value on that OS no test can state yet: a cause that is not known,
    or a defect that its issue fixes. A test whose value on the OS is known asserts it
    with a `cfg` on its expected value. The PR that closes an issue removes each item
    that names only that issue, and removes the issue from an item that names others.
    The lists only shrink. The ONE NODE milestone does not close while a list holds a
    crate or a test.
  - For `/review`, CI does not run an OS for a crate or a test on the list of that OS
    (`laptop.director`,
    https://github.com/synnaxlabs/foundation/pull/2960#issuecomment-6229528745).
  - OS-specific code goes only in `os`. Each `cfg` that leaves an OS out of a module or
    a public item of `os` names the issue that removes it, unless it picks the code of
    each OS (as `allocate` and `net::unsent` do) or the item is a test seam behind
    `sim` that only the tests of `os` call (as `memory::available_under` reads the
    cgroups of Linux). A test of a feature that only one OS has follows
    `docs/claude/testing.md`.

  History and sources:
  - The person, 2026-10-05 (#574), so CI ran only on Linux: "As long as our systems
    are designed to cross compile i'm ok wiht only testing against linux for an alpha.
    as long as the system is designed for cross os deployment".
  - The person, 2026-10-16, relayed word for word by `laptop.monitor` at
    2026-10-16T23:14:36Z
    (https://github.com/synnaxlabs/foundation/issues/2954#issuecomment-6226448224):
    "not cross compilation but full 3 platform support".
  - `laptop.monitor`, the body of #2954 at 2026-10-17T00:25:02Z: the pinned images,
    the lists with their issues, "The list only shrinks", the ONE NODE condition, and
    loom, shuttle, Miri, the sanitizers, and mutants on Linux only.
  - `laptop.director`, 2026-10-17T03:54:33Z, the form of the lists and what an item
    may be
    (https://github.com/synnaxlabs/foundation/pull/2960#issuecomment-6229309642).
  - `laptop.architect-2`
    (https://github.com/synnaxlabs/foundation/issues/2954#issuecomment-6226439164):
    `os` has no exception, and each `cfg` that leaves an OS out of a module of `os`
    names its issue.
  - `laptop.architect-2`, the public items of `os` and the two exceptions of the `cfg`
    rule (https://github.com/synnaxlabs/foundation/pull/2960#issuecomment-6229653306),
    and the narrowed second exception
    (https://github.com/synnaxlabs/foundation/pull/2960#issuecomment-6229892713).
  - Fuzzing and the root tests on Linux only, lists of tests, the review rule above,
    and the ONE NODE condition on tests: `box1.builder-1` in #2960, sent to
    `laptop.monitor` as key choices, and accepted by `laptop.monitor` at
    2026-10-17T03:43Z
    (https://github.com/synnaxlabs/foundation/pull/2960#issuecomment-6229189616).
  - The format, layer, global, oracle, feature, and dependency checks on Linux only:
    `box1.builder-1` in #2960, accepted by `laptop.monitor`
    (https://github.com/synnaxlabs/foundation/pull/2960#issuecomment-6229520072). The
    goal of #2954 asks for a build and the tests on each OS. All but the feature check
    read the source and the metadata, which are the same on each OS. The feature check
    builds each feature of each crate, and Clippy and the tests on each OS build all
    features at once. Lost: a set of features that fails only on macOS or Windows
    passes. On the condition of that acceptance, the dependency check covers the macOS
    and Windows targets: `deny.toml` sets no `targets`, so `cargo deny` checks the
    dependencies of each OS.
