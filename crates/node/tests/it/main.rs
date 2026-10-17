//! Runs the built `foundation` binary as a process.

#![cfg(test)]
// The simulated servers need `os::net()`, which a `--cfg loom` build lacks.
#![cfg(not(loom))]

mod common;
mod log;
mod name;
mod one_node;
mod port;
mod quickstart;
mod rig;
#[cfg(unix)]
mod secret;
#[cfg(unix)]
mod start;
mod status;

use std::collections::BTreeMap;

use rig::Rig;

/// The output of `status` on a node that places no connector.
const EMPTY: &str = "You can read no connector on this node.\n";

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8")
}

/// The log of a node at the start of `stderr`, and the lines after it. `foundation
/// start` prints its own lines once `Node::join` logs `stopped`, the last line of the
/// log.
fn logged(stderr: &[u8]) -> (&str, &str) {
    let text = text(stderr);
    let log = (text.split_inclusive('\n'))
        .take_while(|line| {
            let level = after_stamp(line);
            line.starts_with("{\"timestamp\":")
                || level.is_some_and(|level| {
                    ["ERROR ", "WARN ", "INFO ", "DEBUG ", "TRACE "]
                        .iter()
                        .any(|name| level.starts_with(name))
                })
        })
        .map(str::len)
        .sum();
    text.split_at(log)
}

/// The text log line `line` after its stamp and the space, or `None` when it starts
/// with no stamp.
fn after_stamp(line: &str) -> Option<&str> {
    line.get(26..)?.strip_prefix("Z ")
}

/// The commit of `HEAD` that `git` reads here, or `None` when it fails.
fn head() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| text(&output.stdout).trim().to_owned())
}

#[test]
fn an_operation_prints_its_output_and_exits_0() {
    let commit = head();
    let short = commit.as_deref().map_or("unknown", |commit| &commit[..12]);
    let line = format!("{} ({short})\n", env!("CARGO_PKG_VERSION"));
    for args in [&["version"][..], &["--version"]] {
        let output = Rig::new().run(args, b"");
        assert_eq!(
            (
                output.status.code(),
                text(&output.stdout),
                text(&output.stderr)
            ),
            (Some(0), line.as_str(), ""),
            "{args:?}"
        );
    }
    let output = Rig::new().run(&["version", "--json"], b"");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["commit"].as_str(), commit.as_deref());
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
            counts: Some(BTreeMap::from([
                ("confirmed".into(), 0),
                ("lost".into(), 0),
                ("oversized".into(), 0)
            ])),
            error: Some(status::Error {
                class: "config".into(),
                text: concat!(
                    "the selector matches no channel. ",
                    "Select a channel that the spec defines",
                )
                .into(),
            }),
            note: None,
        }
    );
}

/// `status` reaches the running node, which places no connector.
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
        (Some(0), EMPTY, "")
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
        (output.status.code(), text(&output.stdout)),
        (Some(0), EMPTY)
    );
    let output = rig.run(&["status"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (Some(1), not_running("foundation-data").as_str())
    );
    rig.stop();
    let output = rig.run(&["status", "--data", "plant"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (Some(1), not_running("plant").as_str())
    );
}

#[test]
fn status_on_a_file_as_data_is_not_running() {
    let rig = Rig::new();
    rig.config("");
    let output = rig.run(&["status", "--data", "plant.hcl"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (
            Some(1),
            concat!(
                "error[node.not-running]: no node runs in plant.hcl, which is not a data ",
                "directory\nfix: Give `--data` the data directory of a node that runs\n",
            )
        )
    );
}

#[test]
fn status_on_a_directory_whose_data_is_a_file_is_not_running() {
    let rig = Rig::new();
    std::fs::create_dir(rig.dir.join("plant")).expect("make plant");
    std::fs::write(rig.dir.join("plant/data"), b"").expect("write plant/data");
    let output = rig.run(&["status", "--data", "plant"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (
            Some(1),
            concat!(
                "error[node.not-running]: no node runs in plant, which is not a data ",
                "directory\nfix: Give `--data` the data directory of a node that runs\n",
            )
        )
    );
}

#[test]
fn the_start_in_the_fix_of_a_data_directory_runs_as_printed() {
    let rig = Rig::new();
    let shell = |command: String| {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&rig.dir)
            .output()
            .expect("run sh");
        (
            text(&output.stdout).to_owned(),
            text(&output.stderr).to_owned(),
        )
    };
    for (dir, named) in [
        ("my plant", "my plant"),
        ("-x", "-x"),
        ("it's", "it's"),
        ("=plant", "=plant"),
    ] {
        let data = format!("--data={dir}");
        let output = rig.run(&["status", data.as_str()], b"");
        let errors = text(&output.stderr).to_owned();
        let start = (errors.split('`'))
            .find(|part| part.starts_with("foundation start --data="))
            .unwrap_or_else(|| panic!("no start in {errors}"));
        let words = shell(start.replacen("foundation start", "printf '%s|'", 1));
        assert_eq!(words.0, format!("--data={named}|"), "the fix of {errors}");
        let parsed = start.replacen("foundation", env!("CARGO_BIN_EXE_foundation"), 1);
        let parsed = shell(format!("{parsed} --unknown"));
        assert_eq!(
            parsed.1.lines().next(),
            Some("error[ops.argument]: unexpected argument found: `--unknown`"),
            "the fix of {errors}"
        );
    }
}

#[test]
fn the_fix_of_a_data_directory_that_it_cannot_print_names_no_path() {
    let rig = Rig::new();
    let output = rig.run(&["status", "--data", "a\\b"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (
            Some(1),
            concat!(
                "error[node.not-running]: no node runs in a\\\\b\nfix: Run `foundation ",
                "start` with `--data` set to that directory, with `--name <name>` on its ",
                "first start, or give `--data` the data directory of a node that runs\n",
            )
        )
    );
}

#[test]
fn status_on_a_data_directory_with_a_space_quotes_it_in_the_fix() {
    let rig = Rig::new();
    let output = rig.run(&["status", "--data", "my plant"], b"");
    assert_eq!(
        (output.status.code(), text(&output.stderr)),
        (
            Some(1),
            concat!(
                "error[node.not-running]: no node runs in my plant\nfix: Run `foundation ",
                "start --data='my plant'`, with `--name <name>` on its first start, or ",
                "give `--data` the data directory of a node that runs\n",
            )
        )
    );
}

/// The error of `status` on the data directory `dir`, where no node runs.
fn not_running(dir: &str) -> String {
    format!(
        "error[node.not-running]: no node runs in {dir}\nfix: Run `foundation start \
         --data={dir}`, with `--name <name>` on its first start, or give `--data` the \
         data directory of a node that runs\n"
    )
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

/// The line of the reply of `foundation mcp` to a `status` call with `id`, which
/// gives `output`, else fails with `error`.
fn status_reply(
    id: u64,
    result: Result<serde_json::Value, serde_json::Value>,
) -> String {
    let result = match result {
        Ok(output) => serde_json::json!({
            "content": [{ "type": "text", "text": output.to_string() }],
            "structuredContent": output,
        }),
        Err(error) => {
            let errors = serde_json::json!({ "errors": [error] });
            serde_json::json!({
                "content": [{ "type": "text", "text": errors.to_string() }],
                "isError": true,
            })
        }
    };
    let reply = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    format!("{reply}\n")
}

/// The MCP error of a call whose kept session the node closed.
fn lost() -> serde_json::Value {
    serde_json::json!({
        "code": "node.no-reply",
        "message": "the node gave no reply: the session failed: the peer closed the \
                    session (0)",
        "fix": "See that the node runs, then run the command again. After `apply`, plan \
                again to see whether the change applied",
        "notes": [],
    })
}

/// The MCP error of a call that finds no node on `plant`.
fn unreached() -> serde_json::Value {
    let fix = "Run `foundation start --data=plant`, with `--name <name>` on its first \
               start, or give `--data` the data directory of a node that runs";
    serde_json::json!({ "code": "node.not-running", "message": "no node runs in plant",
        "fix": fix, "notes": [] })
}

/// One `foundation mcp` runs `status` on the node of its `--data`, and reaches it
/// again after the node stops and starts.
#[test]
fn mcp_shows_status_of_the_node_of_data_across_a_restart() {
    let mut rig = Rig::new();
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    let mut mcp = rig.talk(&["mcp", "--data", "plant"]);
    let status = |id| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"status\"}}}}\n"
        )
    };
    let empty = serde_json::json!({ "connectors": [] });
    let mut replies = status_reply(1, Ok(empty.clone()));
    mcp.ask(&status(1), &replies);
    rig.stop();
    // The kept session fails first, and the next call reaches again.
    replies.push_str(&status_reply(2, Err(lost())));
    mcp.ask(&status(2), &replies);
    rig.start_with(&["--data", "plant"]);
    replies.push_str(&status_reply(3, Ok(empty)));
    mcp.ask(&status(3), &replies);
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

/// `foundation mcp` makes its data directory, and a first `foundation start` on that
/// empty directory runs a node there.
#[test]
fn start_runs_on_the_data_directory_that_mcp_made() {
    let mut rig = Rig::new();
    let mut mcp = rig.talk(&["mcp", "--data", "plant"]);
    let status = |id| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"status\"}}}}\n"
        )
    };
    let mut replies = status_reply(1, Err(unreached()));
    mcp.ask(&status(1), &replies);
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    let empty = serde_json::json!({ "connectors": [] });
    replies.push_str(&status_reply(2, Ok(empty)));
    mcp.ask(&status(2), &replies);
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

/// `foundation mcp` keeps its session through a pause past the idle timeout of the
/// node, 30 s: its shard sends the keep-alive while it waits for a line.
#[test]
fn mcp_keeps_its_session_through_a_pause() {
    let mut rig = Rig::new();
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    let mut mcp = rig.talk(&["mcp", "--data", "plant"]);
    let status = |id| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"status\"}}}}\n"
        )
    };
    let empty = serde_json::json!({ "connectors": [] });
    let mut replies = status_reply(1, Ok(empty.clone()));
    mcp.ask(&status(1), &replies);
    mcp.idle(types::time::Span::from_nanos(
        45 * types::time::Span::SECOND.nanos(),
    ));
    replies.push_str(&status_reply(2, Ok(empty)));
    mcp.ask(&status(2), &replies);
    assert_eq!(mcp.close().status.code(), Some(0));
}

/// `foundation mcp` keeps no memory for each call that finds no node.
#[test]
fn mcp_holds_no_memory_for_each_failed_reach() {
    let mut rig = Rig::new();
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    rig.stop();
    let mut mcp = rig.talk(&["mcp", "--data", "plant"]);
    let status = |id| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"status\"}}}}\n"
        )
    };
    let kib = |pid: u32| -> u64 {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        text(&output.stdout).trim().parse().expect("a number")
    };
    let mut replies = String::new();
    let mut before = 0;
    for id in 1..=2020 {
        replies.push_str(&status_reply(id, Err(unreached())));
        mcp.ask(&status(id), &replies);
        if id == 20 {
            before = kib(mcp.pid());
        }
    }
    let after = kib(mcp.pid());
    // Each failed reach held 16 KiB, so 2000 held 32 MiB.
    assert!(after < before + 4 * 1024, "{before} KiB, then {after} KiB");
    assert_eq!(mcp.close().status.code(), Some(0));
}

/// An idle `foundation mcp` holds the UDP sockets of one session after many calls, not
/// one for each call.
#[test]
fn mcp_holds_the_sockets_of_one_session_after_many_calls() {
    let mut rig = Rig::new();
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    let mut mcp = rig.talk(&["mcp", "--data", "plant"]);
    let status = |id| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"status\"}}}}\n"
        )
    };
    let empty = serde_json::json!({ "connectors": [] });
    let sockets = |pid: u32| {
        let output = std::process::Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-i", "UDP", "-F", "f"])
            .output()
            .expect("run lsof");
        text(&output.stdout)
            .lines()
            .filter(|l| l.starts_with('f'))
            .count()
    };
    let mut replies = status_reply(1, Ok(empty.clone()));
    mcp.ask(&status(1), &replies);
    let one = sockets(mcp.pid());
    for id in 2..=20 {
        replies.push_str(&status_reply(id, Ok(empty.clone())));
        mcp.ask(&status(id), &replies);
    }
    assert_eq!(sockets(mcp.pid()), one);
    assert_eq!(mcp.close().status.code(), Some(0));
}

/// The user removes the data directory and starts a new node on it while one
/// `foundation mcp` runs. The kept session fails first, and the next call reaches the
/// new node.
#[test]
fn mcp_reaches_a_node_on_a_data_directory_made_again() {
    let mut rig = Rig::new();
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    let mut mcp = rig.talk(&["mcp", "--data", "plant"]);
    let status = |id| {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"status\"}}}}\n"
        )
    };
    let empty = serde_json::json!({ "connectors": [] });
    let mut replies = status_reply(1, Ok(empty.clone()));
    mcp.ask(&status(1), &replies);
    rig.stop();
    std::fs::remove_dir_all(rig.dir.join("plant")).expect("a remove");
    rig.start_with(&["--name", "edge", "--data", "plant"]);
    replies.push_str(&status_reply(2, Err(lost())));
    mcp.ask(&status(2), &replies);
    replies.push_str(&status_reply(3, Ok(empty)));
    mcp.ask(&status(3), &replies);
    let output = mcp.close();
    assert_eq!(output.status.code(), Some(0));
}

/// A client starts `foundation mcp` at `/`, as the decision says one may, as a user who
/// cannot write `/`.
#[test]
fn mcp_at_the_root_names_files_and_makes_nothing() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_foundation"))
        .arg("mcp")
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("a run");
    let errors = text(&output.stderr);
    assert_eq!(
        (
            output.status.code(),
            errors.starts_with("error[ops.wide-folder]")
        ),
        (Some(2), true),
        "{errors}"
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
