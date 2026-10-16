//! Defines the kind contract (parse, check, discover, run), the thin supervisor, `ctx`,
//! the component library, and the compositions.

pub mod browse;
pub mod cancel;
pub mod endpoint;
pub mod http;
pub mod kind;
pub mod pace;
pub mod reader;
pub mod retry;
pub mod stamp;
/// The types of a secret that a kind reads through [`kind::Context::secret`].
pub mod secret {
    pub use ::secret::{Source, Value};
}
pub mod status;
pub mod supervisor;
#[cfg(any(test, feature = "sim"))]
pub mod testing;

#[cfg(test)]
#[cfg(not(loom))]
mod common;
mod deadline;
