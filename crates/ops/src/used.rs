//! The spec that the node uses, which plan and apply start from.

use std::collections::BTreeMap;
use std::rc::Rc;

use spec::definition::Definition;
use types::name::Name;

use crate::error::Error;

/// The spec that a node uses, as one operation read it.
#[derive(Debug)]
pub(crate) struct Spec(mesh::used::Spec);

/// The spec that `mesh` uses.
///
/// # Errors
///
/// [`Error::Stopped`] when the group of `mesh` stopped.
pub(crate) async fn spec(mesh: &mesh::Mesh) -> Result<Spec, Error> {
    mesh.spec().await.map(Spec).map_err(Error::Stopped)
}

impl Spec {
    /// The pointer and the definitions, by tree key, of the spec.
    ///
    /// # Errors
    ///
    /// [`Error::Behind`] when the node does not use the newest spec.
    pub(crate) fn newest(
        self,
    ) -> Result<(spec::Pointer, Rc<BTreeMap<Name, Definition>>), Error> {
        let Self(spec) = self;
        if let Some(behind) = spec.behind {
            return Err(Error::Behind(Box::new(behind)));
        }
        let pointer = spec
            .pointer
            .expect("invariant: a node that uses no spec is behind its newest pointer");
        Ok((pointer, spec.definitions))
    }
}
