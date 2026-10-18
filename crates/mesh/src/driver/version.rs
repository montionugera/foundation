//! The file `card` of a mesh directory: the last version of a card of this node that
//! the node signed. A node that stops after the commit of its card and before its own
//! apply signs above it at its next start, so no peer refuses the card as behind.
//!
//! The file holds two slots of one sector. Each holds an 8-byte check of the next 10
//! bytes, the format version, and the card version. A write goes to the slot that does
//! not hold the newest version, so a torn write keeps the newest.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use env::files::{self, Files, Mode};
use types::ed25519::PrivateKey;
use types::node;

use super::Config;
use crate::card::addresses::Addresses;
use crate::card::{Card, Signed};
use crate::error::Error;
use crate::file::{self, Blocks, CHECK};

/// The name of the file in a mesh directory.
pub(super) const FILE: &str = "card";
const FORMAT: u16 = 1;
const SLOT: usize = files::SECTOR;
// The bytes of the frame of a slot.
const FRAME: usize = CHECK + 2 + 8;

/// Signs the cards of this node, each at a version that the file holds first. One
/// call at a time writes the file, in the order of the calls.
#[derive(Debug)]
pub(super) struct Cards {
    key: node::Key,
    private_key: PrivateKey,
    files: Files,
    // The mesh directory.
    dir: PathBuf,
    blocks: Blocks,
    // Each call that waits or writes, by its key, with the waker of a call that
    // waits. The first holds the turn.
    calls: RefCell<BTreeMap<u64, Option<Waker>>>,
    // The key of the next call.
    next: Cell<u64>,
}

impl Cards {
    /// The signer of the cards of the node of `config`, which writes the file with
    /// `blocks`.
    pub(super) fn new(config: &Config, blocks: Blocks) -> Self {
        Self {
            key: config.key,
            private_key: config.private_key.clone(),
            files: config.files.clone(),
            dir: config.dir.clone(),
            blocks,
            calls: RefCell::default(),
            next: Cell::new(0),
        }
    }

    /// `held` with `addresses`, at the version that [`next`] gives for it. Gives
    /// `None`, and signs nothing, when `held` has `addresses` and the file holds no
    /// version above it.
    ///
    /// # Errors
    ///
    /// As [`next`].
    pub(super) async fn sign(
        &self,
        held: &Card,
        addresses: &Addresses,
    ) -> Result<Option<Signed>, Error> {
        let mut turn = self.turn();
        (&mut turn).await;
        // At a start, the applied state can lag a card of the file that commits after
        // it, and would then hide `held`.
        if held.addresses == *addresses
            && held.version >= last(&self.files, &self.dir, &self.blocks).await?
        {
            return Ok(None);
        }
        let version = next(&self.files, &self.dir, &self.blocks, held.version).await?;
        let card = Card {
            addresses: addresses.clone(),
            version,
            ..held.clone()
        };
        Ok(Some(Signed::sign(self.key, card, &self.private_key)))
    }

    fn turn(&self) -> Turn<'_> {
        let key = self.next.get();
        self.next.set(
            key.checked_add(1)
                .expect("invariant: 2^64 calls do not happen"),
        );
        self.calls.borrow_mut().insert(key, None);
        Turn { cards: self, key }
    }
}

// A call in the order of `Cards`. It is ready when the call holds the turn, and its
// drop gives the turn to the next call.
struct Turn<'a> {
    cards: &'a Cards,
    key: u64,
}

impl Future for Turn<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut calls = self.cards.calls.borrow_mut();
        if calls.first_key_value().map(|(&key, _)| key) == Some(self.key) {
            return Poll::Ready(());
        }
        let replaced = calls.insert(self.key, Some(cx.waker().clone()));
        // The last drop of a waker can drop a task that uses the cards.
        drop(calls);
        drop(replaced);
        Poll::Pending
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut calls = self.cards.calls.borrow_mut();
        let removed = calls.remove(&self.key);
        // The call that holds the turn keeps no waker, so this wakes a call only
        // when the dropped one held the turn.
        let next = calls
            .first_entry()
            .and_then(|mut next| next.get_mut().take());
        drop(calls);
        drop(removed);
        if let Some(next) = next {
            next.wake();
        }
    }
}

/// Gives a version above `held` and above each version that the file in `dir` holds,
/// once the file holds it durably. One call at a time writes the file; a write open
/// waits for the calls of a dropped call.
///
/// # Errors
///
/// - [`Error::Files`] when a file call fails.
/// - [`Error::Pool`] when the pool of `blocks` has no block.
/// - [`Error::CardFile`] when the file holds no valid slot that a torn write explains.
///
/// # Panics
///
/// When `held` or the version in the file is `u64::MAX`.
pub(super) async fn next(
    files: &Files,
    dir: &Path,
    blocks: &Blocks,
    held: u64,
) -> Result<u64, Error> {
    let opened = open(files, dir).await?;
    let version = write(&opened, &dir.join(FILE), blocks, held).await;
    opened.close().await;
    let version = version?;
    // The create of the first call is durable only after this.
    files.sync_dir(dir).await.map_err(Error::Files)?;
    Ok(version)
}

/// The newest version that the file in `dir` holds, or 0 when it holds none.
///
/// # Errors
///
/// As [`next`].
pub(super) async fn last(
    files: &Files,
    dir: &Path,
    blocks: &Blocks,
) -> Result<u64, Error> {
    let opened = open(files, dir).await?;
    let versions = versions(&opened, &dir.join(FILE), blocks).await;
    opened.close().await;
    Ok(newest(versions?))
}

// A create that a power cut drops leaves no version, as a file that is not there does.
async fn open(files: &Files, dir: &Path) -> Result<files::File, Error> {
    let len = file::wide(2 * SLOT);
    files
        .open(&dir.join(FILE), Mode::Create { len })
        .await
        .map_err(Error::Files)
}

async fn write(
    file: &files::File,
    path: &Path,
    blocks: &Blocks,
    held: u64,
) -> Result<u64, Error> {
    let versions = versions(file, path, blocks).await?;
    let newest = newest(versions);
    let version = held
        .max(newest)
        .checked_add(1)
        .expect("invariant: a card version is below u64::MAX");
    let at = if versions[0] > versions[1] { SLOT } else { 0 };
    let mut bytes = file::frame(FORMAT, &version.to_le_bytes());
    bytes.resize(SLOT, 0);
    blocks.write(file, file::wide(at), &mut &bytes[..]).await?;
    file.sync().await.map_err(Error::Files)?;
    Ok(version)
}

// The version of each slot.
async fn versions(
    file: &files::File,
    path: &Path,
    blocks: &Blocks,
) -> Result<[Option<u64>; 2], Error> {
    let bytes = blocks.read(file).await?;
    let (first, second) = bytes.split_at(SLOT);
    match [read(first), read(second)] {
        [Slot::Broken, Slot::Broken] | [Slot::Foreign, _] | [_, Slot::Foreign] => {
            Err(Error::CardFile {
                path: path.to_path_buf(),
            })
        }
        slots => Ok(slots.map(|slot| match slot {
            Slot::Version(version) => Some(version),
            _ => None,
        })),
    }
}

fn newest(versions: [Option<u64>; 2]) -> u64 {
    versions.into_iter().flatten().max().unwrap_or(0)
}

// What a slot of the file holds. A write touches one slot, so a broken slot is a torn
// write only while the other is not broken.
enum Slot {
    // All zero.
    Unwritten,
    // Its check fails.
    Broken,
    // Its check holds with another format.
    Foreign,
    Version(u64),
}

fn read(slot: &[u8]) -> Slot {
    if slot.iter().all(|&byte| byte == 0) {
        return Slot::Unwritten;
    }
    let framed = slot.get(..FRAME).expect("invariant: a slot is a sector");
    match file::unframe(framed) {
        Some((FORMAT, version)) => {
            let version = version
                .try_into()
                .expect("invariant: a version has 8 bytes");
            Slot::Version(u64::from_le_bytes(version))
        }
        Some(_) => Slot::Foreign,
        None => Slot::Broken,
    }
}
