- **SECTOR VERSIONS (#3212)** The last byte of the tag of a file of `sector` is the
  version of its format, one byte, `1` to `9`, then `a` to `z`, and the bytes before it
  are the tag's prefix. A new format takes another version, so no tag is the prefix of
  another. `sector::read` gives `Held::Version(found)` for a file of any length that
  holds the prefix and another byte after it. A file shorter than the tag, with
  another prefix, or with this version's tag and another length or checksum, is
  `Held::Foreign`. `sector::local` gives `Error::Version { file, found, expected }` for
  `address` or `admin.key` of another version, with the text `<file> holds format
  version <found>, and this program reads version <expected>`, each byte as an escaped
  ASCII char, and not `NotRunning`. The callers in `node` (`budget`, `identity`,
  `name`, `admin`) map `Version` as `Foreign` until #3225 (`laptop.architect-2`,
  2026-10-18T16:51:27Z,
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252530569, item 7
  of https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252508020,
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252630274, and
  https://github.com/synnaxlabs/foundation/pull/3231#issuecomment-6252780856).
  Lost: a version in the decimal digits after the prefix. A key byte follows each tag
  of version 1 and is an ASCII digit 10 times in 256, so a valid file of version 1
  reads as another version, and `foundation/admin/1` is the prefix of
  `foundation/admin/12`. Lost: a version that counts only when the file ends in the
  CRC32C of the rest. Only a node of another version writes the prefix with another
  version byte, and that node tells a damaged file, so the read stops at the tag.
