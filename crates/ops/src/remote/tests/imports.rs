use std::path::{Path, PathBuf};

use env::paths::Paths;
use serde_json::{Value, json};

use super::{CHANGED, Mode, Remote, call, read, shown, with_site};
use crate::Run;
use crate::common::{create_browsing_node, solo};
use crate::error::Error;
use crate::operation;
use crate::scope::most;
use crate::serve::Point;

/// The connector `plant` of the kind `tree` on `edge`.
const PLANT: &str = "connector \"plant\" {\n  kind = \"tree\"\n  node = \"edge\"\n}\n";

/// [`PLANT`] after the import of `Line3/Temperature` and of `Line3/Heater` as `heat`.
const IMPORTED: &str = r#"connector "plant" {
  kind = "tree"
  node = "edge"

  read "plant.Temperature" {
    tag = "line3.temperature"
  }

  write "heat" {
    tag = "line3.heater"
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

channel "heat" {
  data_type = "u16"
  index = "plant.time"
  unit = "kW"
}
"#;

/// The paths of a host with `site.hcl` and `plant.hcl` of `plant`.
fn with_plant(host: &sim::node::Node, plant: &str) -> Paths {
    let paths = with_site(host);
    paths
        .write(Path::new("plant.hcl"), plant.as_bytes())
        .expect("a write");
    paths
}

/// The text of `plant.hcl`.
fn plant(paths: &Paths) -> String {
    let Ok(env::paths::Found::File(bytes)) = paths.read(Path::new("plant.hcl"), most())
    else {
        panic!("a file");
    };
    String::from_utf8(bytes).expect("UTF-8")
}

/// The import of `Line3/Temperature` from the start `Line3` into `plant.hcl`.
fn temperature() -> operation::Import {
    operation::Import {
        connector: "plant".to_owned(),
        files: vec![PathBuf::from("plant.hcl")],
        start: Some("Line3".to_owned()),
        points: vec![Point {
            path: "Line3/Temperature".to_owned(),
            name: None,
        }],
        pattern: None,
    }
}

const ARGS: [&str; 9] = [
    "import",
    "plant",
    "plant.hcl",
    "--start",
    "Line3",
    "--point",
    "Line3/Temperature",
    "--name",
    "heat=Line3/Heater",
];

/// The command line writes the new text, with each `--point` and then each `--name`
/// in pick order. A second run adds nothing and leaves the text as it is.
#[test]
fn the_command_line_writes_the_import_and_a_second_run_adds_nothing() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let remote = Remote::new(create_browsing_node(&fixture, mesh));
        let (run, text, errors) = remote.cli(&paths, &ARGS).await;
        assert_eq!((run, errors.as_str()), (Run::Exit(0), ""));
        let expected = "The import added 2 channels to plant.hcl.\n\
                        plant.Temperature Line3/Temperature f64\n\
                        heat Line3/Heater u16\n\
                        It added the index channel plant.time.\n";
        assert_eq!(text, expected);
        assert_eq!(plant(&paths), IMPORTED);
        let (run, text, errors) = remote.cli(&paths, &ARGS).await;
        assert_eq!((run, errors.as_str()), (Run::Exit(0), ""));
        let expected = "The import added 0 channels to plant.hcl.\n\
                        It did not add 2 points that the file or an earlier pick \
                        reads or writes already.\n";
        assert_eq!(text, expected);
        assert_eq!(plant(&paths), IMPORTED);
    });
}

/// A file that changed after the read gives `ops.file-changed`, and the import writes
/// nothing.
#[test]
fn an_import_into_a_file_that_changed_writes_nothing() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let node = create_browsing_node(&fixture, mesh);
        let remote = Remote::with(node, Mode::Changes(paths.clone(), CHANGED.into()));
        let imported = remote.import(&paths, temperature()).await;
        let Err(error) = imported else {
            panic!("a changed file");
        };
        let message = "the file plant.hcl changed after the import read it, so the \
                       import wrote nothing";
        assert_eq!(shown(&error), read("ops.file-changed", message, 1));
        assert_eq!(
            error.problems()[0].fix,
            "Run the import again on the file as it is now"
        );
        assert_eq!(plant(&paths), CHANGED);
    });
}

/// A file that grew past the bytes of one request after the read gives
/// `ops.file-changed` too.
#[test]
fn an_import_into_a_file_that_grew_too_large_gives_file_changed() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let node = create_browsing_node(&fixture, mesh);
        let large = vec![b'#'; most() + 1];
        let remote = Remote::with(node, Mode::Changes(paths.clone(), large));
        let imported = remote.import(&paths, temperature()).await;
        let Err(error) = imported else {
            panic!("a changed file");
        };
        let message = "the file plant.hcl changed after the import read it, so the \
                       import wrote nothing";
        assert_eq!(shown(&error), read("ops.file-changed", message, 1));
    });
}

/// An import whose new text is the text that it sent neither reads the file again nor
/// writes it, so a change after the read stays.
#[test]
fn an_import_that_adds_nothing_does_not_write() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, IMPORTED);
        let node = create_browsing_node(&fixture, mesh);
        let remote = Remote::with(node, Mode::Changes(paths.clone(), CHANGED.into()));
        let output = remote
            .import(&paths, temperature())
            .await
            .expect("an import");
        assert_eq!((output.channels.len(), output.present), (0, 1));
        assert_eq!(plant(&paths), CHANGED);
    });
}

/// A reply with no text, with text that is not UTF-8, or that names a file that the
/// request did not send gives `ops.bad-reply`, and writes nothing.
#[test]
fn an_import_reply_with_no_text_or_another_file_gives_bad_reply() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let output = |file: &str| {
            let output = json!({
                "file": file,
                "channels": [],
                "index": null,
                "fills": [],
                "unnamed": [],
                "present": 0,
                "unreadable": 0,
            });
            format!("{}\n", json!({ "output": output })).into_bytes()
        };
        let mut not_utf8 = output("plant.hcl");
        not_utf8.push(0xff);
        let mut other = output("other.hcl");
        other.extend_from_slice(b"x");
        let cases = [
            (
                output("plant.hcl"),
                "no text follows the first line of the reply of `import`",
            ),
            (not_utf8, "the text of the reply of `import` is not UTF-8"),
            (
                other,
                "the reply of `import` names a file that the request did not send",
            ),
        ];
        let mut remote = Remote::new(create_browsing_node(&fixture, mesh));
        for (reply, message) in cases {
            remote = Remote::with(remote.node, Mode::Reply(reply));
            let imported = remote.import(&paths, temperature()).await;
            let message = message.to_owned();
            assert_eq!(imported.err(), Some(Error::Reply { message }));
            assert_eq!(plant(&paths), PLANT);
        }
    });
}

/// A `--name` with no `=`, and `--match` with `--point`, are bad arguments that reach
/// no node.
#[test]
fn the_command_line_refuses_a_name_with_no_split_and_a_match_with_points() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let remote = Remote::new(create_browsing_node(&fixture, mesh));
        let cases = [
            (
                ["import", "plant", "plant.hcl", "--name", "heat", "--json"],
                "invalid value for one of the arguments: `--name <NAME=PATH>`: \
                 `heat` has no `=` between a name and a path",
            ),
            (
                ["import", "plant", "plant.hcl", "--match", "*", "--point=x"],
                "an argument cannot be used with one or more of the other specified \
                 arguments: `--match <PATTERN>`",
            ),
        ];
        for (args, message) in cases {
            let (run, _, errors) = remote.cli(&paths, &args).await;
            assert_eq!(run, Run::Exit(2), "{errors}");
            let error = Error::Argument {
                message: message.to_owned(),
            };
            assert!(errors.contains(&error.to_string()), "{errors}");
        }
        assert_eq!(remote.calls(), (0, 0));
    });
}

/// A tool call that gives a point as a list, not an object, is a bad argument, and
/// writes nothing.
#[test]
fn mcp_import_refuses_a_point_as_a_list() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let remote = Remote::new(create_browsing_node(&fixture, mesh));
        let input = json!({
            "connector": "plant",
            "files": ["plant.hcl"],
            "points": [["Line3/Temperature", "temp"]],
        });
        let (run, replies) =
            remote.mcp(&paths, &[], &[call(1, "import", &input)]).await;
        assert_eq!(run, Run::Exit(0));
        let error = &replies[0]["error"];
        assert_eq!(error["code"], -32602, "{error}");
        assert_eq!(
            error["message"],
            "invalid type: sequence, expected an object"
        );
        assert_eq!(plant(&paths), PLANT);
    });
}

/// `tools/list` holds `import`, neither read-only nor destructive, with its input, and
/// a tool call gives the JSON of `foundation import --json`.
#[test]
fn mcp_import_gives_the_json_of_the_command_line() {
    solo(|fixture, mesh| async move {
        let paths = with_plant(&fixture.sim, PLANT);
        let remote = Remote::new(create_browsing_node(&fixture, mesh));
        let input = json!({
            "connector": "plant",
            "files": ["plant.hcl"],
            "start": "Line3",
            "points": [
                { "path": "Line3/Temperature" },
                { "path": "Line3/Heater", "name": "heat" },
            ],
        });
        let list = json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/list" });
        let messages = [list, call(1, "import", &input)];
        let (run, replies) = remote.mcp(&paths, &[], &messages).await;
        assert_eq!(run, Run::Exit(0));
        let tool = replies[0]["result"]["tools"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|tool| tool["name"] == "import")
            .expect("an import tool")
            .clone();
        assert_eq!(tool["annotations"]["readOnlyHint"], false);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        let schema = &tool["inputSchema"];
        let properties: Vec<_> = schema["properties"]
            .as_object()
            .expect("an object")
            .keys()
            .collect();
        assert_eq!(
            properties,
            ["connector", "files", "match", "points", "start"]
        );
        assert_eq!(schema["required"], json!(["connector", "files"]));
        let point = &schema["properties"]["points"]["items"];
        assert_eq!(point["required"], json!(["path"]));
        assert_eq!(plant(&paths), IMPORTED);
        paths
            .write(Path::new("plant.hcl"), PLANT.as_bytes())
            .expect("a write");
        let mut args = ARGS.to_vec();
        args.push("--json");
        let (run, json, errors) = remote.cli(&paths, &args).await;
        assert_eq!((run, errors.as_str()), (Run::Exit(0), ""));
        let output: Value = serde_json::from_str(&json).expect("JSON");
        assert_eq!(replies[1]["result"]["structuredContent"], output);
        assert_eq!(output["channels"][1]["name"], "heat");
    });
}
