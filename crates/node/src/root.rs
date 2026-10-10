//! The region of a data directory, which at the first start is the root region that the
//! node founds alone.

use std::collections::BTreeMap;
use std::rc::Rc;

use mesh::card::addresses::Addresses;
use mesh::card::{self, Card};
use mesh::region::Founding;
use mesh::status::Status;
use types::name::Prefix;

use crate::identity::Identity;
use crate::{Error, admin, directory};

/// The founding in the mesh directory of `files`. When it holds none, the root region
/// that the node of `identity` founds alone: its card has `name`, the public halves of
/// the node's two private keys, and `addresses`; it is the one voter; and the spec
/// holds the first admin, whose key [`admin::load`] reads or makes with `entropy`.
///
/// # Errors
///
/// [`Error::Mesh`] when the founding in the mesh directory does not read, and the
/// errors of [`admin::load`].
pub(crate) async fn founding(
    files: &env::files::Files,
    pool: Rc<block::Pool>,
    entropy: &env::entropy::Entropy,
    identity: &Identity,
    name: types::name::Name,
    addresses: Addresses,
) -> Result<Founding, Error> {
    let stored = mesh::founding(files, &directory::mesh(), pool).await;
    if let Some(founding) = stored.map_err(Error::Mesh)? {
        return Ok(founding);
    }
    let admin = admin::load(files, entropy).await?;
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
        status: Status::new(BTreeMap::new()).expect("invariant: no status is not many"),
    };
    Ok(Founding {
        prefix: Prefix::ROOT,
        members: vec![member],
        voters: [identity.key].into(),
        definitions: spec::founding::create(admin.public()),
        homes: BTreeMap::new(),
    })
}

/// The addresses of `bound` that a card holds: each but a scoped IPv6 address, which
/// means something only on the node.
pub(crate) fn addresses(bound: Vec<transport::Address>) -> Addresses {
    let held = bound
        .into_iter()
        .filter(|&address| Addresses::new(vec![address]).is_ok())
        .collect();
    Addresses::new(held)
        .expect("invariant: a port has fewer addresses than a card holds")
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

    use transport::Address;

    use super::*;

    #[test]
    fn a_card_holds_each_bound_address_but_a_scoped_one() {
        let v4 = Address::Udp(SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 7000)));
        let v6 = Address::Udp(SocketAddr::from((Ipv6Addr::LOCALHOST, 7000)));
        let link = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
        let scoped = Address::Udp(SocketAddrV6::new(link, 7000, 0, 2).into());
        let held = addresses(vec![v4, scoped, v6]);
        assert_eq!(held, Addresses::new(vec![v4, v6]).unwrap());
    }
}
