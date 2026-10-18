- **CONFIG SCHEMA** One static `document::schema::Body` for each body is the only key
  list of its check, and `foundation docs` renders the same bodies, so the reference
  cannot state less than the check takes. `read::unknown(config, of, body)` refuses each
  attribute and block that `body` does not state. A `schema::Block` has one body for
  each form: `channel` has the forms `index` and `data`, and each gives `unknown` its
  own body. `config::SCHEMA` is a const of the seven core blocks in the keyword order of
  the fix of `config.unknown-block`, and the file level gives `unknown` a body whose
  blocks are `config::SCHEMA`. The `connector` block states `kind` and `node`, and the
  kind states the rest. `connector::reader` gives `SELECT` and `BLOCK`, which the body
  of each kind that reads lists, and `reader::read` takes that body. Each
  `schema::Attribute` has a `Presence`: `Required`; `Unless`, required unless the body
  holds each attribute that it names in its place; `Default`, whose absence gives a
  value; or `Optional`, whose absence gives none, and the body summary states each rule
  that ties it to other attributes. Each body that `foundation docs` states holds
  examples, which pin which attributes it takes and which it needs: the check takes each
  with no diagnostic, still with an attribute removed that it does not need, with a
  `document.missing-attribute` that names one that it needs when that one is removed,
  and with one `document.unknown-attribute` for a name that the body does not state.
  Without the `kind` that picks its form, a block reads as the form whose `kind` has a
  default. No test reads the text of `value` or of a `Default`. The first block of an
  example is of its body, and of a block with more than one form, it holds only
  attributes of its form. The body of a block inside another body has no examples of its
  own: the examples of that body show it. Each kind gets its examples with
  `Kind::SCHEMA` (#3270). Each test also walks each block inside an example, with the
  body of its form: the tests of the core examples in `config`, and those of each kind
  in `node`, over `kinds()`.
  Lost: a hand-written `reference.md` with a test that compares its names, whose values
  and defaults drift with no test; a `fn reference(&self) -> String` on each check,
  which keeps a second key list; a `default: Option<&str>`, whose `None` cannot state
  the influx targets or an absence that gives no value; a `OneOf` group, which cannot
  state "`database`, or `bucket` and `org`".
  Decided by `laptop.architect-2` at 2026-10-18T21:28:29Z
  (https://github.com/synnaxlabs/foundation/issues/3270#issuecomment-6255364169) and
  at 2026-10-18T21:34:00Z
  (https://github.com/synnaxlabs/foundation/issues/3270#issuecomment-6255418341).
