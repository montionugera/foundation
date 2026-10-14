//! The tests that bound the heap of `serve` with one count of the bytes it holds. The
//! count covers each thread, so this binary has no test harness. The sim runs on one
//! thread, so the count is exact.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::sync::Arc;

use ops::{Config, FrontEnd, FrontEnds, Node, serve};
use serde_json::{Value, json};
use sim::Sim;
use types::channel::Key;
use wire::hub::client::BODY_BYTES_MAX;

#[path = "../common/founding.rs"]
mod founding;

use founding::{ADMIN, create_mesh};

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The bytes of the path of the file of each request. Each problem or change of a reply
/// holds it, and the heap holds it once for each request.
const PATH_BYTES: usize = 64 << 10;
/// The problems or changes of each reply: with the path, 8 times the bound.
const ENTRIES: usize = 2_000;
/// The bytes of the request and of its steps, apart from the reply, that the heap can
/// hold at once. The steps took at most 3.9 MB on the laptop (Apple M3 Max).
const STEPS_BYTES: usize = 8 << 20;

fn main() {
    a_reply_of_many_problems_holds_no_more_than_the_bound();
    a_reply_of_many_changes_holds_no_more_than_the_bound();
}

fn a_reply_of_many_problems_holds_no_more_than_the_bound() {
    let text = "nope \"a\" {}\n".repeat(ENTRIES);
    serve_large(&text);
}

fn a_reply_of_many_changes_holds_no_more_than_the_bound() {
    let placement = "placement \"p\" {\n  select = \"site.*\"\n  home = \"edge\"\n}\n";
    let channels =
        (0..ENTRIES).map(|n| format!("channel \"site.c{n}\" {{ kind = \"index\" }}\n"));
    let text = std::iter::once(placement.to_owned())
        .chain(channels)
        .collect::<String>();
    serve_large(&text);
}

/// Serves a `plan` request of `text` at a long path, whose reply passes the bound, and
/// asserts that the heap grows by at most the bound and `STEPS_BYTES`.
fn serve_large(text: &str) {
    let path = format!("{}.hcl", "x".repeat(PATH_BYTES - 4));
    let body = format!(
        "{}\n",
        json!({ "plan": { "files": [{ "path": path, "text": text }] } })
    )
    .into_bytes();
    let mut sim = Sim::new(sim::Config::default());
    let sim_node = sim.node(sim::node::Config::default());
    let ran = sim.run_on(&sim_node, move |sim_node, tasks| async move {
        let definitions = spec::founding::create(ADMIN.public());
        let mesh = create_mesh(&sim_node, &tasks, &["edge"], definitions).await;
        let env = hub::testing::Env {
            files: sim_node.files(),
            clock: sim_node.clock(),
            wall: sim_node.wall(),
            entropy: sim_node.entropy(),
            tasks,
        };
        let (hub, _) = hub::testing::open(env).await;
        let front_ends = FrontEnds::new(
            "hcl",
            FrontEnd {
                read: config_hcl::read,
            },
        );
        let node = Node::new(Config {
            mesh,
            key: Box::new(|| Key::from_u128(1)),
            front_ends,
            kinds: Arc::new(connector::kind::Table::new()),
            hub,
            clock: sim_node.clock(),
            name: "edge".parse().expect("a name"),
            secrets: connector::testing::create_secrets(sim_node.files()),
        });
        let subject = "@admin".parse().expect("a name");
        let before = ALLOCATOR.held();
        ALLOCATOR.reset_peak();
        let reply = serve(&node, &subject, &body).await;
        let grown = ALLOCATOR.peak() - before;
        let bound = usize::try_from(BODY_BYTES_MAX).expect("the bound fits");
        assert!(
            grown <= bound + STEPS_BYTES,
            "a reply of at most {bound} bytes grew the heap by {grown}",
        );
        let end = reply
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("a line");
        let head: Value = serde_json::from_slice(&reply[..end]).expect("JSON");
        assert_eq!(head["errors"][0]["code"], "ops.large-reply", "{head}");
    });
    assert_eq!(ran, Ok(()), "the run ends");
}
