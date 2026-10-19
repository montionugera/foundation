//! Directories: the entries of an open one, and the make of one.

use std::ffi::{OsStr, OsString};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use rustix::fs::{self, AtFlags, Dir, FileType};
use rustix::io::{self, Errno};

/// The mode of a new directory: it gives the group and other users no access.
pub(crate) const MODE: fs::Mode = fs::Mode::from_raw_mode(0o700);

/// Makes directory `path` of `at`. A directory there gives `Ok`, and anything else
/// there gives `EXIST`. `flags` are those of the stat of what is there: a link to a
/// directory is a directory unless they hold `SYMLINK_NOFOLLOW`.
pub(crate) fn make(at: impl AsFd, path: &Path, flags: AtFlags) -> io::Result<()> {
    match fs::mkdirat(&at, path, MODE) {
        Err(Errno::EXIST)
            if fs::statat(&at, path, flags)
                .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode).is_dir()) =>
        {
            Ok(())
        }
        made => made,
    }
}

/// The names in the open directory `fd`, without `.` and `..`, read through `fd` with
/// no second open.
pub(crate) fn names(
    fd: impl Into<OwnedFd>,
) -> io::Result<impl Iterator<Item = io::Result<OsString>>> {
    let mut dir = Dir::new(fd)?;
    // A copy of a descriptor shares its place in the listing with the original.
    dir.rewind();
    Ok(dir.filter_map(|entry| match entry {
        Ok(entry) => {
            let name = entry.file_name().to_bytes();
            (name != b"." && name != b"..")
                .then(|| Ok(OsStr::from_bytes(name).to_owned()))
        }
        Err(errno) => Some(Err(errno)),
    }))
}
