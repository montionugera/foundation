//! The files of the host by the paths that a user gave.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
        if path.as_os_str().is_empty() {
            return Err(Error::NotFound);
        }
        // `/`, `.`, and a path that ends in `..` name a directory.
        let Some(name) = path.file_name() else {
            return Err(Error::Io {
                code: Errno::ISDIR.raw_os_error(),
            });
        };
        let mut key = [0; 8];
        entropy::Driver.fill(&mut key);
        let mut new = name.to_owned();
        new.push(format!(".{:016x}.new", u64::from_le_bytes(key)));
        let new = path.with_file_name(new);
        // A new file gives other users no access. A replaced file keeps its mode.
        let mode = fs::metadata(path).map_or(0o600, |old| old.permissions().mode());
        let file = open(&new).map_err(|e| error(&e))?;
        let written = fill(file, bytes, mode).and_then(|()| fs::rename(&new, path));
        if let Err(e) = written {
            match fs::remove_file(&new) {
                // Only an I/O fault keeps the file. The error of the write says why
                // the write failed.
                Ok(()) | Err(_) => {}
            }
            return Err(error(&e));
        }
        Ok(())
    }
}

fn read(path: &Path, most: usize) -> io::Result<Found> {
    #[expect(clippy::disallowed_methods, reason = "os reads the user's files")]
    let file = File::open(path)?;
    if file.metadata()?.is_dir() {
        let (mut names, mut size) = (Vec::new(), 0_usize);
        for entry in fs::read_dir(path)? {
            let name = entry?.file_name();
            size = size.saturating_add(name.len());
            if size > most {
                return Ok(Found::Large);
            }
            names.push(name);
        }
        names.sort_unstable();
        let entries = names.into_iter().map(|name| path.join(name)).collect();
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

/// Writes `bytes` to `file` with `mode`, synced, so a power cut after the rename
/// leaves the whole file.
fn fill(mut file: File, bytes: &[u8], mode: u32) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(mode))?;
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
