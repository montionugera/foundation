//! Admits a node that asks a voter to join by a ticket.

use std::cell::RefCell;
use std::future::poll_fn;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::task::{Poll, Waker};

use env::files::{Files, Mode};
use types::ed25519::PublicKey;

use super::founding::{self, Stored};
use super::{Mesh, Unstamped};
use crate::error::Error;
use crate::file::Blocks;
use crate::refusal::Refusal;
use crate::region::Request;

impl Mesh {
    // Admits the node of `request`, from a peer that proved the key `peer`.
    // `Ok(Ok(()))` once this node applied the join, also when it applied it before the
    // call. `Spoofed` when `peer` is not the key of the card.
    pub(super) async fn admit(
        &self,
        peer: PublicKey,
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
        let change = match self.stamp(request.clone()) {
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

/// The founding file that join answers send. An idle voter holds no copy: each answer
/// holds one, and answers at one time share it.
pub(super) struct Founding {
    files: Files,
    dir: PathBuf,
    blocks: Blocks,
    turn: RefCell<Turn>,
}

#[derive(Default)]
struct Turn {
    copy: Weak<Stored>,
    reading: bool,
    waiting: Vec<Waker>,
}

impl Founding {
    /// The founding file of `dir`, which an open of the mesh checked, read with
    /// `blocks`.
    pub(super) fn new(files: Files, dir: PathBuf, blocks: Blocks) -> Self {
        Self {
            files,
            dir,
            blocks,
            turn: RefCell::default(),
        }
    }

    /// Gives the copy that another answer holds, or reads the file. While one call
    /// reads, each other call waits for its copy.
    pub(super) async fn get(&self) -> Result<Rc<Stored>, Error> {
        poll_fn(|cx| {
            let mut turn = self.turn.borrow_mut();
            if !turn.reading {
                return Poll::Ready(());
            }
            turn.waiting.push(cx.waker().clone());
            Poll::Pending
        })
        .await;
        if let Some(copy) = self.turn.borrow().copy.upgrade() {
            return Ok(copy);
        }
        let reading = Reading::new(&self.turn);
        let read = founding::read(&self.files, &self.dir, &self.blocks, Mode::Read);
        let stored = read.await?.ok_or_else(|| founding::unfounded(&self.dir))?;
        let copy = Rc::new(stored);
        self.turn.borrow_mut().copy = Rc::downgrade(&copy);
        drop(reading);
        Ok(copy)
    }
}

// The turn of the call that reads. It ends when the call ends or drops, and wakes each
// call that waits.
struct Reading<'a>(&'a RefCell<Turn>);

impl<'a> Reading<'a> {
    fn new(turn: &'a RefCell<Turn>) -> Self {
        turn.borrow_mut().reading = true;
        Self(turn)
    }
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        let mut turn = self.0.borrow_mut();
        turn.reading = false;
        turn.waiting.drain(..).for_each(Waker::wake);
    }
}
