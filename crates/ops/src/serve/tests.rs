use std::path::{Path, PathBuf};

use mesh::used::Cause;
use serde_json::{Value, json};
use types::name::Name;
use wire::hub::client::BODY_BYTES_MAX;

use super::serve;
use crate::Node;
use crate::common::{PLANT, SITE, create_node, fail_sync, name, placed_site, solo};
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
        let cases: [(&[u8], &str); 13] = [
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
        applied(&node, &[("people.hcl", &people)]).await;
        let version = mesh.pointer().version;
        let site = site();
        let files = [("people.hcl", &*people), ("site.hcl", &*site)];
        let plan = planned(&node, &files).await;
        // `sam` has no `plan` on each name of the spec in use, and applies a plan
        // that changes only names under `site`.
        let reply = serve(&node, &name("sam"), &apply_body(&plan)).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["added"], 3, "{head}");
        assert_eq!(head["output"]["homes"], 1, "{head}");
        assert_eq!(mesh.pointer().version, version + 1);
    });
}

#[test]
fn refuses_an_apply_of_a_subject_with_no_grant_on_a_home() {
    solo(|_, mesh| async move {
        let node = create_node(mesh.clone());
        let people = people();
        applied(&node, &[("people.hcl", &people)]).await;
        let placed = placed_site();
        let files = [("people.hcl", &*people), ("site.hcl", &*placed)];
        let plan = planned(&node, &files).await;
        let mut site = config::plan::Plan::decode(&plan).expect("a plan");
        let homes = std::mem::take(&mut site.homes);
        assert_eq!(homes.keys().collect::<Vec<_>>(), [&name("site.time")]);
        let reply = serve(&node, &admin(), &apply_body(&site.encode())).await;
        assert!(split(&reply).0.get("output").is_some());
        let mut only = site.clone();
        only.base = mesh.pointer();
        only.changes.clear();
        only.homes = homes;
        let version = mesh.pointer().version;

        let apply = apply_body(&only.encode());
        let reply = serve(&node, &name("bob"), &apply).await;
        let expected = denied("bob", "apply", "`site.time`");
        assert_eq!(split(&reply), (expected, &b""[..]));
        assert_eq!(mesh.pointer().version, version);
        let reply = serve(&node, &name("sam"), &apply).await;
        let (head, _) = split(&reply);
        assert_eq!(head["output"]["homes"], 1, "{head}");
        assert_eq!(mesh.pointer().version, version + 1);
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
        let message = format!(
            "the reply has {} bytes, and a reply holds at most {bound}",
            bound + 1
        );
        let expected = refusal(
            "ops.large-reply",
            1,
            &message,
            "Plan and apply the change in smaller steps",
        );
        assert_eq!(split(&reply), (expected, &b""[..]));

        let reply = serve(&node, &admin(), &plan_body(&[("site.hcl", &site())])).await;
        assert_eq!(split(&reply).0["output"]["added"], 3);
    });
}
