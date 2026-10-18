//! The bound on the `Card` changes that a leader proposes: at most one of each
//! member each minute.

use std::collections::BTreeMap;

use raft::{Position, Term};
use types::node;
use types::time::{Monotonic, Span};

use crate::error::Error;

/// The least time between two `Card` changes of one member that a leader proposes.
const SPAN: Span = Span::MINUTE;

/// The last `Card` change of each member that this node proposed less than
/// [`SPAN`] before.
#[derive(Debug, Default)]
pub(super) struct Bound {
    // By member: the version of the card, when it was proposed, and its entry.
    proposed: BTreeMap<node::Key, (u64, Monotonic, Position)>,
}

impl Bound {
    /// Checks a card of `member` at `version` at `now`, while this node leads in
    /// `term`. Gives the entry of a repeat: the card that this node proposed in
    /// `term`, which gets no new entry. Gives `None` when this node may propose it,
    /// and then [`proposed`](Self::proposed) counts it.
    ///
    /// # Errors
    ///
    /// [`Error::Soon`] when this node proposed a card of `member` at another version
    /// less than a minute before.
    pub(super) fn check(
        &mut self,
        member: node::Key,
        version: u64,
        term: Term,
        now: Monotonic,
    ) -> Result<Option<Position>, Error> {
        self.proposed.retain(|_, &mut (_, at, _)| recent(at, now));
        match self.proposed.get(&member) {
            None => Ok(None),
            Some(&(held, _, at)) if held == version => {
                Ok((at.term == term).then_some(at))
            }
            Some(_) => Err(Error::Soon),
        }
    }

    /// Counts the card of `member` at `version` that this node proposed at `now`, at
    /// the entry `at`.
    pub(super) fn proposed(
        &mut self,
        member: node::Key,
        version: u64,
        at: Position,
        now: Monotonic,
    ) {
        self.proposed.insert(member, (version, now, at));
    }
}

/// Whether a `Card` change proposed at `at` holds back another of its member at `now`.
fn recent(at: Monotonic, now: Monotonic) -> bool {
    at.checked_add(SPAN).is_none_or(|end| now < end)
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_repeat_in_the_term_of_its_entry_gets_that_entry() {
        let mut bound = Bound::default();
        let at = Monotonic(5);
        assert_eq!(bound.check(member(), 2, Term(3), at), Ok(None));
        bound.proposed(member(), 2, entry(3, 7), at);
        let later = Monotonic(9);
        assert_eq!(
            bound.check(member(), 2, Term(3), later),
            Ok(Some(entry(3, 7)))
        );
        assert_eq!(bound.check(member(), 3, Term(3), later), Err(Error::Soon));
        let other = common::key(3);
        assert_eq!(bound.check(other, 3, Term(3), later), Ok(None));
    }

    // The entry of an earlier term can be gone from the log, so the repeat proposes
    // again.
    #[test]
    fn a_repeat_in_a_later_term_proposes_again() {
        let mut bound = Bound::default();
        bound.proposed(member(), 2, entry(3, 7), Monotonic(5));
        assert_eq!(bound.check(member(), 2, Term(4), Monotonic(9)), Ok(None));
        assert_eq!(
            bound.check(member(), 3, Term(4), Monotonic(9)),
            Err(Error::Soon)
        );
    }

    #[test]
    fn a_card_after_the_minute_may_propose() {
        let mut bound = Bound::default();
        bound.proposed(member(), 2, entry(3, 7), Monotonic(5));
        let minute = Monotonic(60_000_000_005);
        assert_eq!(bound.check(member(), 3, Term(3), minute), Ok(None));
        assert_eq!(bound.check(member(), 2, Term(3), minute), Ok(None));
    }
}
