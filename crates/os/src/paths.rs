//! The files of the host by the paths that a user gave.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use env::paths::{Error, Found};
use rustix::io::Errno;

#[derive(Debug)]
pub(crate) struct Driver;

impl env::paths::Driver for Driver {
    fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
        read(path, most).map_err(|e| error(&e))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error> {
        let mut new = path.as_os_str().to_owned();
        new.push(".new");
        let new = PathBuf::from(new);
        let written = create(&new, bytes).and_then(|()| fs::rename(&new, path));
        if written.is_err() {
            // The error of the write is the one to give.
            drop(fs::remove_file(&new));
        }
        written.map_err(|e| error(&e))
    }
}

fn read(path: &Path, most: usize) -> io::Result<Found> {
    #[expect(clippy::disallowed_methods, reason = "os reads the user's files")]
    let file = File::open(path)?;
    if file.metadata()?.is_dir() {
        let mut names = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<Vec<OsString>>>()?;
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

/// Writes `bytes` as a new file at `path`, synced, so a power cut after the rename
/// leaves the whole file.
fn create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[expect(clippy::disallowed_methods, reason = "os writes the user's files")]
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
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
