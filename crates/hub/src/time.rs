use std::cell::RefCell;
use std::rc::Rc;

use types::time::Stamp;

use crate::State;

/// Mesh time of a hub, which a caller can keep. Its `now` never goes back, and two
/// calls can give the same stamp.
#[derive(Clone, Debug)]
pub struct Time(pub(crate) Rc<RefCell<State>>);

impl Time {
    /// As [`Writer::now`](crate::writer::Writer::now).
    #[must_use]
    pub fn now(&self) -> Stamp {
        now(&self.0)
    }
}

/// Waits for the node's first mesh time. Each mesh time reaches the first stamp.
pub(crate) async fn known(state: &RefCell<State>) {
    let first = state.borrow().time.reach(Stamp::from_nanos(i64::MIN));
    first.await;
}

pub(crate) fn now(state: &RefCell<State>) -> Stamp {
    let now = state.borrow().home.now();
    now.expect("invariant: mesh time stays once known")
}
