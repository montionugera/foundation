- **CONFIG SCHEMA** One static `document::schema::Body` for each body is the only key
  list of its check, and `foundation docs` renders the same bodies, so the reference
  cannot state less than the check takes. `read::unknown(config, of, body)` refuses each
  attribute and block that `body` does not state. A `schema::Block` has one body for
  each form: `channel` has the forms `index` and `data`, and each gives `unknown` its
  own body. `config::SCHEMA` is a const of the seven core blocks in the order of their
  keywords, as in `KINDS`, and the file level gives `unknown` a body whose blocks are
  `config::SCHEMA`. The `connector` block states `kind` and `node`, and the kind states
  the rest. `connector::reader` gives `SELECT` and `BLOCK`, which the body of each kind
  that reads lists, and `reader::read` takes that body. Each `schema::Attribute` has a
  `Presence`: `Required`; `Unless`, required unless the body holds each attribute that
  it names in its place; `Default`, whose absence gives a value; or `Optional`, whose
  absence gives none, and the body summary states each rule that ties it to other
  attributes. `Presence::needed` states when a body needs it. Each form of
  `config::SCHEMA` holds examples, which pin which attributes it takes and which it
  needs: the check takes each with no diagnostic, still with an attribute removed that
  it does not need, with a `document.missing-attribute` that names one that it needs
  when that one is removed, still with each attribute with a default removed at once,
  and with one `document.unknown-attribute` for a name that the body does not state.
  Each name of an `Unless` is an attribute of its body. Without the `kind` that picks
  its form, a block reads as the form whose `kind` has a default. No test reads the text
  of `value` or of a `Default`. The first block of an example is of its body, and of a
  block with more than one form, it holds only attributes of its form. Each attribute
  and block of a body is in at least one, or, for the body of a block inside another
  body, in an example of that outer body. Each kind gets its examples with
  `Kind::SCHEMA` (#3270). Each test also walks each block inside an example, with the
  body of its form: the tests of the core examples in `config`, and those of each kind
  in `node`, over `kinds()`.
  Lost: a hand-written `reference.md` with a test that compares its names, whose values
  and defaults drift with no test; a `fn reference(&self) -> String` on each check,
  which keeps a second key list; a `default: Option<&str>`, whose `None` cannot state
  the influx targets or an absence that gives no value; a `OneOf` group, which cannot
  state "`database`, or `bucket` and `org`"; a list of alternative sets on `Body`, whose
  removal test is not exact for `placement`, which refuses with its own code, and which
  gives each body one more field.
  Decided by `laptop.architect-2` at 2026-10-18T21:28:29Z
  (https://github.com/synnaxlabs/foundation/issues/3270#issuecomment-6255364169) and at
  2026-10-18T21:34:00Z
  (https://github.com/synnaxlabs/foundation/issues/3270#issuecomment-6255418341), and on
  the tests at 2026-10-18T22:19:34Z
  (https://github.com/synnaxlabs/foundation/pull/3277#issuecomment-6255858695). The OK
  of `Presence` and of the doc of `examples` at 2026-10-18T22:28:02Z
  (https://github.com/synnaxlabs/foundation/pull/3277#issuecomment-6255939421)
  supersedes "`Attribute` stays as written" of the first ruling.
