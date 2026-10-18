//! Production-path tests of `mesh` through its public surface.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "../common/steps.rs"]
mod steps;
mod surface;
