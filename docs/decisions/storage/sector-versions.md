- **SECTOR VERSIONS (#3212)** The last byte of the tag of a file of `sector` is the
  version of its format, one byte of lowercase ASCII, `1` to `9`, then `a` to `z`, so
  35 versions, and the bytes before it are the tag's prefix. Version 1 is `1`. A new
  format takes another version, so no tag is the prefix of another. `sector::read`
  gives `Held::Version { found, expected }` for a file of any length that holds the
  prefix and another version of the set after it. A file shorter than the tag, with
  another prefix, with a byte outside the set after it, or with this version's tag
  and another length or checksum, is `Held::Foreign`.
  `sector::local::read` gives `local::Error::Version { file, found, expected }` for
  `address` or `admin.key` of another version, with the text `<file> holds format
  version <found>, and this program reads version <expected>`, each byte as an escaped
  ASCII char, and not `NotRunning`. The callers in `node` (`budget`, `identity`,
  `name`, `admin`) map `Version` as `Foreign` until #3225 (`laptop.architect-2`:
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252530569 at
  2026-10-18T16:51:27Z; item 7 of
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252508020 at
  2026-10-18T16:49:11Z;
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252630274 at
  2026-10-18T17:01:02Z;
  https://github.com/synnaxlabs/foundation/pull/3231#issuecomment-6252780856 at
  2026-10-18T17:15:38Z; the set of versions and `sector::local` in
  https://github.com/synnaxlabs/foundation/pull/3231#issuecomment-6252975559 at
  2026-10-18T17:34:06Z; the names `found` and `expected`, over `held` and `read` of
  6252630274, in item 1 of
  https://github.com/synnaxlabs/foundation/pull/3231#issuecomment-6253217340 at
  2026-10-18T17:57:52Z).
  Lost: a version in the decimal digits after the prefix. A key byte follows each tag
  of version 1 and is an ASCII digit 10 times in 256, so a valid file of version 1
  reads as another version, and `foundation/admin/1` is the prefix of
  `foundation/admin/12`. Lost: a version that counts only when the file ends in the
  CRC32C of the rest. Only a node of another version writes the prefix with another
  version byte, and that node tells a damaged file, so the read stops at the tag.
