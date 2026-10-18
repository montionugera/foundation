- **SECTOR VERSIONS (#3212)** The last byte of the tag of a file of `sector` is the
  version of its format, one ASCII digit or letter, and the bytes before it are the
  tag's prefix. A new format takes another version, so no tag is the prefix of another.
  `sector::read` gives `Held::Version(found)` for a file of any length that holds the
  prefix and another byte after it. A file shorter than the tag, with another prefix,
  or with this version's tag and another length or checksum, is `Held::Foreign`.
  `sector::local` gives `Error::Version { file, found, expected }` for `address` or
  `admin.key` of another version, with the text `<file> holds format version <found>,
  and this program reads version <expected>`, each byte as an escaped ASCII char, and
  not `NotRunning`. The callers in `node` (`budget`, `identity`, `name`, `admin`) map
  `Version` as `Foreign` until #3225 (`laptop.architect-2`, 2026-10-18T16:51:27Z,
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252530569, item 7
  of https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252508020).
  Lost: a version in decimal digits after the prefix with a checksum over the whole
  file, because the tag `foundation/admin/1` is then the prefix of
  `foundation/admin/12`.
