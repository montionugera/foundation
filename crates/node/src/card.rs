//! The addresses on the card of this node.

use std::net::SocketAddr;

use mesh::card::addresses::Addresses;
use types::time::Span;

/// Makes `addresses` the addresses of the card of this node on `mesh`, the node
/// bound at `bound`. When `addresses` is empty, it first logs a warning that no peer
/// can dial the node. After [`mesh::Error::Soon`], it logs a warning and calls again a
/// minute later on `clock`. After another error but [`mesh::Error::Stopped`], it logs
/// a warning and returns.
pub(crate) async fn set(
    mesh: mesh::Mesh,
    addresses: Addresses,
    bound: SocketAddr,
    clock: env::clock::Clock,
) {
    if addresses.as_slice().is_empty() {
        tracing::warn!(
            %bound,
            "no peer can dial this node: give `--listen` an address of this host"
        );
    }
    loop {
        match mesh.set_addresses(addresses.clone()).await {
            Ok(()) | Err(mesh::Error::Stopped(_)) => return,
            Err(error @ mesh::Error::Soon) => {
                tracing::warn!(
                    %error,
                    "the card of this node keeps its addresses, and the node tries \
                     again in a minute"
                );
                clock.sleep(Span::MINUTE).await;
            }
            Err(error) => {
                tracing::warn!(%error, "the card of this node keeps its addresses");
                return;
            }
        }
    }
}
