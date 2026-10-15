- **SDK DATA PATH (reverses D12 for Python)** The Python SDK wraps the Rust SDK `sdk`
  with PyO3, so the wire, the TLS pin, the hello, and the frame decode have one
  implementation. `sdk` does not depend on `os`, so the wheels run on Windows too. The
  person decided on 2026-10-15 at 17:40 UTC ("Ok, let's do A", relayed by
  `laptop.monitor`, quoted at
  https://github.com/synnaxlabs/foundation/issues/2177#issuecomment-6198508264), with
  these reservations: "my main reservations for A is the end user DX and the build
  process for different OSes and makking it hard for our users to use our packages.
  But B means we need to constantly sync protocols and we might hit a performacne
  cost". Terms: the first PR of C6 builds the wheels for Linux, macOS, and Windows on
  x86_64 and arm64, and installs each on a clean machine and connects; a failure or
  pain goes to the person, who can switch to B (the protocol again in pure Python)
  before other SDK code exists. `pyo3`, `maturin`, and `numpy` (the Rust crate) are
  approved. Supersedes C7 + D12 REJECTED for the data path of the Python SDK. The base
  layer generated from the operation table and the native layer (C7) stay.
