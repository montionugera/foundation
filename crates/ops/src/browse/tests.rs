use std::path::Path;

use document::diagnostic::Diagnostic;
use env::paths::Paths;
use serde_json::{Value, json};
use types::name::Name;
use types::sample;

use super::Output;
use crate::common::{self, create_browsing_node, name, solo};
use crate::error::{Error, Sources};
use crate::serve::serve;
use crate::{Failure, Node, Run};

/// The text of a file with the connector `plant` of `kind` on `node`, and `more`
/// lines in its block.
fn site(kind: &str, node: &str, more: &str) -> String {
    let head = format!("connector \"plant\" {{\n  kind    = \"{kind}\"\n");
    format!("{head}  node    = \"{node}\"\n{more}}}\n")
}

/// The body of a `browse` request of `connector` in the file `path` of `text`, with
/// each field of `query`.
fn body(connector: &str, path: &str, text: &str, query: &Value) -> Vec<u8> {
    let mut browse = json!({
        "connector": connector,
        "files": [{ "path": path, "text": text }],
    });
    for (key, value) in query.as_object().expect("an object") {
        browse[key] = value.clone();
    }
    format!("{}\n", json!({ "browse": browse })).into_bytes()
}

/// The reply of `serve` to `body` as `subject`, as JSON. It asserts that the reply is
/// one line.
async fn served(node: &Node, subject: &str, body: &[u8]) -> Value {
    let reply = serve(node, &name(subject), body).await;
    let (line, rest) = reply.split_at(reply.len() - 1);
    assert_eq!(rest, b"\n");
    assert!(!line.contains(&b'\n'));
    serde_json::from_slice(line).expect("JSON")
}

/// The output of a browse of `plant` of `kind` on `edge` as `@admin`, with `query`.
async fn browsed(node: &Node, kind: &str, query: &Value) -> Value {
    let site = site(kind, "edge", "");
    let reply = served(node, "@admin", &body("plant", "site.hcl", &site, query)).await;
    let output = reply["output"].clone();
    assert!(!output.is_null(), "{reply}");
    output
}

/// The reply of one error with no place.
fn refusal(code: &str, status: u8, message: &str, fix: &str) -> Value {
    json!({
        "errors": [{ "code": code, "message": message, "fix": fix, "notes": [] }],
        "status": status,
    })
}

/// The message of a failed browse of `plant` of `kind`.
fn failed(kind: &str, message: &str) -> String {
    format!("the browse of the connector `plant` of kind `{kind}` failed: {message}")
}

/// A point of `facts` at `path` with the tag `tag`.
fn point(path: &str, tag: &str, facts: &Value) -> Value {
    let mut point = json!({ "path": path, "tag": tag, "shape": "point" });
    for (key, value) in facts.as_object().expect("an object") {
        point[key] = value.clone();
    }
    point
}

/// The facts of a point that the device knows nothing of.
fn unknown() -> Value {
    json!({ "type": null, "unit": null, "unit_code": null, "access": null })
}

/// The text of `output`, after it parses back to an [`Output`].
fn text(output: &Value) -> String {
    let output: Output = serde_json::from_value(output.clone()).expect("an output");
    output.text()
}

/// The paths of a host whose working directory holds `site.hcl` with the connector
/// `plant` of `kind` on `edge`.
fn host(fixture: &common::Fixture, kind: &str) -> Paths {
    fixture.sim.make_dir(Path::new("/work"));
    fixture.sim.set_working_dir(Path::new("/work"));
    let paths = fixture.sim.paths();
    let site = site(kind, "edge", "");
    paths
        .write(Path::new("site.hcl"), site.as_bytes())
        .expect("a write");
    paths
}

/// The run, output, and errors of `foundation <args>` on `node`, which serves each
/// request as `@admin`.
async fn cli(node: &Node, paths: &Paths, args: &[&str]) -> (Run, String, String) {
    let reach = async |_: &Path| {
        Ok::<_, Failure>(async |body: &[u8]| {
            Ok::<_, Failure>(serve(node, &name("@admin"), body).await)
        })
    };
    common::cli(paths, args, reach).await
}

/// Each check fails in its order: each request fails each later check too.
#[test]
fn checks_the_files_the_name_access_the_node_the_start_and_the_match_in_order() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let query = json!({ "start": "a//b", "match": "a\\", "cursor": "0" });
        let other = site("tree", "other", "");
        let reply =
            served(&node, "eve", &body("x y", "site.yaml", &other, &query)).await;
        let expected = json!({
            "errors": [{
                "code": "ops.unknown-extension",
                "message": "no config syntax reads this file",
                "fix": "Use a file that ends in `.hcl`",
                "place": { "file": "site.yaml", "line": 1, "column": 1 },
                "notes": [],
            }],
            "status": 2,
        });
        assert_eq!(reply, expected);

        let reply =
            served(&node, "eve", &body("x y", "site.hcl", &other, &query)).await;
        let cause = "x y".parse::<Name>().expect_err("no name");
        let message = format!("the connector `x y` is not a name: {cause}");
        let fix = "Match the arguments to `foundation --help` or `foundation \
                   <operation> --help`";
        assert_eq!(reply, refusal("ops.argument", 2, &message, fix));

        let reply =
            served(&node, "eve", &body("gone", "site.hcl", &other, &query)).await;
        let message = "no `connector` block of the files has the name \"gone\"";
        let fix = "Use one of [\"plant\"]";
        assert_eq!(reply, refusal("config.no-connector", 2, message, fix));

        let reply =
            served(&node, "eve", &body("plant", "site.hcl", &other, &query)).await;
        let message = "no access policy gives `eve` the action `apply` on `plant`";
        let fix = "Ask an admin of the region for an access policy that allows it";
        assert_eq!(reply, refusal("ops.denied", 1, message, fix));

        let at = |text: &str, query: &Value| body("plant", "site.hcl", text, query);
        let reply = served(&node, "@admin", &at(&other, &query)).await;
        let message = "the connector `plant` runs on the node `other`, not on this \
                       node `edge`";
        let fix = "Run it with `--data` of node `other`";
        assert_eq!(reply, refusal("ops.browse-node", 2, message, fix));

        let edge = site("tree", "edge", "");
        let reply = served(&node, "@admin", &at(&edge, &query)).await;
        let message = "the start `a//b` is not a path: a name in it is empty";
        let fix = "Give folder names joined by `/`, with a `\\` before each `/`, `*`, \
                   or `\\` in a name";
        assert_eq!(reply, refusal("ops.bad-start", 2, message, fix));

        let query = json!({ "start": "Line3", "match": "a\\", "cursor": "0" });
        let reply = served(&node, "@admin", &at(&edge, &query)).await;
        let message = "the match `a\\` is not a pattern: it ends in a `\\` that takes \
                       no character";
        let fix = "Give names joined by `/`, at most 64 other than `**` and at most \
                   256 `*`, with a `\\` before each `/`, `*`, or `\\` in a name";
        assert_eq!(reply, refusal("ops.bad-pattern", 2, message, fix));

        let query = json!({ "start": "Line3", "match": "*", "cursor": "0" });
        let reply = served(&node, "@admin", &at(&edge, &query)).await;
        let message = "a browse takes `match` or `cursor`, not both";
        assert_eq!(reply, refusal("ops.argument", 2, message, fix_of(&reply)));

        let query = json!({ "start": "Line3", "cursor": "4" });
        let reply = served(&node, "@admin", &at(&edge, &query)).await;
        assert_eq!(reply["output"]["end"], "done", "{reply}");
    });
}

/// The fix of the one problem of `reply`.
fn fix_of(reply: &Value) -> &str {
    reply["errors"][0]["fix"].as_str().expect("a fix")
}

#[test]
fn a_start_or_a_match_that_is_too_deep_or_has_too_many_stars_gives_its_cause() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let edge = site("tree", "edge", "");
        let deep = vec!["a"; 65].join("/");
        let stars = "*a".repeat(257);
        let cases = [
            (
                json!({ "start": "a\\" }),
                "ops.bad-start",
                "the start `a\\` is not a path: it ends in a `\\` that takes no \
                 character"
                    .to_owned(),
            ),
            (
                json!({ "match": deep }),
                "ops.bad-pattern",
                format!(
                    "the match `{deep}` is not a pattern: it has more than 64 names, \
                     and a browse finds no path that long"
                ),
            ),
            (
                json!({ "match": stars }),
                "ops.bad-pattern",
                format!(
                    "the match `{stars}` is not a pattern: it has more than 256 `*`, \
                     and each costs a search of each name"
                ),
            ),
        ];
        for (query, code, message) in cases {
            let reply =
                served(&node, "@admin", &body("plant", "site.hcl", &edge, &query))
                    .await;
            assert_eq!(reply["errors"][0]["code"], code, "{reply}");
            assert_eq!(reply["errors"][0]["message"], message);
        }
    });
}

/// The broken kind gives a page over the most, which the walk refuses.
#[test]
fn the_command_line_writes_the_defect_of_a_kind_that_breaks_a_rule_of_a_browse() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let paths = host(&fixture, "broken");
        let shown = cli(
            &node,
            &paths,
            &["browse", "plant", "site.hcl", "--start", "Line3"],
        )
        .await;
        let errors = "error[connector.broken-browse]: the browse of the connector \
                      `plant` of kind `broken` failed: the connector kind broke a rule \
                      of a browse: a page holds at most 500 entries, and a page of \
                      \"Line3\" held 501\n\
                      fix: Report this defect of the connector kind with this \
                      message\n";
        assert_eq!(shown, (Run::Exit(1), String::new(), errors.to_owned()));
    });
}

#[test]
fn a_failure_of_the_device_gives_browse_with_its_cause() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let edge = site("failing", "edge", "");
        let reply = served(
            &node,
            "@admin",
            &body("plant", "site.hcl", &edge, &json!({})),
        )
        .await;
        let message = failed(
            "failing",
            "the device is in a bad state: the server closed the session",
        );
        let fix = "Fix the cause in the message, then browse again";
        assert_eq!(reply, refusal("ops.browse", 1, &message, fix));
    });
}

#[test]
fn a_retry_gives_browse_with_its_cause() {
    let cause = connector::kind::Error::Retry("the request timed out".into());
    let error =
        Error::browse(&name("plant"), &name("tree"), cause, &Sources::default());
    let message = failed("tree", "an attempt failed: the request timed out");
    let mut expected = refusal(
        "ops.browse",
        1,
        &message,
        "Fix the cause in the message, then browse again",
    );
    expected
        .as_object_mut()
        .expect("an object")
        .remove("status");
    assert_eq!((error.json(), error.status()), (expected, 1));
    assert_eq!(error.to_string(), message);
}

/// The text of an error of many problems counts them, with the connector and the kind.
#[test]
fn an_error_of_many_problems_counts_them() {
    let problem = |message: &str| {
        Diagnostic::new(common::ADDRESS, None, message.to_owned(), "Fix".to_owned())
    };
    let cause = connector::kind::Error::Config(vec![problem("a"), problem("b")]);
    let error =
        Error::browse(&name("plant"), &name("tree"), cause, &Sources::default());
    assert_eq!(
        error.to_string(),
        "the browse of the connector `plant` of kind `tree` failed with 2 problems"
    );
}

#[test]
fn refuses_bytes_after_the_first_line() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let site = site("tree", "edge", "");
        let mut request = body("plant", "site.hcl", &site, &json!({}));
        request.extend_from_slice(b"more");
        let reply = served(&node, "@admin", &request).await;
        let message = "the body is no request: bytes follow the first line of `browse`";
        assert_eq!(reply["errors"][0]["code"], "ops.bad-request", "{reply}");
        assert_eq!(reply["errors"][0]["message"], message, "{reply}");
    });
}

/// A problem of the config of the kind keeps its code, fix, place, and notes.
#[test]
fn a_problem_of_the_config_keeps_its_code_fix_place_and_notes() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let text = site("tree", "edge", "  address = \"bad\"\n");
        let reply = served(
            &node,
            "@admin",
            &body("plant", "site.hcl", &text, &json!({})),
        )
        .await;
        let expected = json!({
            "errors": [{
                "code": common::ADDRESS.as_str(),
                "message": failed("tree", "the address \"bad\" names no device"),
                "fix": "Give the address of a device",
                "place": { "file": "site.hcl", "line": 4, "column": 13 },
                "notes": [{
                    "text": "the key of the address",
                    "place": { "file": "site.hcl", "line": 4, "column": 3 },
                }],
            }],
            "status": 1,
        });
        assert_eq!(reply, expected);
    });
}

/// A level gives a page with each shape and a cursor, and the cursor gives the next
/// page. A fact that the device cannot know is `null`, and `?` in the text.
#[test]
fn a_level_gives_each_shape_and_its_next_page_at_the_cursor() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let first = browsed(&node, "tree", &json!({ "start": "Line3" })).await;
        let expected = json!({
            "entries": [
                { "path": "Line3/Cell", "tag": "line3.cell", "shape": "folder" },
                point("Line3/Temperature", "line3.temperature", &json!({
                    "type": "f64", "unit": "°C", "unit_code": "CEL", "access": "read",
                })),
                point("Line3/Bare", "line3.bare", &unknown()),
                point("Line3/Heater", "line3.heater", &json!({
                    "type": "u16", "unit": null, "unit_code": "KWT", "access": "write",
                })),
            ],
            "end": { "next": "4" },
        });
        assert_eq!(first, expected);
        let lines = "Line3/Cell/\n\
                     Line3/Temperature f64 °C read\n\
                     Line3/Bare ? ? ?\n\
                     Line3/Heater u16 KWT write\n\
                     more: --cursor 4\n";
        assert_eq!(text(&first), lines);

        let query = json!({ "start": "Line3", "cursor": "4" });
        let next = browsed(&node, "tree", &query).await;
        let expected = json!({
            "entries": [
                {
                    "path": "Line3/Recipe",
                    "tag": "line3.recipe",
                    "shape": "unreadable",
                    "reason": "an array",
                },
                point("Line3/Setpoint", "line3.setpoint", &json!({
                    "type": "f32",
                    "unit": "%",
                    "unit_code": "P1",
                    "access": "read-write",
                })),
            ],
            "end": "done",
        });
        assert_eq!(next, expected);
        let lines =
            "Line3/Recipe unreadable: an array\nLine3/Setpoint f32 % read-write\n";
        assert_eq!(text(&next), lines);

        let root = browsed(&node, "tree", &json!({})).await;
        let paths: Vec<_> = root["entries"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|e| e["path"].clone())
            .collect();
        assert_eq!(paths, ["Line3", "Types", "Many", "Huge"]);
        assert_eq!(root["end"], "done");
    });
}

/// With values, each point has a `value`, `null` when the device gave none, and a
/// folder or an unreadable point has none.
#[test]
fn values_are_only_on_points_and_null_when_the_device_gave_none() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let query = json!({ "start": "Line3", "values": true });
        let first = browsed(&node, "tree", &query).await;
        let entries = first["entries"].as_array().expect("a list");
        assert_eq!(entries[0].get("value"), None);
        assert_eq!(entries[1]["value"], 21.5);
        let mut bare = unknown();
        bare["value"] = Value::Null;
        assert_eq!(entries[2], point("Line3/Bare", "line3.bare", &bare));
        assert_eq!(entries[3]["value"], 3);
        let lines = "Line3/Cell/\n\
                     Line3/Temperature f64 °C read 21.5\n\
                     Line3/Bare ? ? ? ?\n\
                     Line3/Heater u16 KWT write 3\n\
                     more: --cursor 4\n";
        assert_eq!(text(&first), lines);
        let query = json!({ "start": "Line3", "cursor": "4", "values": true });
        let next = browsed(&node, "tree", &query).await;
        assert_eq!(next["entries"][0].get("value"), None);
        assert_eq!(next["entries"][1]["value"], 42.0);
    });
}

/// A match with values gives each value type in its JSON form.
#[test]
fn a_match_with_values_gives_each_value_type() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let query = json!({ "start": "Types", "match": "*", "values": true });
        let found = browsed(&node, "tree", &query).await;
        let values: Vec<_> = found["entries"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|entry| (entry["type"].clone(), entry["value"].clone()))
            .collect();
        let expected = [
            (json!("bool"), json!(true)),
            (json!("i8"), json!(-8)),
            (json!("i16"), json!(-16)),
            (json!("i32"), json!(-32)),
            (json!("i64"), json!(i64::MIN.to_string())),
            (json!("u8"), json!(8)),
            (json!("u16"), json!(16)),
            (json!("u32"), json!(32)),
            (json!("u64"), json!(u64::MAX.to_string())),
            (json!("f32"), json!(0.1)),
            (json!("f64"), json!(-2.5)),
            (json!("f64"), json!("NaN")),
            (json!("f32"), json!("Infinity")),
            (json!("f64"), json!("-Infinity")),
            (json!("timestamp"), json!("2023-11-14T22:13:20.000000001Z")),
            (json!("duration"), json!("250us")),
            (json!("uuid"), json!("01234567-89ab-cdef-0123-456789abcdef")),
            (Value::Null, Value::Null),
        ];
        assert_eq!(values, expected);
        assert_eq!(found["end"], "done");
        let shown = text(&found);
        let lines: Vec<_> = shown.lines().collect();
        assert_eq!(lines[0], "Types/bool bool ? read true");
        assert_eq!(lines[9], "Types/f32 f32 ? read 0.1");
        assert_eq!(lines[11], "Types/nan f64 ? read NaN");
        assert_eq!(
            lines[16],
            "Types/uuid uuid ? read 01234567-89ab-cdef-0123-456789abcdef"
        );
        assert_eq!(lines[17], "Types/none ? ? ? ?");
        assert_eq!(lines.len(), 18);
    });
}

/// An integer of 64 bits is its decimal text, also past 2^53, where a JSON number loses
/// digits in JavaScript, and a float that is not finite is the text of proto3 JSON.
#[test]
fn a_value_is_json_as_proto3_gives_it() {
    let cases = [
        (
            sample::Value::U64(9_007_199_254_740_993),
            json!("9007199254740993"),
        ),
        (
            sample::Value::I64(-9_007_199_254_740_993),
            json!("-9007199254740993"),
        ),
        (sample::Value::U32(u32::MAX), json!(4_294_967_295_u32)),
        (sample::Value::I32(i32::MIN), json!(-2_147_483_648_i32)),
        (sample::Value::F32(f32::NAN), json!("NaN")),
        (sample::Value::F32(f32::INFINITY), json!("Infinity")),
        (sample::Value::F32(f32::NEG_INFINITY), json!("-Infinity")),
        (sample::Value::F64(f64::NAN), json!("NaN")),
        (sample::Value::F64(f64::INFINITY), json!("Infinity")),
        (sample::Value::F64(f64::NEG_INFINITY), json!("-Infinity")),
    ];
    for (value, expected) in cases {
        let shown = format!("{value:?}");
        let json = serde_json::to_value(super::Value::of(value)).expect("JSON");
        assert_eq!(json, expected, "{shown}");
    }
}

/// A match of more points than the most gives `limit`, and a match that looks at more
/// entries than the budget gives `budget`.
#[test]
fn a_match_ends_at_the_limit_or_at_the_budget() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let found =
            browsed(&node, "tree", &json!({ "start": "Many", "match": "*" })).await;
        assert_eq!(found["entries"].as_array().expect("a list").len(), 500);
        assert_eq!(found["end"], "limit");
        let shown = text(&found);
        assert_eq!(
            shown.lines().last(),
            Some(
                "More than 500 points match, the most for one browse; narrow the \
                 pattern or the start"
            )
        );

        let query = json!({ "start": "Huge", "match": "none" });
        let found = browsed(&node, "tree", &query).await;
        assert_eq!(found, json!({ "entries": [], "end": "budget" }));
        let lines = "The browse found no entries.\n\
                     The browse stopped at its budget, so more may match; narrow the \
                     pattern or the start\n";
        assert_eq!(text(&found), lines);
    });
}

/// The command line writes the text of a page, and with `--json` its JSON.
#[test]
fn the_command_line_writes_a_page_as_text_or_json() {
    solo(|fixture, mesh| async move {
        let node = create_browsing_node(&fixture, mesh);
        let paths = host(&fixture, "tree");
        let args = [
            "browse", "plant", "site.hcl", "--start", "Line3", "--cursor", "4",
        ];
        let shown = cli(&node, &paths, &args).await;
        let lines =
            "Line3/Recipe unreadable: an array\nLine3/Setpoint f32 % read-write\n";
        assert_eq!(shown, (Run::Exit(0), lines.to_owned(), String::new()));
        let (run, json, errors) =
            cli(&node, &paths, &[&args[..], &["--json"]].concat()).await;
        assert_eq!((run, errors.as_str()), (Run::Exit(0), ""));
        let query = json!({ "start": "Line3", "cursor": "4" });
        assert_eq!(json, format!("{}\n", browsed(&node, "tree", &query).await));
        let args = [
            "browse", "plant", "site.hcl", "--match", "*", "--cursor", "4",
        ];
        let (run, output, errors) = cli(&node, &paths, &args).await;
        assert_eq!((run, output.as_str()), (Run::Exit(2), ""));
        assert!(errors.starts_with("error[ops.argument]: "), "{errors}");
    });
}

#[test]
fn the_text_of_no_entries_says_so() {
    let output = Output {
        entries: Vec::new(),
        end: super::End::Done,
    };
    assert_eq!(output.text(), "The browse found no entries.\n");
}
