- **C7 + D12 rejected** Each SDK has a generated base layer and a hand-written native
  layer. Each SDK's data path is hand-written as the fastest idiomatic code for its
  language. Guard: the Rust codec is the reference; golden vectors from it run in every
  SDK's CI; differential fuzzing runs both ways; the drift agent watches it.
  The Python SDK has no hand-written data path: it wraps the Rust `sdk` with PyO3, so
  the golden vectors and differential fuzzing have no object in it (the person chose A
  at 2026-10-15T17:40Z,
  https://github.com/synnaxlabs/foundation/issues/2177#issuecomment-6198508264).
