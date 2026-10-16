//! The tests of the seqs that a `program::Reader` lost, against a simulated node.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "../../../hub/tests/common/agent.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod agent;
#[path = "../common/connect.rs"]
mod connect;
mod gaps;
#[path = "../common/hand.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod hand;
#[path = "../common/home.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod home;
#[path = "../../../hub/tests/common/net.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod net;
#[path = "../../../hub/tests/common/node.rs"]
mod node;
#[path = "../../../hub/tests/common/sessions.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod sessions;
