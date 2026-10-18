use serde_json::{Value, json};
use types::name::Name;
use wire::hub::client::BODY_BYTES_MAX;

use super::Output;
use crate::Node;
use crate::common::{create_browsing_node, name, solo};
use crate::serve::serve;

/// The text of a file with the connector `plant` of the kind `tree` on `edge`, and
/// `more` after its block.
fn site(more: &str) -> String {
    let head = "connector \"plant\" {\n  kind    = \"tree\"\n  node    = \"edge\"\n}\n";
    format!("{head}{more}")
}

/// The body of an `import` request of `plant` in the file `site.hcl` of `text`, with
/// each field of `import`.
fn body(text: &str, import: &Value) -> Vec<u8> {
    let mut request = json!({
        "connector": "plant",
        "files": [{ "path": "site.hcl", "text": text }],
    });
    for (key, value) in import.as_object().expect("an object") {
        request[key] = value.clone();
    }
    format!("{}\n", json!({ "import": request })).into_bytes()
}

/// The first line of the reply of `serve` to `body` as `subject`, as JSON, and the
/// text after it.
async fn served(node: &Node, subject: &str, body: &[u8]) -> (Value, String) {
    let reply = serve(node, &name(subject), body).await;
    let end = reply
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("a first line");
    let line = serde_json::from_slice(&reply[..end]).expect("JSON");
    let rest = String::from_utf8(reply[end + 1..].to_vec()).expect("UTF-8");
    (line, rest)
}

/// The first line of the reply to an import of `import` into [`site`] as `@admin`, and
/// the text after it.
async fn imported(node: &Node, import: &Value) -> (Value, String) {
    served(node, "@admin", &body(&site(""), import)).await
}

/// The reply of one error with no place.
fn refusal(code: &str, status: u8, message: &str, fix: &str) -> Value {
    json!({
        "errors": [{ "code": code, "message": message, "fix": fix, "notes": [] }],
        "status": status,
    })
}

/// The points at `paths`, with no given name.
fn points(paths: &[&str]) -> Value {
    paths.iter().map(|path| json!({ "path": path })).collect()
}

/// The checks up to the paths fail in their order: each request fails each later check
/// too.
#[test]
fn checks_access_the_picks_and_the_paths_in_order() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let bad = json!([
            { "path": "a//b" },
            { "path": "x", "name": "a b" },
            { "path": "c\\" },
        ]);
        let all = json!({ "points": bad, "match": "a\\" });
        let (reply, _) = served(&node, "eve", &body(&site(""), &all)).await;
        let message = "no access policy gives `eve` the action `apply` on `plant`";
        let fix = "Ask an admin of the region for an access policy that allows it";
        assert_eq!(reply, refusal("ops.denied", 1, message, fix));

        let (reply, _) = imported(&node, &all).await;
        let message = "an import takes `points` or `match`, not both";
        let fix = "Match the arguments to `foundation --help` or `foundation \
                   <operation> --help`";
        assert_eq!(reply, refusal("ops.argument", 2, message, fix));

        let (reply, _) = imported(&node, &json!({ "points": [] })).await;
        let message = "the import picks no point and gives no match";
        let fix = "Give the points to import, or `--match <pattern>`";
        assert_eq!(reply, refusal("ops.no-pick", 2, message, fix));

        let many = vec![json!({ "path": "a//b" }); 501];
        let (reply, _) = imported(&node, &json!({ "points": many })).await;
        let message = "the import picks 501 points, more than 500, the most for one \
                       import";
        let fix = "Import at most 500 points at a time";
        assert_eq!(reply, refusal("ops.import-limit", 2, message, fix));
        let most = vec![json!({ "path": "a//b" }); 500];
        let (reply, _) = imported(&node, &json!({ "points": most })).await;
        assert_eq!(
            reply["errors"].as_array().map(Vec::len),
            Some(500),
            "{reply}"
        );
        assert_eq!(reply["errors"][0]["code"], "ops.bad-point");

        let (reply, text) = imported(&node, &json!({ "points": bad })).await;
        let problem = |message: &str| {
            json!({
                "code": "ops.bad-point",
                "message": message,
                "fix": "Give the names of the path joined by `/`, with a `\\` before \
                        each `/`, `*`, or `\\` in a name",
                "notes": [],
            })
        };
        let errors = [
            problem("the point `a//b` is not a path: a name in it is empty"),
            problem(
                "the point `c\\` is not a path: it ends in a `\\` that takes no \
                 character",
            ),
        ];
        assert_eq!(reply, json!({ "errors": errors, "status": 2 }));
        assert_eq!(text, "");
    });
}

/// The checks after the paths fail in their order: each request passes each earlier
/// check and fails each later one too.
#[test]
fn checks_the_duplicates_the_names_and_the_match_after_the_paths_in_order() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let twice = json!([
            { "path": "x", "name": "a b" },
            { "path": "x" },
            { "path": "y" },
            { "path": "y" },
            { "path": "y" },
        ]);
        let (reply, _) = imported(&node, &json!({ "points": twice })).await;
        let problem = |path: &str| {
            json!({
                "code": "ops.duplicate-pick",
                "message": format!("the import picks the point `{path}` twice"),
                "fix": "Pick each point once",
                "notes": [],
            })
        };
        let expected = json!({ "errors": [problem("x"), problem("y")], "status": 2 });
        assert_eq!(reply, expected);

        let named = json!([
            { "path": "x", "name": "a b" },
            { "path": "y" },
            { "path": "z", "name": "" },
        ]);
        let (reply, text) = imported(&node, &json!({ "points": named })).await;
        let problem = |path: &str, text: &str| {
            let cause = text.parse::<Name>().expect_err("no name");
            json!({
                "code": "ops.bad-name",
                "message": format!(
                    "the name `{text}` of the point `{path}` is not a name: {cause}"
                ),
                "fix": "Give another name, or leave it out for the default name",
                "notes": [],
            })
        };
        let errors = [problem("x", "a b"), problem("z", "")];
        assert_eq!(reply, json!({ "errors": errors, "status": 2 }));
        assert_eq!(text, "");

        let (reply, _) = imported(&node, &json!({ "match": "a\\" })).await;
        let message = "the match `a\\` is not a pattern: it ends in a `\\` that takes \
                       no character";
        let fix = "Give names joined by `/`, at most 64 other than `**` and at most \
                   256 `*`, with a `\\` before each `/`, `*`, or `\\` in a name";
        assert_eq!(reply, refusal("ops.bad-pattern", 2, message, fix));

        let (reply, _) =
            imported(&node, &json!({ "points": points(&["Line3/Gone"]) })).await;
        assert_eq!(reply["errors"][0]["code"], "connector.no-point", "{reply}");
    });
}

/// A match adds each point that the connector can read or write, with a `fill` for
/// each fact that the device does not give, and counts the rest.
#[test]
fn a_match_adds_each_point_that_the_connector_can_read_or_write() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let (reply, text) =
            imported(&node, &json!({ "start": "Line3", "match": "*" })).await;
        let channel = |name: &str, path: &str, data_type: Value| {
            json!({
                "name": name,
                "path": path,
                "type": data_type,
            })
        };
        let output = json!({
            "file": "site.hcl",
            "channels": [
                channel("plant.Temperature", "Line3/Temperature", json!("f64")),
                channel("plant.Bare", "Line3/Bare", Value::Null),
                channel("plant.Heater", "Line3/Heater", json!("u16")),
                channel("plant.Setpoint", "Line3/Setpoint", json!("f32")),
            ],
            "index": "plant.time",
            "fills": [{ "name": "plant.Bare", "what": ["the sample type"] }],
            "unnamed": [],
            "present": 0,
            "unreadable": 1,
        });
        assert_eq!(reply, json!({ "output": output }));
        let expected = r#"connector "plant" {
  kind    = "tree"
  node    = "edge"

  read "plant.Temperature" {
    tag = "line3.temperature"
  }

  read "plant.Bare" {
    tag = "line3.bare"
  }

  write "plant.Heater" {
    tag = "line3.heater"
  }

  read "plant.Setpoint" {
    tag = "line3.setpoint"
  }
}

channel "plant.time" {
  kind = "index"
}

channel "plant.Temperature" {
  data_type = "f64"
  index = "plant.time"
  unit = "degC"
}

channel "plant.Bare" {
  data_type = fill("the sample type")
  index = "plant.time"
}

channel "plant.Heater" {
  data_type = "u16"
  index = "plant.time"
  unit = "kW"
}

channel "plant.Setpoint" {
  data_type = "f32"
  index = "plant.time"
  unit = "%"
}
"#;
        assert_eq!(text, expected);
    });
}

/// Points take their given names, in the order of the points, and a second import of
/// them adds nothing and gives the text as it was.
#[test]
fn points_take_their_given_names_and_a_second_import_adds_nothing() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let picks = json!({ "points": [
            { "path": "Line3/Heater" },
            { "path": "Line3/Temperature", "name": "temp" },
        ]});
        let (reply, text) = imported(&node, &picks).await;
        let output = json!({
            "file": "site.hcl",
            "channels": [
                { "name": "plant.Line3.Heater", "path": "Line3/Heater", "type": "u16" },
                { "name": "temp", "path": "Line3/Temperature", "type": "f64" },
            ],
            "index": "plant.time",
            "fills": [],
            "unnamed": [],
            "present": 0,
            "unreadable": 0,
        });
        assert_eq!(reply, json!({ "output": output }));
        assert!(
            text.contains("  write \"plant.Line3.Heater\" {\n"),
            "{text}"
        );
        assert!(text.contains("\nchannel \"temp\" {\n"), "{text}");

        let (reply, again) = served(&node, "@admin", &body(&text, &picks)).await;
        let output = json!({
            "file": "site.hcl",
            "channels": [],
            "index": null,
            "fills": [],
            "unnamed": [],
            "present": 2,
            "unreadable": 0,
        });
        assert_eq!(reply, json!({ "output": output }));
        assert_eq!(again, text);
    });
}

/// A picked point that the connector cannot read or write fails the import, with its
/// reason.
#[test]
fn a_picked_point_that_the_connector_cannot_read_gives_its_reason() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let picks = points(&["Line3/Temperature", "Line3/Recipe"]);
        let (reply, text) = imported(&node, &json!({ "points": picks })).await;
        let message = "a connector of the kind cannot read or write the point \
                       `Line3/Recipe`: an array";
        let fix = "Leave the point out of the import";
        assert_eq!(reply, refusal("ops.unreadable-point", 2, message, fix));
        assert_eq!(text, "");
    });
}

/// A match of more points than one import takes, and a walk that stops at its budget,
/// add nothing.
#[test]
fn a_match_past_the_limit_and_a_walk_past_the_budget_add_nothing() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let (reply, _) =
            imported(&node, &json!({ "start": "Many", "match": "*" })).await;
        let message = "more than 500 points match, the most for one import";
        let fix = "Import at most 500 points at a time";
        assert_eq!(reply, refusal("ops.import-limit", 2, message, fix));

        let fix = "Narrow the start, or import fewer points at a time";
        let (reply, _) =
            imported(&node, &json!({ "start": "Huge", "match": "x" })).await;
        let message =
            "the import stopped after 10000 entries, so more points can match";
        assert_eq!(reply, refusal("ops.import-budget", 1, message, fix));

        let picks = points(&["Huge/p10000"]);
        let (reply, _) = imported(&node, &json!({ "points": picks })).await;
        let message =
            "the import stopped after 10000 entries before it found each point";
        assert_eq!(reply, refusal("ops.import-budget", 1, message, fix));
    });
}

#[test]
fn refuses_bytes_after_the_first_line() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let mut request = body(&site(""), &json!({ "match": "*" }));
        request.extend_from_slice(b"more");
        let (reply, _) = served(&node, "@admin", &request).await;
        let message = "the body is no request: bytes follow the first line of `import`";
        assert_eq!(reply["errors"][0]["code"], "ops.bad-request", "{reply}");
        assert_eq!(reply["errors"][0]["message"], message, "{reply}");
    });
}

/// An output and new text that pass the bound of a reply give `ops.large-import`.
#[test]
fn a_reply_past_the_bound_gives_large_import() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let bound = usize::try_from(BODY_BYTES_MAX).expect("a usize");
        let comment = format!("# {}\n", "x".repeat(bound - 200));
        let picks = json!({ "points": points(&["Line3/Temperature"]) });
        let (reply, text) =
            served(&node, "@admin", &body(&site(&comment), &picks)).await;
        let message = format!(
            "the reply of the import has more than {BODY_BYTES_MAX} bytes, the most \
             that a reply holds"
        );
        let fix = "Import fewer points at a time";
        assert_eq!(reply, refusal("ops.large-import", 1, &message, fix));
        assert_eq!(text, "");
    });
}

/// The text gives the file, each channel, the index, each fill, each unnamed point,
/// and each count that is not 0, with each control character escaped.
#[test]
fn the_text_gives_each_part_of_the_output() {
    let output: Output = serde_json::from_value(json!({
        "file": "site\u{7}.hcl",
        "channels": [
            { "name": "plant.a", "path": "A", "type": "f64" },
            { "name": "plant.b", "path": "B", "type": null },
        ],
        "index": "plant.time",
        "fills": [{ "name": "plant.b", "what": ["the sample type", "the unit\n"] }],
        "unnamed": [{ "path": "Long", "fix": "Give it a name" }],
        "present": 1,
        "unreadable": 2,
    }))
    .expect("an output");
    let expected = "The import added 2 channels to site\\u{7}.hcl.
plant.a A f64
plant.b B ?
It added the index channel plant.time.
Write in place of each `fill` of plant.b: the sample type; the unit\\n.
It did not add Long, as its default name is too long. Give it a name.
It did not add 1 point that the file reads or writes already.
It left out 2 points that the connector cannot read or write.
";
    assert_eq!(output.text(), expected);

    let none: Output = serde_json::from_value(json!({
        "file": "site.hcl",
        "channels": [{ "name": "plant.a", "path": "A", "type": "f64" }],
        "index": null,
        "fills": [],
        "unnamed": [],
        "present": 0,
        "unreadable": 1,
    }))
    .expect("an output");
    let expected = "The import added 1 channel to site.hcl.\nplant.a A f64\n\
                    It left out 1 point that the connector cannot read or write.\n";
    assert_eq!(none.text(), expected);
}
