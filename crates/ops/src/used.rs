//! The spec that the node uses, which plan and apply start from.

use std::collections::BTreeMap;
use std::rc::Rc;

use spec::definition::Definition;
use types::name::Name;

use crate::error::Error;

/// The pointer and the definitions, by tree key, of the spec that `mesh` uses.
///
/// # Errors
///
/// - [`Error::Stopped`] when the group of `mesh` stopped.
/// - [`Error::Behind`] when the node does not use the newest spec.
pub(crate) async fn spec(
    mesh: &mesh::Mesh,
) -> Result<(spec::Pointer, Rc<BTreeMap<Name, Definition>>), Error> {
    newest(mesh.spec().await.map_err(Error::Stopped)?)
}

/// The pointer and the definitions, by tree key, of `spec`.
///
/// # Errors
///
/// [`Error::Behind`] when `spec` is not the newest spec of its node.
pub(crate) fn newest(
    spec: mesh::used::Spec,
) -> Result<(spec::Pointer, Rc<BTreeMap<Name, Definition>>), Error> {
    if let Some(behind) = spec.behind {
        return Err(Error::Behind(Box::new(behind)));
    }
    let pointer = spec
        .pointer
        .expect("invariant: a node that uses no spec is behind its newest pointer");
    Ok((pointer, spec.definitions))
}
