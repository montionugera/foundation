//! Runs the built `foundation` binary as a process.

#![cfg(test)]
// The simulated servers need `os::net()`, which a `--cfg loom` build lacks.
#![cfg(not(loom))]

mod one_node;
mod port;
mod quickstart;
mod rig;
#[cfg(unix)]
mod start;
mod status;

use std::collections::BTreeMap;

use rig::Rig;

/// The refusal of `status` on a node that places no connector.
const REFUSED: &str = concat!(
    "error[ops.denied]: no access policy gives `@admin` the action `read` on a ",
    "request that names no definition\n",
    "fix: Ask an admin of the region for an access policy that allows it\n",
);

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8")
}

#[test]
fn an_operation_prints_its_output_and_exits_0() {
    let output = Rig::new().run(&["version"], b"");
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (
            Some(0),
            format!("{}\n", env!("CARGO_PKG_VERSION")).as_str(),
            ""
        )
    );
}

#[test]
fn an_error_goes_to_standard_error_and_exits_2() {
    let output = Rig::new().run(&["versoin"], b"");
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (
            Some(2),
            "",
            "error[ops.unknown-operation]: no operation is named `versoin`\n\
             fix: Use `version`, the closest name\n"
        )
    );
}

/// `plan` and `apply` reach the running node through its hub, as its first admin. A
/// second `plan` of the same files finds no change.
#[test]
fn plan_and_apply_run_on_the_node() {
    let mut rig = Rig::new();
    rig.config(concat!(
        "channel \"plant.time\" { kind = \"index\" }\n",
        "placement \"plant.home\" {\n  select = \"plant.*\"\n  home = \"edge\"\n}\n",
    ));
    rig.start();
    let planned = concat!(
        "+ channel plant.time\n",
        "+ placement plant.home\n",
        "2 to add, 0 to change, 0 to remove.\n",
        "note: 2 definitions that Foundation made are not in the files: ",
        "@admin.@access, @admin.@subject\n",
    );
    let applied = "Applied plant.plan: 2 added, 1 home listed.\n";
    assert_eq!(rig.apply(), [planned, applied]);
    let output = rig.run(&["plan", "plant.hcl"], b"");
    let unchanged = concat!(
        "0 to add, 0 to change, 0 to remove.\n",
        "note: 2 definitions that Foundation made are not in the files: ",
        "@admin.@access, @admin.@subject\n",
    );
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (Some(0), unchanged, "")
    );
}

/// `status --json` gives each connector that the node places, with its error.
#[test]
fn status_gives_each_connector_as_json() {
    let mut rig = Rig::new();
    rig.config(concat!(
        "channel \"plant.time\" { kind = \"index\" }\n",
        "placement \"plant.home\" {\n  select = \"plant.*\"\n  home = \"edge\"\n}\n",
        "connector \"plant.influx\" {\n  kind = \"influx\"\n  node = \"edge\"\n",
        "  address = \"http://127.0.0.1:1\"\n  database = \"plant\"\n",
        "  measurement = \"telemetry\"\n  select = \"plant.missing\"\n",
        "  reader {\n    mode = \"complete\"\n  }\n}\n",
    ));
    rig.start();
    rig.apply();
    let stopped = rig.wait("`plant.influx` stops", || {
        let status = rig.status();
        (status.get("plant.influx"))
            .filter(|connector| connector.state == "stopped")
            .cloned()
            .ok_or_else(|| format!("{status:#?}"))
    });
    assert_eq!(
        stopped,
        status::Connector {
            name: "plant.influx".into(),
            kind: "influx".into(),
            state: "stopped".into(),
            address: Some("http://127.0.0.1:1".into()),
            restarts: Some(0),
            counts: Some(BTreeMap::from([("confirmed".into(), 0)])),
            error: Some(status::Error {
                class: "config".into(),
                text: concat!(
                    "the selector matches no channel. ",
                    "Select a channel that the spec defines",
                )
                .into(),
            }),
        }
    );
}

/// `status` reaches the running node, which refuses it while it places no connector.
#[test]
fn status_runs_on_the_node() {
    let mut rig = Rig::new();
    rig.start();
    let output = rig.run(&["status"], b"");
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (Some(1), "", REFUSED)
    );
}

/// `status --data` reaches the node of that data directory, and `status` with no flag
/// reaches none in `foundation-data`.
#[test]
fn status_reaches_the_node_of_data() {
    let mut rig = Rig::new();
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    let output = rig.run(&["status", "--data", "plant"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (Some(1), REFUSED)
    );
    let output = rig.run(&["status"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (
            Some(1),
            concat!(
                "error[node.not-running]: no node runs in foundation-data\n",
                "fix: Start one with `foundation start --name <name>`, or give the ",
                "data directory of a node that runs with `--data`\n",
            )
        )
    );
}

#[test]
fn mcp_answers_each_request_on_its_own_line_until_input_closes() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":"b","method":"nope"}"#,
        "\n",
    );
    let output = Rig::new().run(&["mcp"], input.as_bytes());
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (
            Some(0),
            concat!(
                r#"{"id":1,"jsonrpc":"2.0","result":{}}"#,
                "\n",
                r#"{"error":{"code":-32601,"message":"Method not found"},"id":"b","jsonrpc":"2.0"}"#,
                "\n",
            ),
            ""
        )
    );
}

#[test]
fn mcp_replies_before_the_next_request_arrives() {
    let rig = Rig::new();
    let mut mcp = rig.talk(&["mcp"]);
    let mut replies = String::new();
    for id in 1..=2 {
        let reply = format!("{{\"id\":{id},\"jsonrpc\":\"2.0\",\"result\":{{}}}}\n");
        replies.push_str(&reply);
        mcp.ask(
            &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n"),
            &replies,
        );
    }
    let output = mcp.close();
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (Some(0), replies.as_str(), "")
    );
}

#[test]
fn mcp_answers_a_line_that_is_not_utf8_and_goes_on() {
    let input = [
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"\xff\"}\n".as_slice(),
        b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
    ]
    .concat();
    let output = Rig::new().run(&["mcp"], &input);
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (
            Some(0),
            concat!(
                r#"{"error":{"code":-32700,"message":"Parse error"},"id":null,"jsonrpc":"2.0"}"#,
                "\n",
                r#"{"id":2,"jsonrpc":"2.0","result":{}}"#,
                "\n",
            ),
            ""
        )
    );
}

#[test]
fn help_lists_mcp() {
    let output = Rig::new().run(&["--help"], b"");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        text(&output.stdout)
            .lines()
            .any(|line| line.trim_start().starts_with("mcp ")),
        "{}",
        text(&output.stdout)
    );
}
