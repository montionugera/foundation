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

/// Opens the regular file or the directory at `path` to read, and gives it with its
/// stat. The open does not wait for a writer of a FIFO.
///
/// # Errors
///
/// 22 (`EINVAL`) for a file that is neither a regular file nor a directory, else the
/// error of the stat or the open.
pub(crate) fn open(path: &Path) -> Result<(OwnedFd, fs::Stat), Errno> {
    // The check before the open keeps the open of a socket from giving `ENXIO`, and
    // that of a device from running its driver. The check after it covers a swap.
    readable(&fs::stat(path)?)?;
    let fd = fs::open(path, READ, fs::Mode::empty())?;
    let stat = fs::fstat(&fd)?;
    readable(&stat)?;
    Ok((fd, stat))
}

fn readable(stat: &fs::Stat) -> Result<(), Errno> {
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::RegularFile | FileType::Directory => Ok(()),
        _ => Err(Errno::INVAL),
    }
}
