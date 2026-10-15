- **SDK DATA PATH (adopts D12 for Python)** The Python SDK wraps the Rust SDK `sdk`
  with PyO3, so the wire, the TLS pin, the hello, and the frame decode have one
  implementation. `sdk` must not depend on `os`, so that the wheels can run on Windows
  too (a term of the person). The person decided on 2026-10-15 at 17:40 UTC ("Ok, let's
  do A", relayed by `laptop.monitor`, quoted at
  https://github.com/synnaxlabs/foundation/issues/2177#issuecomment-6198508264), with
  these reservations: "my main reservations for A is the end user DX and the build
  process for different OSes and makking it hard for our users to use our packages.
  But B means we need to constantly sync protocols and we might hit a performacne
  cost". Terms: the gate PR of C6 (PR 1 of its list, before any other SDK code) builds
  the wheels for Linux, macOS, and Windows on x86_64 and arm64, and installs each on a
  clean machine and connects; a failure or pain goes to the person, who can switch to B
  (the protocol again in pure Python) before other SDK code exists. `pyo3`, `maturin`,
  and `numpy` (the Rust crate) are approved. Supersedes C7 + D12 REJECTED
  (`docs/decisions/operations/c7-d12-rejected.md`) for the data path of the Python SDK.
  The base layer generated from the operation table and the native layer (C7) stay.
