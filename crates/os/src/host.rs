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
pub(crate) enum Handle {
    /// A regular file, with its stat.
    File(OwnedFd, fs::Stat),
    Directory(OwnedFd),
}

/// Opens the regular file or the directory at `path` to read. The open does not wait
/// for a writer of a FIFO.
///
/// # Errors
///
/// 22 (`EINVAL`) for a file that is neither a regular file nor a directory, else the
/// error of the stat or the open. A link that another writer changes between the
/// stat and the open gives the error of the open of the file that it then names, and
/// the open of a device there runs its driver.
pub(crate) fn open(path: &Path) -> Result<Handle, Errno> {
    // The check before the open keeps the open of a socket from giving `ENXIO`, and
    // that of a device from running its driver. The check after it refuses a swap.
    kind(&fs::stat(path)?)?;
    let fd = fs::open(path, READ, fs::Mode::empty())?;
    let stat = fs::fstat(&fd)?;
    Ok(match kind(&stat)? {
        FileType::Directory => Handle::Directory(fd),
        _ => Handle::File(fd, stat),
    })
}

/// The kind of a regular file or a directory, or 22 (`EINVAL`) for another kind.
fn kind(stat: &fs::Stat) -> Result<FileType, Errno> {
    match FileType::from_raw_mode(stat.st_mode) {
        kind @ (FileType::RegularFile | FileType::Directory) => Ok(kind),
        _ => Err(Errno::INVAL),
    }
}
