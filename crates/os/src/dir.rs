//! The entries of an open directory.

use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;

use rustix::fs::Dir;
use rustix::io;

/// The names in the open directory `fd`, without `.` and `..`, read through `fd` with
/// no second open.
pub(crate) fn names(
    fd: impl Into<OwnedFd>,
) -> io::Result<impl Iterator<Item = io::Result<OsString>>> {
    Ok(Dir::new(fd)?.filter_map(|entry| match entry {
        Ok(entry) => {
            let name = entry.file_name().to_bytes();
            (name != b"." && name != b"..")
                .then(|| Ok(OsStr::from_bytes(name).to_owned()))
        }
        Err(errno) => Some(Err(errno)),
    }))
}
