- **LINUX CI (2026-10-05)** For the alpha, tests run only on Linux (x86-64 and ARM).
  No CI job runs on macOS or Windows. The design stays cross-OS: each C9d target must
  still be a valid build, so OS-specific code goes only in `os`. The person said: "As
  long as our systems are designed to cross compile i'm ok wiht only testing against
  linux for an alpha. as long as the system is designed for cross os deployment"
  (#574).
  Amended (the person, 2026-10-16, relayed word for word by `laptop.monitor`,
  2026-10-16T23:14:36Z, https://github.com/synnaxlabs/foundation/issues/2954#issuecomment-6226448224:
  "not cross compilation but full 3 platform support"): each PR also runs Clippy and
  the tests of the workspace on macOS (`macos-latest`, arm64) and Windows
  (`windows-latest`, x86-64), in the required jobs `macos` and `windows` of `ci.yaml`.
  Loom, shuttle, Miri, the sanitizers, fuzzing, mutants, and the root tests stay on
  Linux. A crate or test that fails on one of these OSes is an exception of that job,
  with the open issue that removes it. The list only shrinks, and the ONE NODE
  milestone does not close while it holds a crate. OS-specific code still goes only in
  `os`.
