//! Admits a node that asks a voter to join by a ticket.

use types::ed25519::PublicKey;

use super::{Mesh, Unstamped};
use crate::error::Error;
use crate::refusal::Refusal;
use crate::region::Request;

impl Mesh {
    // Admits the node of `request`, from a peer that proved the key `peer`, by the
    // mesh time of `time`. `Ok(Ok(()))` once this node applied the join, also when it
    // applied it before the call. `Spoofed` when `peer` is not the key of the card.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the join stream of #336 is the first user")
    )]
    pub(super) async fn admit(
        &self,
        peer: PublicKey,
        time: &clock::Reader,
        request: Request,
    ) -> Result<Result<(), Refusal>, Error> {
        let card = &request.card;
        if card.card.public_key != peer {
            return Err(Error::Spoofed { from: card.key });
        }
        match self.check_proposer() {
            Ok(()) => {}
            Err(Error::NoVote) => return Ok(Err(Refusal::NoVote)),
            Err(Error::Stopped(_)) => return Ok(Err(Refusal::Stopped)),
            Err(error) => return Err(error),
        }
        if self.admitted(&request) {
            return Ok(Ok(()));
        }
        let change = match self.stamp(time, request.clone()) {
            Ok(change) => change,
            Err(Unstamped::Unsynced) => return Ok(Err(Refusal::Unsynced)),
            Err(Unstamped::Stopped(_)) => return Ok(Err(Refusal::Stopped)),
            Err(Unstamped::Refused(refused)) => {
                return Ok(Err(Refusal::Refused(refused)));
            }
            Err(Unstamped::Status(_)) => {
                unreachable!("invariant: a decoded request has at most 64 names")
            }
        };
        // Each try proposes the same join. An entry of an earlier try can apply after
        // the try gave up on its answer, and then this one is a duplicate.
        loop {
            let settled = match self.attempt() {
                Ok(attempt) => attempt.settle(change.clone()).await,
                Err(error) => Err(error),
            };
            match settled {
                Ok(Some(Err(_))) if self.admitted(&request) => return Ok(Ok(())),
                Ok(Some(applied)) => return Ok(applied.map_err(Refusal::Refused)),
                Ok(None) => {}
                Err(Error::Contested { .. }) => return Ok(Err(Refusal::Contested)),
                Err(Error::NoVote) => return Ok(Err(Refusal::NoVote)),
                Err(Error::Stopped(_)) => return Ok(Err(Refusal::Stopped)),
                Err(error) => return Err(error),
            }
        }
    }

    // Whether this node applied the join of `request`: a member holds its node key
    // with its public key and its admission.
    fn admitted(&self, request: &Request) -> bool {
        let group = self.group.borrow();
        group.state.member(request.card.key).is_some_and(|member| {
            member.card.card().public_key == request.card.card.public_key
                && member.admission == request.admission
        })
    }
}
