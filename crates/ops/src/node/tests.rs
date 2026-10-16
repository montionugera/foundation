use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use connector::testing::create_secret;
use document::diagnostic::{Code, Diagnostic};
use document::{Position, Span};
use mesh::Mesh;
use serde_json::{Value, json};

use crate::apply::{Applied, Effect};
use crate::common::{
    ADMIN, Fixture, NODE, create_config, create_kinds, create_node,
    create_node_reading, create_node_with, fail_sync, founded, front_ends, name,
    placed_site, plan, plant, shown, solo,
};
use crate::error::Error;
use crate::front_end::{File, FrontEnd, FrontEnds};
use crate::plan::{Counts, Output, Pointer};
use crate::used;

const REFUSED: Code = Code::new("test.refused");
/// The directory of the store that `connector::testing::create_secret` writes.
const SECRETS: &str = "secrets";

/// The output of `plan` of `files` on `mesh`.
async fn planned(mesh: &Mesh, files: Vec<(PathBuf, String)>) -> Output {
    let spec = mesh.spec().await.expect("a spec");
    let files: Vec<File> = files
        .into_iter()
        .map(|(path, text)| File { path, text })
        .collect();
    let base = spec.pointer.expect("a spec in use");
    let names = mesh.names();
    let kinds = create_kinds();
    let planned = plan(
        &files,
        base,
        &spec.definitions,
        &names,
        &front_ends(),
        &kinds,
    );
    planned.expect("a plan").0
}

fn site() -> Vec<(PathBuf, String)> {
    vec![(PathBuf::from("site.hcl"), placed_site())]
}

#[test]
fn applies_with_each_connector_kind_of_the_node() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh.clone());
        let files = vec![(PathBuf::from("plant.hcl"), plant().to_owned())];
        let (plan, _) = node.plan(files).await.expect("a plan");
        let applied = node.apply(Path::new("plant.plan"), &plan).await;
        let expected = Applied {
            file: "plant.plan".to_owned(),
            effect: Effect {
                pointer: Pointer::from(mesh.pointer()),
                counts: Counts {
                    added: 28,
                    changed: 0,
                    removed: 0,
                },
                homes: 3,
            },
        };
        assert_eq!(expected.text(), shown("Applied"));
        assert_eq!(applied, Ok(serde_json::to_value(expected).expect("JSON")));
    });
}

#[test]
fn plans_the_config_of_the_quickstart_page_as_the_page_shows() {
    solo(|_, mesh| async move {
        let files = vec![(PathBuf::from("plant.hcl"), plant().to_owned())];
        assert_eq!(planned(&mesh, files).await.text(), shown("+ channel"));
    });
}

#[test]
fn plans_and_applies_in_the_json_of_the_cli() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh.clone());
        let expected = crate::value(&planned(&mesh, site()).await);
        let (plan, output) = node.plan(site()).await.expect("a plan");
        assert_eq!(output["added"], 3, "{output}");
        assert_eq!(output, expected);
        let applied = node.apply(Path::new("site.plan"), &plan).await;
        let pointer = mesh.pointer();
        let expected = Applied {
            file: "site.plan".to_owned(),
            effect: Effect {
                pointer: Pointer::from(pointer),
                counts: Counts {
                    added: 3,
                    changed: 0,
                    removed: 0,
                },
                homes: 1,
            },
        };
        assert_eq!(applied, Ok(serde_json::to_value(expected).expect("JSON")));
        let (_, output) = node.plan(site()).await.expect("a second plan");
        assert_eq!(output["changes"], json!([]), "{output}");
        let definitions = mesh.spec().await.expect("a spec").definitions;
        let Some(spec::definition::Definition::Channel(time)) =
            definitions.get(&"site.time".parse().expect("a name"))
        else {
            panic!("a channel at site.time");
        };
        assert_eq!(mesh.watch(time.key).next().await, Ok(Some(NODE)));
    });
}

#[test]
fn plans_with_each_connector_kind_of_the_node() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh.clone());
        let files = vec![(PathBuf::from("plant.hcl"), plant().to_owned())];
        let expected = crate::value(&planned(&mesh, files.clone()).await);
        let (_, output) = node.plan(files).await.expect("a plan");
        assert_eq!(output, expected);
    });
}

#[test]
fn gives_the_json_error_of_a_plan() {
    solo(|fixture, mesh| async move {
        assert_unknown(&fixture, mesh, front_ends(), "`.hcl`").await;
    });
}

#[test]
fn gives_the_json_error_of_an_apply() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh.clone());
        let base = mesh.pointer();
        let (plan, _) = node.plan(site()).await.expect("a plan");
        let path = Path::new("site.plan");
        node.apply(path, &plan).await.expect("an apply");
        let error = node.apply(path, &plan).await.expect_err("a stale plan");
        let pointer = mesh.pointer();
        assert_eq!(error, Error::Stale { base, pointer }.json());
    });
}

#[test]
fn debug_names_each_front_end() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let expected = r#"Node { front_ends: FrontEnds(["hcl"]), .. }"#;
        assert_eq!(format!("{node:?}"), expected);
        let secrets = connector::testing::create_secrets(fixture.sim.files());
        let config =
            create_config(&fixture, node.mesh().clone(), front_ends(), secrets);
        let expected =
            r#"Config { front_ends: FrontEnds(["hcl"]), name: Name("edge"), .. }"#;
        assert_eq!(format!("{config:?}"), expected);
        assert_eq!(
            format!(
                "{:?}",
                FrontEnd {
                    read: config_hcl::read
                }
            ),
            "FrontEnd { .. }"
        );
    });
}

/// Plans a `.txt` file on a node of `mesh` that reads with `front_ends`, and asserts
/// that its error names `extensions` in the fix.
async fn assert_unknown(
    fixture: &Fixture,
    mesh: Mesh,
    front_ends: FrontEnds,
    extensions: &str,
) {
    let node = create_node_reading(fixture, mesh, front_ends);
    let files = vec![(PathBuf::from("site.txt"), placed_site())];
    let error = node.plan(files).await.expect_err("an unknown extension");
    let expected = json!({ "errors": [{
        "code": "ops.unknown-extension",
        "fix": format!("Use a file that ends in {extensions}"),
        "message": "no config syntax reads this file",
        "notes": [],
        "place": { "column": 1, "file": "site.txt", "line": 1 },
    }]});
    assert_eq!(error, expected);
}

#[test]
fn names_each_extension_of_the_table_in_the_fix() {
    solo(|fixture, mesh| async move {
        let two = front_ends().with(
            "toml",
            FrontEnd {
                read: config_hcl::read,
            },
        );
        assert_unknown(&fixture, mesh.clone(), two.clone(), "`.hcl` or `.toml`").await;
        let three = two.with(
            "yaml",
            FrontEnd {
                read: config_hcl::read,
            },
        );
        assert_unknown(&fixture, mesh, three, "`.hcl`, `.toml`, or `.yaml`").await;
    });
}

#[test]
fn reads_with_the_last_front_end_of_an_extension() {
    solo(|fixture, mesh| async move {
        let refuse = FrontEnd {
            read: |source, _| {
                let start = Position {
                    offset: 0,
                    line: 0,
                    column: 0,
                };
                Err(vec![Diagnostic::new(
                    REFUSED,
                    Span::new(source, start, start),
                    "refused".to_owned(),
                    "Fix it".to_owned(),
                )])
            },
        };
        let front_ends = FrontEnds::new("hcl", refuse).with(
            "hcl",
            FrontEnd {
                read: config_hcl::read,
            },
        );
        let node = create_node_reading(&fixture, mesh.clone(), front_ends);
        let expected = crate::value(&planned(&mesh, site()).await);
        let (_, output) = node.plan(site()).await.expect("a plan");
        assert_eq!(output, expected);
    });
}

/// The output of `ops.stopped` for `stopped`.
fn stopped_errors(stopped: &mesh::Stopped) -> Value {
    json!({ "errors": [{
        "code": "ops.stopped",
        "message": stopped.to_string(),
        "fix": "Fix the cause in the message, then start the node and plan again",
        "notes": [],
    }] })
}

#[test]
fn gives_a_stop_of_the_group_as_stopped_in_plan() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh.clone());
        let (plan, _) = node.plan(site()).await.expect("a plan");
        let stopped = fail_sync(&fixture.sim);
        let applied = node.apply(Path::new("site.plan"), &plan).await;
        assert_eq!(applied, Err(stopped_errors(&stopped)));
        let planned = node.plan(site()).await.map(|(_, output)| output);
        assert_eq!(planned, Err(stopped_errors(&stopped)));
        let error = used::spec(&mesh).await.expect_err("a stop");
        assert_eq!(error, Error::Stopped(stopped));
        assert_eq!(error.status(), 1);
    });
}

#[test]
fn gives_a_stop_of_the_group_as_stopped_in_apply_of_a_plan_with_no_change() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let (plan, _) = node.plan(site()).await.expect("a plan");
        let (empty, output) = node.plan(Vec::new()).await.expect("a plan");
        assert_eq!(output["added"], json!(0));
        assert_eq!(output["removed"], json!(0));
        let stopped = fail_sync(&fixture.sim);
        let applied = node.apply(Path::new("site.plan"), &plan).await;
        assert_eq!(applied, Err(stopped_errors(&stopped)));
        let applied = node.apply(Path::new("empty.plan"), &empty).await;
        assert_eq!(applied, Err(stopped_errors(&stopped)));
    });
}

/// A config with a connector on the node `node` that reads the file `/run/ca` in a
/// block, `plant.pw` from the store in a list, and `TOKEN` and `HEADER` from the
/// environment, the second in a map.
fn with_sources(node: &str) -> Vec<(PathBuf, String)> {
    let text = format!(
        r#"connector "c" {{
  kind  = "influx"
  node  = "{node}"
  tls {{
    ca = file("/run/ca")
  }}
  users = [secret("plant.pw")]
  token = env("TOKEN")
  headers = {{ a = env("HEADER") }}
}}
"#
    );
    vec![(PathBuf::from("c.hcl"), text)]
}

/// The JSON of one `secret.unresolved` problem at `line` and `column` of `c.hcl`.
fn unresolved(message: &str, line: u32, column: u32) -> Value {
    json!({
        "code": "secret.unresolved",
        "fix": "Give the node the value at the source, or change the source, then plan \
                again",
        "message": message,
        "notes": [],
        "place": { "column": column, "file": "c.hcl", "line": line },
    })
}

#[test]
fn refuses_a_plan_with_each_secret_of_the_node_that_has_no_value() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let error = node
            .plan(with_sources("edge"))
            .await
            .expect_err("no values");
        let unset = |name: &str| {
            format!(
                "the secret env(\"{name}\"): the environment variable {name} of the node \
                 is not set. Start the node with it set to the value"
            )
        };
        let expected = json!({ "errors": [
            unresolved(
                "the secret file(\"/run/ca\"): the file /run/ca is not there. Write the \
                 value to it",
                5,
                10,
            ),
            unresolved(
                "the secret secret(\"plant.pw\"): the file /data/secrets/plant.pw is \
                 not there. Write the value to it",
                7,
                12,
            ),
            unresolved(&unset("TOKEN"), 8, 11),
            unresolved(&unset("HEADER"), 9, 19),
        ]});
        assert_eq!(error, expected);
    });
}

#[test]
fn gives_the_secrets_with_no_value_of_each_connector_in_name_order() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let connector = |name: &str| {
            format!(
                "connector \"{name}\" {{\n  kind  = \"influx\"\n  node  = \"edge\"\n  \
                 token = env(\"{name}\")\n}}\n"
            )
        };
        let text = format!("{}{}", connector("b"), connector("a"));
        let files = vec![(PathBuf::from("c.hcl"), text)];
        let error = node.plan(files).await.expect_err("no values");
        let unset = |name: &str| {
            format!(
                "the secret env(\"{name}\"): the environment variable {name} of the node \
                 is not set. Start the node with it set to the value"
            )
        };
        let expected = json!({ "errors": [
            unresolved(&unset("a"), 9, 11),
            unresolved(&unset("b"), 4, 11),
        ]});
        assert_eq!(error, expected);
    });
}

#[test]
fn plans_when_each_secret_of_the_node_has_a_value() {
    solo(|fixture, mesh| async move {
        let files = fixture.sim.files();
        create_secret(&files, &name("plant.pw"), b"pw").await;
        let paths = fixture.sim.paths();
        paths.write(Path::new("/run/ca"), b"ca").expect("writes it");
        let vars = BTreeMap::from([
            (OsString::from("TOKEN"), OsString::from("t")),
            (OsString::from("HEADER"), OsString::from("h")),
        ]);
        let secrets = secret::Resolver::new(Arc::new(vars), files, Path::new(SECRETS));
        let node = create_node_with(&fixture, mesh, front_ends(), secrets);
        let (_, output) = node.plan(with_sources("edge")).await.expect("a plan");
        assert_eq!(output["added"], 10, "{output}");
    });
}

#[test]
fn plans_a_connector_of_another_node_with_no_read_of_its_secrets() {
    founded(
        &["edge", "other"],
        spec::founding::create(ADMIN.public()),
        |fixture, mesh| async move {
            let node = create_node(&fixture, mesh);
            let (_, output) = node.plan(with_sources("other")).await.expect("a plan");
            assert_eq!(output["added"], 10, "{output}");
        },
    );
}

#[test]
fn refuses_a_secret_with_a_bad_name_or_a_file_of_more_than_64_kib() {
    solo(|fixture, mesh| async move {
        let paths = fixture.sim.paths();
        paths
            .write(Path::new("/run/large"), &vec![b'x'; 70 * 1024])
            .expect("writes it");
        let node = create_node(&fixture, mesh);
        let text = r#"connector "c" {
  kind  = "influx"
  node  = "edge"
  token = secret("plant.Token")
  ca    = file("/run/large")
}
"#;
        let files = vec![(PathBuf::from("c.hcl"), text.to_owned())];
        let error = node.plan(files).await.expect_err("no values");
        let expected = json!({ "errors": [
            unresolved(
                "the secret secret(\"plant.Token\"): the store cannot hold the name. \
                 Rename the secret in lowercase letters, since the file store holds no \
                 name with a capital letter",
                4,
                11,
            ),
            unresolved(
                "the secret file(\"/run/large\"): the file /run/large is 71680 bytes, \
                 more than 65536. Make it at most 65536 bytes",
                5,
                11,
            ),
        ]});
        assert_eq!(error, expected);
    });
}

#[test]
fn gives_the_secrets_with_no_value_after_the_other_problems_of_the_plan() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = r#"connector "c" {
  kind  = "influx"
  node  = "edge"
  token = env("TOKEN")
}
placement "c.home" {
  select = "c.**"
  home   = "nowhere"
}
"#;
        let files = vec![(PathBuf::from("c.hcl"), text.to_owned())];
        let error = node.plan(files).await.expect_err("problems");
        let place = json!({ "column": 12, "file": "c.hcl", "line": 8 });
        let expected = json!({ "errors": [
            {
                "code": "config.connector-home",
                "fix": "Name `edge` as the `home`, and keep `edge` out of `standby` and \
                        `copies`",
                "message": "the placement `c.home` names the home `nowhere`, but the \
                            connector `c` runs on the node `edge`",
                "notes": [],
                "place": place,
            },
            {
                "code": "config.unknown-node",
                "fix": "Name a node of the mesh",
                "message": "no node of the mesh is named `nowhere`",
                "notes": [],
                "place": place,
            },
            unresolved(
                "the secret env(\"TOKEN\"): the environment variable TOKEN of the node \
                 is not set. Start the node with it set to the value",
                4,
                11,
            ),
        ]});
        assert_eq!(error, expected);
    });
}
