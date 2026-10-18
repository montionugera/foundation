//! The file `card` of a mesh directory: the last version of a card of this node that
//! the node signed. A node that stops after the commit of its card and before its own
//! apply signs above it at its next start, so no peer refuses the card as behind.
//!
//! The file holds two slots of one sector. Each holds an 8-byte check of the next 10
//! bytes, the format version, and the card version. A write goes to the slot that does
//! not hold the newest version, so a torn write keeps the newest.

use std::path::Path;

use env::files::{self, Files, Mode};

use crate::error::Error;
use crate::file::{self, Blocks, CHECK};

/// The name of the file in a mesh directory.
pub(super) const FILE: &str = "card";
const FORMAT: u16 = 1;
const SLOT: usize = files::SECTOR;
const BODY: usize = 2 + 8;

/// Gives a version above `held` and above each version that the file in `dir` holds,
/// once the file holds it durably. One call at a time writes the file; a write open
/// waits for the calls of a dropped call.
///
/// # Errors
///
/// [`Error::Files`] when a file call fails, and [`Error::Pool`] when the pool of
/// `blocks` has no block.
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
    let version = write(&opened, blocks, held).await;
    opened.close().await;
    let version = version?;
    // The create of the first call is durable only after this.
    files.sync_dir(dir).await.map_err(Error::Files)?;
    Ok(version)
}

async fn write(file: &files::File, blocks: &Blocks, held: u64) -> Result<u64, Error> {
    let bytes = blocks.read(file).await?;
    let (first, second) = bytes.split_at(SLOT);
    let versions = [read(first), read(second)];
    let newest = versions.iter().flatten().max().copied().unwrap_or(0);
    let version = held
        .max(newest)
        .checked_add(1)
        .expect("invariant: a card version is below u64::MAX");
    let slot = usize::from(versions[0] > versions[1]);
    let mut body = FORMAT.to_le_bytes().to_vec();
    body.extend(version.to_le_bytes());
    let mut bytes = file::check(&body).to_vec();
    bytes.extend(body);
    bytes.resize(SLOT, 0);
    blocks
        .write(file, file::wide(slot * SLOT), &mut &bytes[..])
        .await?;
    file.sync().await.map_err(Error::Files)?;
    Ok(version)
}

// The version that `slot` holds, or `None` when its check or format does not hold.
fn read(slot: &[u8]) -> Option<u64> {
    let (check, rest) = slot.split_first_chunk::<CHECK>()?;
    let body = rest.get(..BODY)?;
    let (format, version) = body.split_first_chunk::<2>()?;
    if *check != file::check(body) || u16::from_le_bytes(*format) != FORMAT {
        return None;
    }
    Some(u64::from_le_bytes(version.try_into().ok()?))
}
