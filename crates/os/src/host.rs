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

/// A regular file or a directory of the host, open to read.
pub(crate) enum Readable {
    /// A regular file, with its stat.
    Regular(OwnedFd, fs::Stat),
    Directory(OwnedFd),
}

/// Opens the regular file or the directory at `path` to read. The open does not wait
/// for a writer of a FIFO.
///
/// # Errors
///
/// 22 (`EINVAL`) for a file that is neither a regular file nor a directory, else the
/// error of the stat or the open. A link that another writer changes between the stat
/// and the open gives the error of the open of the file that it then names, and the
/// open of a device there runs its driver. On Linux, the walk of a link that a rename
/// replaces during the call can read only a first part of its old target, so the call
/// can open a folder on the way to that target, such as the folder of the link or `/`,
/// and the rest of `path` resolves under that folder.
pub(crate) fn open(path: &Path) -> Result<Readable, Errno> {
    kind(&fs::stat(path)?)?;
    opened(path)
}

/// Opens the regular file at `path` to read, as `open` does, and gives its stat.
///
/// # Errors
///
/// 21 (`EISDIR`) for a directory, also one that this user cannot read, else as
/// `open`.
pub(crate) fn file(path: &Path) -> Result<(OwnedFd, fs::Stat), Errno> {
    if kind(&fs::stat(path)?)? == FileType::Directory {
        return Err(Errno::ISDIR);
    }
    match opened(path)? {
        Readable::Regular(fd, stat) => Ok((fd, stat)),
        Readable::Directory(_) => Err(Errno::ISDIR),
    }
}

/// Opens `path`, after a stat found a regular file or a directory there. The stat of
/// the caller keeps the open of a socket from giving `ENXIO`, and that of a device from
/// running its driver.
fn opened(path: &Path) -> Result<Readable, Errno> {
    // The fstat after the open refuses a swap.
    let fd = fs::open(path, READ, fs::Mode::empty())?;
    let stat = fs::fstat(&fd)?;
    Ok(match kind(&stat)? {
        FileType::Directory => Readable::Directory(fd),
        _ => Readable::Regular(fd, stat),
    })
}

/// The kind of a regular file or a directory, or 22 (`EINVAL`) for another kind.
fn kind(stat: &fs::Stat) -> Result<FileType, Errno> {
    match FileType::from_raw_mode(stat.st_mode) {
        kind @ (FileType::RegularFile | FileType::Directory) => Ok(kind),
        _ => Err(Errno::INVAL),
    }
}
