- **LINUX CI (2026-10-05)** For the alpha, tests run only on Linux (x86-64 and ARM).
  No CI job runs on macOS or Windows. The design stays cross-OS: each C9d target must
  still be a valid build, so OS-specific code goes only in `os` and, outside the
  binary, in `sdk` (`laptop.architect`, 2026-10-16T21:33:33Z,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225122027;
  `laptop.monitor`, 2026-10-16, under the person's term 2 of C6 (the client does not
  depend on `os`), relayed to the person with no objection,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225611780). The
  person said: "As long as our systems are designed to cross compile i'm ok wiht only
  testing against linux for an alpha. as long as the system is designed for cross os
  deployment" (#574).
