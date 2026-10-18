//! The root region that a node founds alone at a start whose data directory holds no
//! founding.

use std::collections::BTreeMap;

use mesh::card::addresses::Addresses;
use mesh::card::{self, Card};
use mesh::region::Founding;
use mesh::status::Status;
use types::name::Prefix;

use crate::identity::Identity;
use crate::{Error, admin, channel_key, status};

/// The root region that the node of `identity` founds alone: its card has `name`, the
/// public halves of the node's two private keys, and `addresses`; it is the one voter;
/// and the spec holds the first admin, whose key [`admin::load`] reads from `files` or
/// makes with `entropy`. The member's status holds a key for each name of
/// [`status::names`], which [`channel_key`] makes at the mesh time of `time`.
///
/// # Errors
///
/// The errors of [`admin::load`].
pub(crate) async fn founding(
    files: &env::files::Files,
    time: &clock::Reader,
    entropy: &env::entropy::Entropy,
    identity: &Identity,
    name: types::name::Name,
    addresses: Addresses,
) -> Result<Founding, Error> {
    let admin = admin::load(files, entropy).await?;
    let key = |name| (name, channel_key(time, entropy));
    let keys = status::names().into_iter().map(key).collect();
    let card = Card {
        name,
        public_key: identity.private_key.public(),
        seal_key: identity.opener.public(),
        addresses,
        version: 1,
    };
    let member = mesh::Member {
        card: card::Signed::sign(identity.key, card, &identity.private_key),
        admission: [0; 64],
        ephemeral: None,
        status: Status::new(keys).expect("invariant: the status table is not many"),
    };
    Ok(Founding {
        prefix: Prefix::ROOT,
        members: vec![member],
        voters: [identity.key].into(),
        definitions: spec::founding::create(admin.public()),
        homes: BTreeMap::new(),
    })
}
