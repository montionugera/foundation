- **CONFIG SCHEMA** One static `document::schema::Body` for each body is the only key
  list of its check, and `foundation docs` renders the same bodies, so the reference
  cannot state less than the check takes. `read::unknown(config, of, body)` refuses
  each attribute and block that `body` does not state. A `schema::Block` has one body
  for each form: `channel` has the forms `index` and `data`, and each gives `unknown`
  its own body. `config::SCHEMA` is a const of the seven core blocks in the order of
  the checks, and the file level gives `unknown` a body whose blocks are
  `config::SCHEMA`. The `connector` block states `kind` and `node`, and the kind
  states the rest. `connector::reader` gives `SELECT` and `READER`, which the body of
  each kind that reads lists, and `reader::read` takes that body. Each body holds
  examples, so the text of `value` and `default` cannot drift: the check takes each
  with no diagnostic, still with an attribute that has a `default` removed, with
  `document.missing-attribute` when a required attribute is removed, and with one
  `document.unknown-attribute` for a name that the body does not state. The first
  block of an example is of its body, and the body of a block inside another body has
  no examples. The tests of the core examples run in `config`, and those of each kind
  in `node`, over `kinds()`. Lost: a hand-written `reference.md` with a test that
  compares its names, whose values and defaults drift with no test; a
  `fn reference(&self) -> String` on each check, which keeps a second key list.
  Decided by `laptop.architect-2` at 2026-10-18T21:28:29Z
  (https://github.com/synnaxlabs/foundation/issues/3270#issuecomment-6255364169) and
  at 2026-10-18T21:34:00Z
  (https://github.com/synnaxlabs/foundation/issues/3270#issuecomment-6255418341).
