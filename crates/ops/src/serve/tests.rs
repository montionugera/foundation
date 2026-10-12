use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mesh::used::Cause;
use serde_json::{Value, json};
use spec::definition::Definition;
use types::name::Name;
use wire::hub::client::BODY_BYTES_MAX;

use super::{Large, Reply, serve};
use crate::Node;
use crate::common::{
    NODE, PLANT, SITE, create_node, fail_sync, name, placed_site, solo,
};
use crate::plan;

/// Lines that `ssh-keygen -t ed25519` wrote.
const BOB: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIP0QMDFGOHfS9XR71aVyCvs+QnNQ4BXrHs9dGDDz7KY6",
    " bob@site",
);
const EVE: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIGVVuOR8JKYpAcWLMUveadmJ1wUAmYGgIDtqlhFe7Yhg",
    " eve@laptop",
);
const SAM: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIBwW/hh0ZkgH9jcODHef/ulXUH2Hx66uLYTxi9hWfS+o",
    " sam@site",
);

const DENIED_FIX: &str =
    "Ask an admin of the region for an access policy that allows it";
/// What `ops.denied` names for a request that names no definition.
const NOTHING: &str = "a request that names no definition";

/// The first line of the reply of `ops.large-reply`.
fn large() -> Value {
    let message = format!(
        "the reply has more than {BODY_BYTES_MAX} bytes, the most that a reply holds"
    );
    refusal(
        "ops.large-reply",
        1,
        &message,
        "Plan and apply the change in smaller steps",
    )
}

/// The subject of the first admin, which may do each action on each name.
fn admin() -> Name {
    name("@admin")
}

/// Three subjects. `bob` may plan each name. `sam` may plan and apply each name
/// under `site`. `eve` has no access policy.
fn people() -> String {
    format!(
        r#"subject "bob" {{ keys = ["{BOB}"] }}
subject "eve" {{ keys = ["{EVE}"] }}
subject "sam" {{ keys = ["{SAM}"] }}
access "planner" {{
  subjects = "bob"
  select = "**"
  allow = [plan]
}}
access "surveyor" {{
  subjects = "sam"
  select = "site.**"
  allow = [plan, apply]
}}
"#
    )
}

/// `site.hcl` with a placement under `site` that homes its index on `edge`.
fn site() -> String {
    let placement = "placement \"site.home\" {\n  select = \"site.*\"\n  home = \
                     \"edge\"\n}\n";
    format!("{SITE}{placement}")
}

/// The first line of the reply of `ops.stale-plan` for a plan at `base` on a region
/// at `pointer`.
fn stale(base: spec::Pointer, pointer: spec::Pointer) -> Value {
    let message = format!(
        "the spec changed: the spec is at {pointer}, not at the base {base} of the plan"
    );
    refusal("ops.stale-plan", 1, &message, "Plan again")
}

/// The body of a `plan` request of `files`, each a path and its text.
fn plan_body(files: &[(&str, &str)]) -> Vec<u8> {
    let files: Vec<Value> = files
        .iter()
        .map(|(path, text)| json!({ "path": path, "text": text }))
        .collect();
    format!("{}\n", json!({ "plan": { "files": files } })).into_bytes()
}

/// The body of an `apply` request of the plan file `plan`.
fn apply_body(plan: &[u8]) -> Vec<u8> {
    [b"{\"apply\":{}}\n", plan].concat()
}

/// The first line of `reply`, as JSON, and the bytes after it.
fn split(reply: &[u8]) -> (Value, &[u8]) {
    let end = reply
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("a reply has a first line");
    let head = serde_json::from_slice(&reply[..end]).expect("a first line of JSON");
    (head, &reply[end + 1..])
}

/// The first line of the reply of one error with no place.
fn refusal(code: &str, status: u8, message: &str, fix: &str) -> Value {
    json!({
        "errors": [{ "code": code, "message": message, "fix": fix, "notes": [] }],
        "status": status,
    })
}

/// The first line of the reply of `ops.bad-request` with `message`.
fn bad(message: &str) -> Value {
    refusal(
        "ops.bad-request",
        1,
        &format!("the body is no request: {message}"),
        "Use a `foundation` program of the same version as the node",
    )
}

/// The first line of the reply of `ops.denied` for `subject` and `action` on `on`.
fn denied(subject: &str, action: &str, on: &str) -> Value {
    let message =
        format!("no access policy gives `{subject}` the action `{action}` on {on}");
    refusal("ops.denied", 1, &message, DENIED_FIX)
}

/// The plan file of `files` on `node`, as `@admin` plans it through `serve`.
async fn planned(node: &Node, files: &[(&str, &str)]) -> Vec<u8> {
    let reply = serve(node, &admin(), &plan_body(files)).await;
    let (head, plan) = split(&reply);
    assert!(head.get("output").is_some(), "{head}");
    plan.to_vec()
}

/// Applies `files` on `node` as `@admin`, through `serve`.
async fn applied(node: &Node, files: &[(&str, &str)]) {
    let plan = planned(node, files).await;
    let reply = serve(node, &admin(), &apply_body(&plan)).await;
    let (head, rest) = split(&reply);
    assert!(head.get("output").is_some(), "{head}");
    assert_eq!(rest, b"");
}

#[test]
fn plans_applies_and_plans_no_change() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let site = placed_site();
        let files = vec![(PathBuf::from("site.hcl"), site.clone())];
        let (plan, output) = node.plan(files.clone()).await.expect("a plan");
        assert_eq!(output["added"], 3, "{output}");
        let reserved = json!([
            { "name": "@admin", "kind": "access", "key": "@admin.@access" },
            { "name": "@admin", "kind": "subject", "key": "@admin.@subject" },
        ]);
        assert_eq!(output["reserved"], reserved);
        let body = plan_body(&[("site.hcl", &site)]);
        let reply = serve(&node, &admin(), &body).await;
        assert_eq!(split(&reply), (json!({ "output": output }), &*plan));

        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        let pointer = plan::Pointer::from(mesh.pointer());
        assert_eq!(pointer.version, 1);
        let effect = format!(
            "{{\"output\":{{\"pointer\":{{\"version\":1,\"root\":\"{}\"}},\
             \"added\":3,\"changed\":0,\"removed\":0,\"homes\":1}}}}\n",
            pointer.root
        );
        assert_eq!(String::from_utf8(reply), Ok(effect));

        let (plan, output) = node.plan(files).await.expect("a second plan");
        assert_eq!(output["changes"], json!([]), "{output}");
        let reply = serve(&node, &admin(), &body).await;
        assert_eq!(split(&reply), (json!({ "output": output }), &*plan));
        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        let (head, rest) = split(&reply);
        assert_eq!(head["output"]["pointer"]["version"], 1, "{head}");
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_the_reply_of_an_error_as_one_line() {
    solo(|_, mesh| async move {
        let node = create_node(mesh);
        let reply = serve(&node, &admin(), b"{\"version\":{}}\n").await;
        let expected = "{\"errors\":[{\"code\":\"ops.bad-request\",\"message\":\"the \
                        body is no request: the operation `version` does not run on \
                        a node\",\"fix\":\"Use a `foundation` program of the same \
                        version as the node\",\"notes\":[]}],\"status\":1}\n";
        assert_eq!(String::from_utf8(reply).as_deref(), Ok(expected));
    });
}

#[test]
fn refuses_each_body_that_is_no_request() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let after = [&plan_body(&[("site.hcl", &site())])[..], b"x"].concat();
        let cases: [(&[u8], &str); 14] = [
            (b"", "no newline ends the first line"),
            (b"{\"apply\":{}}", "no newline ends the first line"),
            (b"\n", "EOF while parsing a value at line 1 column 0"),
            (b"nope\n", "expected ident at line 1 column 2"),
            (
                b"[]\n",
                "invalid type: sequence, expected an object with one operation at \
                 line 1 column 0",
            ),
            (b"{}\n", "no operation at line 1 column 2"),
            (
                b"{\"apply\":{},\"plan\":{\"files\":[]}}\n",
                "a second operation at line 1 column 18",
            ),
            (
                b"{\"apply\":{},\"apply\":{}}\n",
                "a second operation at line 1 column 19",
            ),
            (
                b"{\"plan\":{}}\n",
                "missing field `files` at line 1 column 10",
            ),
            (
                b"{\"plan\":{\"files\":[{\"path\":\"a.hcl\",\"text\":\"\",\
                  \"mode\":1}]}}\n",
                "unknown field `mode`, expected `path` or `text` at line 1 column 50",
            ),
            (
                b"{\"plan\":{\"files\":[],\"force\":true}}\n",
                "unknown field `force`, expected `files` at line 1 column 27",
            ),
            (
                b"{\"apply\":{\"file\":\"site.plan\"}}\n",
                "unknown field `file`, there are no fields at line 1 column 16",
            ),
            (&after, "bytes follow the first line of `plan`"),
            (
                b"{\"docs\":{}}\n",
                "the operation `docs` does not run on a node",
            ),
        ];
        for (body, message) in cases {
            let reply = serve(&node, &admin(), body).await;
            let text = String::from_utf8_lossy(body);
            assert_eq!(split(&reply), (bad(message), &b""[..]), "{text}");
        }
        assert_eq!(mesh.pointer().version, 0);
    });
}

#[test]
fn refuses_an_operation_that_the_table_does_not_hold() {
    solo(|_, mesh| async move {
        let node = create_node(mesh);
        let reply = serve(&node, &admin(), b"{\"export\":{\"to\":1}}\n").await;
        let expected = bad("the operation `export` does not run on a node");
        assert_eq!(split(&reply), (expected, &b""[..]));
    });
}

#[test]
fn gives_each_error_of_plan_and_apply_with_its_status() {
    solo(|sim, mesh| async move {
        let node = create_node(mesh.clone());
        let reply = serve(&node, &admin(), &apply_body(b"plan")).await;
        let expected = refusal(
            "ops.bad-plan",
            2,
            &config::plan::Error::Version { found: b'p' }.to_string(),
            "Make a plan with `foundation plan`, and apply it with no edits",
        );
        assert_eq!(split(&reply), (expected, &b""[..]));

        let body = plan_body(&[("site.txt", SITE)]);
        let reply = serve(&node, &admin(), &body).await;
        let expected = json!({
            "errors": [{
                "code": "ops.unknown-extension",
                "message": "no config syntax reads this file",
                "fix": "Use a file that ends in `.hcl`",
                "place": { "file": "site.txt", "line": 1, "column": 1 },
                "notes": [],
            }],
            "status": 2,
        });
        assert_eq!(split(&reply), (expected, &b""[..]));

        let base = mesh.pointer();
        let plan = planned(&node, &[("site.hcl", &placed_site())]).await;
        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        assert!(split(&reply).0.get("output").is_some());
        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        let expected = stale(base, mesh.pointer());
        assert_eq!(split(&reply), (expected, &b""[..]));

        let plan = planned(&node, &[("plant.hcl", PLANT)]).await;
        let stopped = fail_sync(&sim);
        let expected = refusal(
            "ops.stopped",
            1,
            &stopped.to_string(),
            "Fix the cause in the message, then start the node and plan again",
        );
        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        assert_eq!(split(&reply), (expected.clone(), &b""[..]));
        let body = plan_body(&[("plant.hcl", PLANT)]);
        let reply = serve(&node, &admin(), &body).await;
        assert_eq!(split(&reply), (expected, &b""[..]));
    });
}

#[test]
fn refuses_a_plan_of_a_subject_with_no_grant_on_a_name_of_the_files() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        applied(&node, &[("people.hcl", &people())]).await;
        let site = plan_body(&[("site.hcl", &placed_site())]);

        let reply = serve(&node, &name("eve"), &site).await;
        let expected = denied("eve", "plan", "`p.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        // A subject that the spec does not hold has no grant.
        let reply = serve(&node, &name("mallory"), &site).await;
        let expected = denied("mallory", "plan", "`p.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        // `sam` may plan the names under `site`, and not the placement.
        let reply = serve(&node, &name("sam"), &site).await;
        let expected = denied("sam", "plan", "`p.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
    });
}

#[test]
fn refuses_a_plan_of_a_subject_with_no_grant_on_a_name_of_the_spec() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        applied(&node, &[("people.hcl", &people)]).await;
        // The files of `sam` hold only names under `site`. The spec in use also
        // holds the subjects and the access policies, which `plan` would remove.
        let site = site();
        let reply =
            serve(&node, &name("sam"), &plan_body(&[("site.hcl", &site)])).await;
        let expected = denied(
            "sam",
            "plan",
            "a definition of the spec in use that the files do not hold",
        );
        assert_eq!(split(&reply), (expected, &b""[..]));

        let both = [("people.hcl", &*people), ("site.hcl", &*site)];
        let reply = serve(&node, &name("bob"), &plan_body(&both)).await;
        let (head, plan) = split(&reply);
        assert_eq!(head["output"]["added"], 3, "{head}");
        assert_eq!(plan, planned(&node, &both).await);
    });
}

#[test]
fn plans_for_a_subject_with_no_grant_on_a_founding_definition() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let policy = "access \"site.surveyor\" {\n  subjects = \"sam\"\n  select = \
                      \"site.**\"\n  allow = [plan]\n}\n";
        applied(&node, &[("people.hcl", policy)]).await;
        let site = site();
        let files = [("people.hcl", policy), ("site.hcl", &*site)];
        let reply = serve(&node, &name("sam"), &plan_body(&files)).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["added"], 3, "{head}");
    });
}

#[test]
fn refuses_an_apply_of_a_subject_with_no_grant_on_a_changed_name() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        applied(&node, &[("people.hcl", &people)]).await;
        let version = mesh.pointer().version;
        let placed = placed_site();
        let files = [("people.hcl", &*people), ("site.hcl", &*placed)];
        let reply = serve(&node, &name("bob"), &plan_body(&files)).await;
        let (head, plan) = split(&reply);
        assert_eq!(head["output"]["added"], 3, "{head}");

        let apply = apply_body(plan);
        let reply = serve(&node, &name("bob"), &apply).await;
        let expected = denied("bob", "apply", "`p.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        let reply = serve(&node, &name("eve"), &apply).await;
        let expected = denied("eve", "apply", "`p.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        // `sam` may apply the names under `site`, and not the placement.
        let reply = serve(&node, &name("sam"), &apply).await;
        let expected = denied("sam", "apply", "`p.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        assert_eq!(mesh.pointer().version, version);
    });
}

#[test]
fn applies_for_a_subject_with_a_grant_on_each_changed_name_and_each_home() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        let placement = site().replace(SITE, "");
        applied(&node, &[("people.hcl", &people), ("p.hcl", &placement)]).await;
        let version = mesh.pointer().version;
        let site = site();
        let files = [("people.hcl", &*people), ("site.hcl", &*site)];
        let plan = planned(&node, &files).await;
        // `sam` has no `plan` on each name of the spec in use, and applies a plan
        // that changes only names under `site`.
        let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["added"], 2, "{head}");
        assert_eq!(head["output"]["homes"], 1, "{head}");
        assert_eq!(mesh.pointer().version, version + 1);
    });
}

/// The home of the index `index` in `mesh`.
async fn home(mesh: &mesh::Mesh, index: &str) -> Option<types::node::Key> {
    let spec = mesh.spec().await.expect("a spec");
    let Definition::Channel(channel) = &spec.definitions[&name(index)] else {
        panic!("a channel at {index}");
    };
    mesh.watch(channel.key)
        .next()
        .await
        .expect("a running mesh")
}

/// The plan of `files` that `@admin` makes and applies with no home, and the homes
/// that it leaves out.
async fn applied_with_no_home(
    node: &Node,
    files: &[(&str, &str)],
) -> BTreeMap<Name, Name> {
    let plan = planned(node, files).await;
    let mut plan = config::plan::Plan::decode(&plan).expect("a plan");
    let homes = std::mem::take(&mut plan.homes);
    let reply = serve(node, &admin(), &apply_body(&plan.encode())).await;
    assert!(split(&reply).0.get("output").is_some());
    homes
}

#[test]
fn an_apply_gives_no_home_to_an_index_on_which_the_subject_has_no_grant() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        applied(&node, &[("people.hcl", &people)]).await;
        let placed = placed_site();
        let files = [("people.hcl", &*people), ("site.hcl", &*placed)];
        let plan = planned(&node, &files).await;
        let site = config::plan::Plan::decode(&plan).expect("a plan");
        let homes = applied_with_no_home(&node, &files).await;
        assert_eq!(homes.keys().collect::<Vec<_>>(), [&name("site.time")]);
        let mut only = site;
        only.base = mesh.pointer();
        only.changes.clear();
        only.homes = homes;
        let version = mesh.pointer().version;

        let apply = apply_body(&only.encode());
        // With the home dropped, the plan of `bob` names no definition.
        let reply = serve(&node, &name("bob"), &apply).await;
        let expected = denied("bob", "apply", NOTHING);
        assert_eq!(split(&reply), (expected, &b""[..]));
        assert_eq!(mesh.pointer().version, version);
        assert_eq!(home(&mesh, "site.time").await, None);
        let reply = serve(&node, &name("sam"), &apply).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["homes"], 1, "{head}");
        assert_eq!(mesh.pointer().version, version + 1);
        assert_eq!(home(&mesh, "site.time").await, Some(NODE));
    });
}

#[test]
fn an_apply_gives_a_home_only_to_the_index_of_the_grant_of_the_subject() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        applied(&node, &[("people.hcl", &people)]).await;
        let site = site();
        let yard = "channel \"yard.time\" { kind = \"index\" }\nplacement \
                    \"yard.home\" {\n  select = \"yard.*\"\n  home = \"edge\"\n}\n";
        let files = [
            ("people.hcl", &*people),
            ("site.hcl", &*site),
            ("yard.hcl", yard),
        ];
        let homes = applied_with_no_home(&node, &files).await;
        assert_eq!(homes.len(), 2);
        let version = mesh.pointer().version;
        // `sam` may apply `site.hum`, and has no grant on `yard.time`.
        let hum = format!(
            "{site}channel \"site.hum\" {{\n  data_type = \"f64\"\n  index = \
             \"site.time\"\n}}\n"
        );
        let files = [
            ("people.hcl", &*people),
            ("site.hcl", &*hum),
            ("yard.hcl", yard),
        ];
        let plan = planned(&node, &files).await;

        let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["added"], 1, "{head}");
        assert_eq!(head["output"]["homes"], 1, "{head}");
        assert_eq!(mesh.pointer().version, version + 1);
        assert_eq!(home(&mesh, "site.time").await, Some(NODE));
        assert_eq!(home(&mesh, "yard.time").await, None);
    });
}

#[test]
fn refuses_a_subject_with_no_grant_before_it_compares_the_base_of_a_plan() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let base = mesh.pointer();
        let plan = planned(&node, &[("site.hcl", &site())]).await;
        applied(&node, &[("people.hcl", &people())]).await;
        let apply = apply_body(&plan);
        let reply = serve(&node, &name("eve"), &apply).await;
        let expected = denied("eve", "apply", "`site.home.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        let reply = serve(&node, &admin(), &apply).await;
        let expected = stale(base, mesh.pointer());
        assert_eq!(split(&reply), (expected, &b""[..]));
    });
}

#[test]
fn refuses_a_plan_and_an_apply_on_a_node_that_uses_an_old_spec_as_behind() {
    solo(|sim, mesh| async move {
        let node = create_node(mesh.clone());
        let site = placed_site();
        applied(&node, &[("site.hcl", &site)]).await;
        let first = mesh.pointer();
        let both = [("site.hcl", &*site), ("plant.hcl", PLANT)];
        let plan = planned(&node, &both).await;
        sim.fail_file(Path::new("spec"), env::files::Operation::SyncDir);
        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        assert!(split(&reply).0.get("output").is_some());
        let spec = mesh.spec().await.expect("a spec");
        assert_eq!(spec.pointer, Some(first));
        let behind = spec.behind.expect("a node behind");
        let Cause::Files(error) = &behind.cause else {
            panic!("a pointer file that is not durable, not {behind:?}");
        };
        let message = format!(
            "the node does not use the newest spec, at {}: the file of the pointer \
             in use was not made durable: {error}",
            behind.pointer
        );
        let expected =
            refusal("ops.behind", 1, &message, "Fix the cause, then plan again");

        let reply = serve(&node, &admin(), &plan_body(&both)).await;
        assert_eq!(split(&reply), (expected.clone(), &b""[..]));
        let reply = serve(&node, &admin(), &apply_body(&plan)).await;
        assert_eq!(split(&reply), (expected, &b""[..]));
        // The check of the subject comes first, on the rules of the spec in use.
        let reply = serve(&node, &name("eve"), &plan_body(&both)).await;
        let expected = denied("eve", "plan", "`influx`");
        assert_eq!(split(&reply), (expected, &b""[..]));
    });
}

#[test]
fn applies_a_plan_that_removes_the_policy_of_the_subject() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let policy = "access \"site.surveyor\" {\n  subjects = \"sam\"\n  select = \
                      \"site.**\"\n  allow = [plan, apply, admin]\n}\n";
        applied(&node, &[("people.hcl", policy)]).await;
        let version = mesh.pointer().version;
        let reply = serve(&node, &name("sam"), &plan_body(&[])).await;
        let (head, plan) = split(&reply);
        assert_eq!(head["output"]["removed"], 1, "{head}");

        let reply = serve(&node, &name("sam"), &apply_body(plan)).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["removed"], 1, "{head}");
        assert_eq!(mesh.pointer().version, version + 1);
    });
}

#[test]
fn refuses_a_change_of_access_of_a_subject_with_no_admin_on_its_key() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        applied(&node, &[("people.hcl", &people)]).await;
        let version = mesh.pointer().version;
        // A policy governs its whole region, whatever its key.
        let policy = "access \"site.evil\" {\n  subjects = \"sam\"\n  select = \
                      \"**\"\n  allow = [plan, apply]\n}\n";
        let mole = format!("subject \"site.mole\" {{ keys = [\"{EVE}\"] }}\n");
        let cases = [
            (policy, "`site.evil.@access`"),
            (&*mole, "`site.mole.@subject`"),
        ];
        for (text, key) in cases {
            let files = [("people.hcl", &*people), ("evil.hcl", text)];
            let plan = planned(&node, &files).await;
            let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
            let expected = denied("sam", "admin", key);
            assert_eq!(split(&reply), (expected, &b""[..]));
        }
        assert_eq!(mesh.pointer().version, version);

        let admin =
            people.replace("allow = [plan, apply]", "allow = [plan, apply, admin]");
        applied(&node, &[("people.hcl", &admin)]).await;
        for (text, _) in cases {
            let version = mesh.pointer().version;
            let files = [("people.hcl", &*admin), ("evil.hcl", text)];
            let plan = planned(&node, &files).await;
            let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
            let (head, _) = split(&reply);
            assert_eq!(head["output"]["added"], 1, "{head}");
            assert_eq!(mesh.pointer().version, version + 1);
        }
    });
}

#[test]
fn refuses_a_policy_of_a_subject_with_no_admin_on_its_key() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        let yard = "channel \"yard.time\" { kind = \"index\" }\nplacement \
                    \"yard.home\" {\n  select = \"yard.*\"\n  home = \"edge\"\n}\n";
        applied(&node, &[("people.hcl", &people), ("yard.hcl", yard)]).await;
        let version = mesh.pointer().version;
        // `sam` has no grant on `yard.time`, which each policy selects.
        let wipe =
            "retention \"site.wipe\" {\n  select = \"yard.*\"\n  keep = \"0s\"\n}\n";
        let away = "placement \"site.away\" {\n  select = \"yard.time\"\n  home = \
                    \"edge\"\n}\n";
        let cases = [
            (wipe, "`site.wipe.@retention`"),
            (away, "`site.away.@placement`"),
        ];
        for (text, key) in cases {
            let files = [
                ("people.hcl", &*people),
                ("yard.hcl", yard),
                ("p.hcl", text),
            ];
            let plan = planned(&node, &files).await;
            let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
            let expected = denied("sam", "admin", key);
            assert_eq!(split(&reply), (expected, &b""[..]), "{text}");
        }
        assert_eq!(mesh.pointer().version, version);

        let admin =
            people.replace("allow = [plan, apply]", "allow = [plan, apply, admin]");
        applied(&node, &[("people.hcl", &admin), ("yard.hcl", yard)]).await;
        for (text, _) in cases {
            let version = mesh.pointer().version;
            let files = [("people.hcl", &*admin), ("yard.hcl", yard), ("p.hcl", text)];
            let plan = planned(&node, &files).await;
            let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
            let (head, _) = split(&reply);
            assert_eq!(head["output"]["added"], 1, "{head}");
            assert_eq!(mesh.pointer().version, version + 1);
        }
    });
}

#[test]
fn refuses_a_change_or_removal_of_access_with_no_admin_on_its_key() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        let crew = "access \"site.crew\" {\n  subjects = \"bob\"\n  select = \
                    \"site.*\"\n  allow = [plan]\n}\n";
        let mole = format!("subject \"site.mole\" {{ keys = [\"{EVE}\"] }}\n");
        let both = format!("{crew}{mole}");
        applied(&node, &[("people.hcl", &people), ("crew.hcl", &both)]).await;
        let version = mesh.pointer().version;
        let taken = crew
            .replace("\"bob\"", "\"sam\"")
            .replace("\"site.*\"", "\"**\"")
            .replace("[plan]", "[plan, apply, admin]");
        let rekeyed = mole.replace(EVE, SAM);
        let cases = [
            (format!("{taken}{mole}"), "`site.crew.@access`"),
            (mole.clone(), "`site.crew.@access`"),
            (format!("{crew}{rekeyed}"), "`site.mole.@subject`"),
            (crew.to_owned(), "`site.mole.@subject`"),
        ];
        for (text, key) in &cases {
            let files = [("people.hcl", &*people), ("crew.hcl", &**text)];
            let plan = planned(&node, &files).await;
            let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
            let expected = denied("sam", "admin", key);
            assert_eq!(split(&reply), (expected, &b""[..]), "{text}");
        }
        assert_eq!(mesh.pointer().version, version);
    });
}

#[test]
fn refuses_a_request_that_names_no_definition_for_each_subject() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        // `plan` compares no founding definition, so the spec has no name to check.
        for subject in ["@admin", "mallory"] {
            let reply = serve(&node, &name(subject), &plan_body(&[])).await;
            let expected = denied(subject, "plan", NOTHING);
            assert_eq!(split(&reply), (expected, &b""[..]));
        }
        let plan = planned(&node, &[("site.hcl", &site())]).await;
        let mut empty = config::plan::Plan::decode(&plan).expect("a plan");
        empty.changes.clear();
        empty.homes.clear();
        applied(&node, &[("people.hcl", &people())]).await;
        let version = mesh.pointer().version;
        empty.base = mesh.pointer();
        let apply = apply_body(&empty.encode());
        for subject in ["@admin", "mallory"] {
            let reply = serve(&node, &name(subject), &apply).await;
            let expected = denied(subject, "apply", NOTHING);
            assert_eq!(split(&reply), (expected, &b""[..]));
        }
        assert_eq!(mesh.pointer().version, version);
    });
}

#[test]
fn refuses_each_remote_subject_on_a_node_that_uses_no_spec() {
    let mut founding = spec::founding::create(crate::common::ADMIN.public());
    let policy = founding[&name("@admin.@access")].clone();
    // An access policy at a key that is no tree key of its kind: a problem.
    founding.insert(name("x"), policy);
    crate::common::founded(&["edge"], founding, |_, mesh| async move {
        let node = create_node(mesh.clone());
        let spec = mesh.spec().await.expect("a spec");
        assert_eq!(spec.pointer, None, "{spec:?}");
        assert!(spec.behind.is_some(), "{spec:?}");
        let site = site();
        let reply = serve(&node, &admin(), &plan_body(&[("site.hcl", &site)])).await;
        let expected = denied("@admin", "plan", "`site.home.@placement`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        let reply = serve(&node, &admin(), &plan_body(&[])).await;
        let expected = denied("@admin", "plan", NOTHING);
        assert_eq!(split(&reply), (expected, &b""[..]));
        let local = node.plan(vec![(PathBuf::from("site.hcl"), site)]).await;
        let error = local.expect_err("a node behind");
        assert_eq!(error["errors"][0]["code"], "ops.behind", "{error}");
    });
}

/// The `plan` body of `PLANT` at a path of `long` bytes, which each change of the
/// reply holds, and of one subject at a path of `short` bytes.
fn wide_body(long: usize, short: usize) -> Vec<u8> {
    let long = format!("{}.hcl", "x".repeat(long - 4));
    let short = format!("{}.hcl", "y".repeat(short - 4));
    let subject = format!("subject \"bob\" {{ keys = [\"{BOB}\"] }}\n");
    plan_body(&[(&long, PLANT), (&short, &subject)])
}

#[test]
fn gives_a_reply_at_the_bound_and_refuses_one_over_it() {
    solo(|_, mesh| async move {
        let node = create_node(mesh);
        let bound = usize::try_from(BODY_BYTES_MAX).expect("the bound fits");
        // Each of the 18 changes of `PLANT` holds the long path.
        let long = bound / 18 - 1_000;
        let reply = serve(&node, &admin(), &wide_body(long, 4)).await;
        assert!(split(&reply).0.get("output").is_some());
        let short = 4 + bound - reply.len();

        let reply = serve(&node, &admin(), &wide_body(long, short)).await;
        assert_eq!(reply.len(), bound);
        assert!(split(&reply).0.get("output").is_some());

        let reply = serve(&node, &admin(), &wide_body(long, short + 1)).await;
        assert_eq!(split(&reply), (large(), &b""[..]));

        let reply = serve(&node, &admin(), &plan_body(&[("site.hcl", &site())])).await;
        assert_eq!(split(&reply).0["output"]["added"], 3);
    });
}

#[test]
fn refuses_the_reply_of_many_problems_at_a_long_path() {
    solo(|_, mesh| async move {
        let node = create_node(mesh);
        let path = format!("{}.hcl", "x".repeat((64 << 10) - 4));
        let text = "nope \"a\" {}\n".repeat(2_000);
        let reply = serve(&node, &admin(), &plan_body(&[(&path, &text)])).await;
        assert_eq!(split(&reply), (large(), &b""[..]));
    });
}

/// A JSON list of `len` strings of 100 bytes, which counts each string it writes.
struct Strings {
    len: usize,
    written: Cell<usize>,
}

impl serde::Serialize for Strings {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut list = serializer.serialize_seq(Some(self.len))?;
        for _ in 0..self.len {
            self.written.set(self.written.get() + 1);
            list.serialize_element(&"x".repeat(98))?;
        }
        list.end()
    }
}

#[test]
fn stops_writing_a_reply_at_the_bound() {
    let bound = usize::try_from(BODY_BYTES_MAX).expect("the bound fits");
    // The JSON of each string is 101 bytes with its comma.
    let fits = (bound - 2) / 101;
    let strings = Strings {
        len: fits,
        written: Cell::new(0),
    };
    Reply::line(&strings).expect("a reply at the bound");
    let strings = Strings {
        len: 10 * fits,
        written: Cell::new(0),
    };
    let Err(Large) = Reply::line(&strings) else {
        panic!("a reply over the bound fits");
    };
    assert_eq!(strings.written.get(), fits + 1);
}

/// A value that no serializer can write.
struct Unwritable;

impl serde::Serialize for Unwritable {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("no JSON"))
    }
}

#[test]
#[should_panic(expected = "invariant: a reply is plain JSON data: no JSON")]
fn panics_on_a_reply_that_is_no_json_data() {
    if let Err(Large) = Reply::line(&Unwritable) {
        panic!("`Large` in place of the panic");
    }
}
