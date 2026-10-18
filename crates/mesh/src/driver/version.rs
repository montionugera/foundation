//! The file `card` of a mesh directory: the last version of a card of this node that
//! the node signed. A node that stops after the commit of its card and before its own
//! apply signs above it at its next start, so no peer refuses the card as behind.
//!
//! The file holds two slots of one sector. Each holds an 8-byte check of the next 10
//! bytes, the format version, and the card version. A write goes to the slot that does
//! not hold the newest version, so a torn write keeps the newest.

use std::path::{Path, PathBuf};

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

/// Signs the cards of this node, each at a version that the file holds first.
#[derive(Debug)]
pub(super) struct Cards {
    key: node::Key,
    private_key: PrivateKey,
    files: Files,
    // The mesh directory.
    dir: PathBuf,
    blocks: Blocks,
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
        }
    }

    /// `held` with `addresses`, at the version that [`next`] gives for it.
    ///
    /// # Errors
    ///
    /// As [`next`].
    pub(super) async fn sign(
        &self,
        held: &Card,
        addresses: Addresses,
    ) -> Result<Signed, Error> {
        let version = next(&self.files, &self.dir, &self.blocks, held.version).await?;
        let card = Card {
            addresses,
            version,
            ..held.clone()
        };
        Ok(Signed::sign(self.key, card, &self.private_key))
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
    let len = file::wide(2 * SLOT);
    let opened = files
        .open(&dir.join(FILE), Mode::Create { len })
        .await
        .map_err(Error::Files)?;
    let path = dir.join(FILE);
    let version = write(&opened, &path, blocks, held).await;
    opened.close().await;
    let version = version?;
    // The create of the first call is durable only after this.
    files.sync_dir(dir).await.map_err(Error::Files)?;
    Ok(version)
}

async fn write(
    file: &files::File,
    path: &Path,
    blocks: &Blocks,
    held: u64,
) -> Result<u64, Error> {
    let bytes = blocks.read(file).await?;
    let (first, second) = bytes.split_at(SLOT);
    let versions = match [read(first), read(second)] {
        [Slot::Broken, Slot::Broken] | [Slot::Foreign, _] | [_, Slot::Foreign] => {
            return Err(Error::CardFile {
                path: path.to_path_buf(),
            });
        }
        slots => slots.map(|slot| match slot {
            Slot::Version(version) => Some(version),
            _ => None,
        }),
    };
    let newest = versions.iter().flatten().max().copied().unwrap_or(0);
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
