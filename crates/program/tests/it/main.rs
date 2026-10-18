//! Tests of `program::Session` against a simulated node that serves it on one
//! `hub::Link`.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "../../../hub/tests/common/agent.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod agent;
#[path = "../common/connect.rs"]
mod connect;
#[path = "../common/hand.rs"]
mod hand;
#[path = "../common/home.rs"]
mod home;
#[path = "../../../hub/tests/common/net.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod net;
#[path = "../../../hub/tests/common/node.rs"]
mod node;
mod read;
mod session;
#[path = "../../../hub/tests/common/sessions.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod sessions;
