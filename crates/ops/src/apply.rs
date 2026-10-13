//! Applies a plan file to the spec of the region.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use types::channel;

use crate::error::{self, Error, Sources};
use crate::front_end;
use crate::plan::{self, Counts};
use crate::used;

#[cfg(test)]
mod tests;

/// Applies the plan in `bytes`, read from the plan file at `path`, to the spec of
/// `mesh`, as [`apply`] does.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` when `path` is not UTF-8.
/// - [`Error::Plan`] when `bytes` are not a plan.
/// - [`Error::Stopped`] when the group of `mesh` stopped.
/// - Each error of [`apply`].
pub(crate) async fn file(
    path: &Path,
    bytes: &[u8],
    mesh: &mesh::Mesh,
    kinds: &connector::kind::Table,
    key: impl FnMut() -> channel::Key,
) -> Result<Applied, Error> {
    let file = path.to_str().ok_or_else(|| {
        Error::config(vec![front_end::not_utf8(path)], &Sources::default())
    })?;
    let plan = config::plan::Plan::decode(bytes).map_err(Error::Plan)?;
    let effect = apply(plan, used::spec(mesh).await?, mesh, kinds, key).await?;
    Ok(Applied {
        file: file.to_owned(),
        effect,
    })
}

/// Applies `plan` to the spec of `mesh`, which uses `used`. A new channel gets its key
/// from `key`, as [`config::plan::Plan::definitions`] states. `kinds` holds the
/// connector kinds of the node that applies. The members are those of `mesh` at the
/// apply.
///
/// # Errors
///
/// - [`Error::Behind`] when the node does not use the newest spec.
/// - [`Error::Stale`] when the spec in use is not the base of the plan, or when another
///   change applies first.
/// - [`Error::Plan`] when the plan holds a change that `plan` does not make from the
///   spec at its base, such as one at a reserved label.
/// - [`Error::Config`] with each problem of [`config::plan::check`] on the definitions
///   after the plan, with no place.
/// - [`Error::Stopped`] when the group of `mesh` stopped during the apply.
/// - [`Error::Apply`] with each other error of [`mesh::Mesh::apply`].
///
/// A plan with no change and no home proposes nothing and gives the spec in use, so
/// it makes no other plan stale. No error changes the spec. Only `Apply`, `Stopped`,
/// and `Stale` when another change applies first, can come after a proposal.
pub(crate) async fn apply(
    plan: config::plan::Plan,
    used: used::Spec,
    mesh: &mesh::Mesh,
    kinds: &connector::kind::Table,
    key: impl FnMut() -> channel::Key,
) -> Result<Effect, Error> {
    let (pointer, applied) = used.newest()?;
    // `definitions` reads the definitions at the base, so the compare comes first.
    if pointer != plan.base {
        return Err(Error::Stale {
            base: plan.base,
            pointer,
        });
    }
    let definitions = plan.definitions(&applied, key).map_err(Error::Plan)?;
    let counts = Counts::of(&plan, &applied);
    let homes = plan.homes.len();
    let pointer = if plan.changes.is_empty() && plan.homes.is_empty() {
        pointer
    } else {
        config::plan::check(&definitions, &mesh.names(), kinds)
            .map_err(|diagnostics| Error::config(diagnostics, &Sources::default()))?;
        mesh.apply(plan.base, definitions, plan.homes)
            .await
            .map_err(|error| match error {
                mesh::Error::Stale { base, pointer } => Error::Stale { base, pointer },
                mesh::Error::Stopped(stopped) => Error::Stopped(stopped),
                error => Error::Apply(error),
            })?
    };
    Ok(Effect {
        pointer: plan::Pointer::from(pointer),
        counts,
        homes,
    })
}

/// What `apply` changed, for the plan file of a command line.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Applied {
    /// The plan file, as the user gave its path.
    pub(crate) file: String,
    #[serde(flatten)]
    pub(crate) effect: Effect,
}

/// What an apply changed in the region.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Effect {
    /// The spec after the apply.
    pub(crate) pointer: plan::Pointer,
    #[serde(flatten)]
    pub(crate) counts: Counts,
    /// The count of homes that the applied plan lists. An index that has a home keeps
    /// it.
    pub(crate) homes: usize,
}

impl Applied {
    /// The output as JSON.
    pub(crate) fn json(&self) -> Value {
        serde_json::to_value(self).expect("invariant: an output is plain JSON data")
    }

    /// `Applied <file>: <a> added, <c> changed, <r> removed, <h> homes listed.`, with
    /// each count of 0 left out and `1 home listed` for one home, or `no change` in
    /// place of the counts when each is 0.
    pub(crate) fn text(&self) -> String {
        let Effect { counts, homes, .. } = &self.effect;
        let counts = [
            (counts.added, "added"),
            (counts.changed, "changed"),
            (counts.removed, "removed"),
        ];
        let mut counts: Vec<String> = counts
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, action)| format!("{count} {action}"))
            .collect();
        match homes {
            0 => {}
            1 => counts.push("1 home listed".to_owned()),
            homes => counts.push(format!("{homes} homes listed")),
        }
        let counts = if counts.is_empty() {
            "no change".to_owned()
        } else {
            counts.join(", ")
        };
        format!("Applied {}: {counts}.\n", error::escape(&self.file))
    }
}
