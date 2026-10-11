//! The files of the host by the paths that a user gave.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use env::entropy::Driver as _;
use env::paths::{Error, Found};
use rustix::io::Errno;

use crate::entropy;

#[derive(Debug)]
pub(crate) struct Driver;

impl env::paths::Driver for Driver {
    fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
        read(path, most).map_err(|e| error(&e))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error> {
        if names_directory(path) {
            // Such a path resolves only to a directory, so the open fails.
            return in_place(path, bytes).map_err(|e| error(&e));
        }
        let kept = match fs::symlink_metadata(path) {
            Ok(old) if old.is_file() => Some(old.permissions()),
            Ok(_) => return in_place(path, bytes).map_err(|e| error(&e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(error(&e)),
        };
        let mut key = [0; 8];
        entropy::Driver.fill(&mut key);
        let name = format!(".{:016x}.new", u64::from_le_bytes(key));
        let new = path.with_file_name(name);
        let file = open(&new).map_err(|e| error(&e))?;
        fill(file, bytes, kept)
            .and_then(|()| fs::rename(&new, path))
            .or_else(|e| fs::remove_file(&new).and(Err(e)))
            .map_err(|e| error(&e))
    }
}

/// Whether `path` is empty or ends in `/`, `.`, or `..`, so that no file can be at
/// it.
fn names_directory(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    let name = bytes.rsplit(|&byte| byte == b'/').next();
    matches!(name, Some(b"" | b"." | b".."))
}

fn read(path: &Path, most: usize) -> io::Result<Found> {
    #[expect(clippy::disallowed_methods, reason = "os reads the user's files")]
    let file = File::open(path)?;
    if file.metadata()?.is_dir() {
        let (mut entries, mut size) = (Vec::new(), 0_usize);
        for entry in fs::read_dir(path)? {
            let entry = path.join(entry?.file_name());
            size = size.saturating_add(entry.as_os_str().len());
            if size > most {
                return Ok(Found::Large);
            }
            entries.push(entry);
        }
        entries.sort_unstable();
        return Ok(Found::Directory(entries));
    }
    let over = u64::try_from(most).map_or(u64::MAX, |most| most.saturating_add(1));
    let mut bytes = Vec::new();
    file.take(over).read_to_end(&mut bytes)?;
    if bytes.len() > most {
        return Ok(Found::Large);
    }
    Ok(Found::File(bytes))
}

/// Makes a file at `path` that only this user can read or write.
fn open(path: &Path) -> io::Result<File> {
    #[expect(clippy::disallowed_methods, reason = "os writes the user's files")]
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Writes `bytes` through an open of `path`, for a link, a device, a pipe, or a
/// directory, which gives `EISDIR`. A device may refuse a sync, so there is none.
fn in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[expect(clippy::disallowed_methods, reason = "os writes the user's files")]
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(bytes)
}

/// Writes `bytes` to `file`, synced, so a power cut after the rename leaves the
/// whole file. `kept` is the mode of the file that it replaces.
fn fill(mut file: File, bytes: &[u8], kept: Option<Permissions>) -> io::Result<()> {
    if let Some(kept) = kept {
        file.set_permissions(kept)?;
    }
    file.write_all(bytes)?;
    file.sync_all()
}

fn error(e: &io::Error) -> Error {
    if e.kind() == io::ErrorKind::NotFound {
        return Error::NotFound;
    }
    // Std gives no code for a path with a nul byte.
    let code = e.raw_os_error().unwrap_or(Errno::INVAL.raw_os_error());
    Error::Io { code }
}
