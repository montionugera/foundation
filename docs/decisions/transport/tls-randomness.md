- **TLS RANDOMNESS (2026-10-04)** All randomness inside TLS (key shares, client
  random, nonces) comes from aws-lc, not from `env`. rustls holds its random source
  as a `&'static` value, and aws-lc makes X25519 key shares with its own randomness,
  so neither can be injected without a leak per `Transport`. It changes bytes, never
  sizes or timing. Simulated runs still replay because nothing branches on those
  bytes; replay traces leave out ciphertext and handshake randoms, and a `sim` test
  runs one value twice and compares the traces. The person accepted it ("Accept TLS
  RANDOMNESS"), from `network`'s proposal on #54.
  #2934 extends it to the OPC UA secure channel of `connector-opcua`: the OpenSSL
  plugins of open62541 draw nonces, keys, padding, and salts from aws-lc
  (`RAND_bytes`), not from `env`, with the same bounds: bytes only, and nothing
  branches on them. `AWS_LC` of `cargo xtask open62541` lists each function of aws-lc
  that draws. OK from `laptop.architect-2`
  (https://github.com/synnaxlabs/foundation/issues/2934#issuecomment-6230838999,
  2026-10-17 06:46 UTC). The person: pending
  (https://github.com/synnaxlabs/foundation/issues/2934#issuecomment-6231143169).
