//! The open of a file of the host by its path.

use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{self, FileType, OFlags};
use rustix::io::Errno;

/// The flags that open a file of the host to read, with no wait for a writer of a FIFO.
/// `NOCTTY` keeps a terminal from becoming that of a node that leads its session.
const READ: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK)
    .union(OFlags::NOCTTY);

/// Opens the regular file at `path` to read, and gives its stat. The open does not
/// wait for a writer of a FIFO.
///
/// # Errors
///
/// 21 (`EISDIR`) for a directory, also one that this user cannot read, 22 (`EINVAL`)
/// for a file that is neither a regular file nor a directory, else the error of the
/// stat or the open. A link that another writer changes between the stat and the open
/// gives the error of the open of the file that it then names, and the open of a
/// device there runs its driver. On Linux, the walk of a link that a rename replaces
/// during the call can read only a first part of its old target, also none of it, so
/// the call can open a folder on the way to that target, such as `/`, and the rest of
/// `path` resolves under that folder.
pub(crate) fn file(path: &Path) -> Result<(OwnedFd, fs::Stat), Errno> {
    // The stat keeps the open of a socket from giving `ENXIO` on Linux or `EOPNOTSUPP`
    // on macOS, and that of a device from running its driver.
    if kind(&fs::stat(path)?)? == FileType::Directory {
        return Err(Errno::ISDIR);
    }
    // The fstat after the open refuses a swap.
    let fd = fs::open(path, READ, fs::Mode::empty())?;
    let stat = fs::fstat(&fd)?;
    match kind(&stat)? {
        FileType::RegularFile => Ok((fd, stat)),
        _ => Err(Errno::ISDIR),
    }
}

/// The kind of a regular file or a directory, or 22 (`EINVAL`) for another kind.
fn kind(stat: &fs::Stat) -> Result<FileType, Errno> {
    match FileType::from_raw_mode(stat.st_mode) {
        kind @ (FileType::RegularFile | FileType::Directory) => Ok(kind),
        _ => Err(Errno::INVAL),
    }
}
