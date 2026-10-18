//! The bound on the `Card` changes that a leader proposes: at most one of each
//! member each minute.

use std::collections::BTreeMap;
use std::rc::Rc;

use raft::Term;
use types::node;
use types::time::{Monotonic, Span};

use super::Proposal;
use crate::error::Error;

/// The least time between two `Card` changes of one member that a leader proposes.
const SPAN: Span = Span::MINUTE;

/// The last `Card` change of each member that this node proposed less than
/// [`SPAN`] before.
#[derive(Default)]
pub(super) struct Bound {
    // By member: the version of the card, when it was proposed, and its proposal.
    proposed: BTreeMap<node::Key, (u64, Monotonic, Rc<Proposal>)>,
}

impl Bound {
    /// The proposal of a card of `member` at `version` at `now`, while this node
    /// leads in `term`. A repeat of the card that this node proposed in `term` gets
    /// that proposal and no new entry. Else `propose` gives it, and the bound counts
    /// it.
    ///
    /// # Errors
    ///
    /// - [`Error::Soon`] when this node proposed a card of `member` at another
    ///   version less than a minute before.
    /// - Each error of `propose`.
    pub(super) fn propose(
        &mut self,
        member: node::Key,
        version: u64,
        term: Term,
        now: Monotonic,
        propose: impl FnOnce() -> Result<Rc<Proposal>, Error>,
    ) -> Result<Rc<Proposal>, Error> {
        self.proposed.retain(|_, &mut (_, at, _)| recent(at, now));
        match self.proposed.get(&member) {
            Some((held, _, proposal))
                if *held == version && proposal.at.term == term =>
            {
                return Ok(Rc::clone(proposal));
            }
            Some((held, _, _)) if *held != version => return Err(Error::Soon),
            _ => {}
        }
        let proposal = propose()?;
        self.proposed
            .insert(member, (version, now, Rc::clone(&proposal)));
        Ok(proposal)
    }
}

/// Whether a `Card` change proposed at `at` holds back another of its member at `now`.
fn recent(at: Monotonic, now: Monotonic) -> bool {
    at.checked_add(SPAN).is_none_or(|end| now < end)
}

#[cfg(test)]
mod tests {
    use raft::Position;

    use super::*;
    use crate::common;

    fn member() -> node::Key {
        common::key(2)
    }

    fn entry(term: u64, index: u64) -> Position {
        Position {
            term: Term(term),
            index,
        }
    }

    #[test]
    fn a_card_change_holds_back_another_for_one_minute() {
        let at = Monotonic(5);
        assert!(recent(at, at));
        assert!(recent(at, Monotonic(60_000_000_004)));
        assert!(!recent(at, Monotonic(60_000_000_005)));
        assert!(recent(Monotonic(u64::MAX), Monotonic(u64::MAX)));
    }

    // What `bound` gives for a card of `member` at `version` in `term` at `now`,
    // with `at` as the entry of a new proposal: the entry, and whether it is new.
    fn propose(
        bound: &mut Bound,
        member: node::Key,
        version: u64,
        term: u64,
        now: u64,
        at: Position,
    ) -> Result<(Position, bool), Error> {
        let mut new = false;
        let proposal =
            bound.propose(member, version, Term(term), Monotonic(now), || {
                new = true;
                Ok(Rc::new(Proposal::new(at)))
            })?;
        Ok((proposal.at, new))
    }

    #[test]
    fn a_repeat_in_the_term_of_its_entry_gets_that_entry() {
        let mut bound = Bound::default();
        let first = propose(&mut bound, member(), 2, 3, 5, entry(3, 7));
        assert_eq!(first, Ok((entry(3, 7), true)));
        let repeat = propose(&mut bound, member(), 2, 3, 9, entry(3, 8));
        assert_eq!(repeat, Ok((entry(3, 7), false)));
        let other = propose(&mut bound, member(), 3, 3, 9, entry(3, 8));
        assert_eq!(other, Err(Error::Soon));
        let key = common::key(3);
        let next = propose(&mut bound, key, 3, 3, 9, entry(3, 8));
        assert_eq!(next, Ok((entry(3, 8), true)));
    }

    // The entry of an earlier term can be gone from the log, so the repeat proposes
    // again.
    #[test]
    fn a_repeat_in_a_later_term_proposes_again() {
        let mut bound = Bound::default();
        propose(&mut bound, member(), 2, 3, 5, entry(3, 7)).unwrap();
        let repeat = propose(&mut bound, member(), 2, 4, 9, entry(4, 8));
        assert_eq!(repeat, Ok((entry(4, 8), true)));
        let other = propose(&mut bound, member(), 3, 4, 9, entry(4, 9));
        assert_eq!(other, Err(Error::Soon));
    }

    #[test]
    fn a_card_after_the_minute_may_propose() {
        let minute = 60_000_000_005;
        for version in [2, 3] {
            let mut bound = Bound::default();
            propose(&mut bound, member(), 2, 3, 5, entry(3, 7)).unwrap();
            let after = propose(&mut bound, member(), version, 3, minute, entry(3, 8));
            assert_eq!(after, Ok((entry(3, 8), true)));
        }
    }

    #[test]
    fn a_proposal_that_fails_holds_back_nothing() {
        let mut bound = Bound::default();
        let failed = bound.propose(member(), 2, Term(3), Monotonic(5), || {
            Err(Error::NotMember(member()))
        });
        assert_eq!(failed.err(), Some(Error::NotMember(member())));
        let next = propose(&mut bound, member(), 3, 3, 9, entry(3, 8));
        assert_eq!(next, Ok((entry(3, 8), true)));
    }
}
