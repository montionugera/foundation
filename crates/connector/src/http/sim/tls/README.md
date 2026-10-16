# Test certificates

The TLS mode of the simulated HTTP server (`serve_tls`) uses these files. They are
test files: no node trusts them outside a test.

Make them again with OpenSSL 3, in this folder:

```sh
sh make.sh
```

The script deletes the two CA keys, so a new run makes all the files again.

| File | Host | Signed by | Valid (UTC) | `Certificate` |
| --- | --- | --- | --- | --- |
| `ca.pem` | none, a CA | itself | 2025-01-01 to 2036-01-01 | the roots of `ca()` |
| `other-ca.pem` | none, a CA | itself | 2025-01-01 to 2036-01-01 | none |
| `influx.pem`, `.key` | `influx` | `ca` | 2026-01-01 to 2027-01-01 | `Valid` |
| `other.pem`, `.key` | `other` | `ca` | 2026-01-01 to 2027-01-01 | `OtherHost` |
| `expired.pem`, `.key` | `influx` | `ca` | 2026-01-01 to 2026-02-01 | `Expired` |
| `stranger.pem`, `.key` | `influx` | `other-ca` | 2026-01-01 to 2027-01-01 | `OtherCa` |

Each test sets the mesh time of its client node, so no test depends on the date it
runs.

Each key is Ed25519. TLS draws its own randomness (TLS RANDOMNESS), so the bytes of a
handshake change from run to run, but their sizes must not: the digest of a simulation
holds the length of each packet. An Ed25519 signature always has 64 bytes. An ECDSA
signature in DER changes its size by a byte or two from run to run.
