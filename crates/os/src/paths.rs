//! The files of the host by the paths that a user gave.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use env::entropy::Driver as _;
use env::paths::{Entry, Error, Found, Key, Kind, names_directory};
use rustix::io::Errno;

use crate::{dir, entropy, host};

/// The longest path that the host takes, with its end NUL.
const PATH_MAX: usize = libc::PATH_MAX.unsigned_abs() as usize;

#[derive(Debug)]
pub(crate) struct Driver;

impl env::paths::Driver for Driver {
    fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
        read(path, most).map_err(|e| error(&e))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error> {
        write(path, bytes).map_err(|e| error(&e))
    }

    fn make_dir(&self, path: &Path) -> Result<(), Error> {
        make_dir(path).map_err(|e| error(&e))
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::home_dir()
    }

    fn entry(&self, path: &Path) -> Result<Entry, Error> {
        entry(path).map_err(|e| error(&e))
    }

    fn absolute(&self, path: &Path) -> Result<PathBuf, Error> {
        // The walk takes a step for each name, so this bounds its work.
        if path.as_os_str().len() >= PATH_MAX {
            return Err(Error::Io {
                code: Errno::NAMETOOLONG.raw_os_error(),
            });
        }
        std::path::absolute(path).map_err(|e| error(&e))
    }

    fn looped(&self) -> Error {
        Error::Io {
            code: Errno::LOOP.raw_os_error(),
        }
    }
}

fn entry(path: &Path) -> io::Result<Entry> {
    let file = fs::symlink_metadata(path)?;
    let key = Key::new(file.dev(), file.ino());
    Ok(if file.is_symlink() {
        Entry::Link(key, fs::read_link(path)?)
    } else if file.is_dir() {
        Entry::Directory(key)
    } else {
        Entry::File(key)
    })
}

fn read(path: &Path, most: usize) -> io::Result<Found> {
    let file = match host::open(path)? {
        // macOS 15 opens the file of a link at a path that ends in `/`.
        host::Readable::Regular(..) if names_directory(path) => {
            return Err(Errno::NOTDIR.into());
        }
        host::Readable::Regular(file, _) => file,
        host::Readable::Directory(dir) => {
            let (mut entries, mut size) = (Vec::new(), 0_usize);
            for name in dir::names(dir)? {
                let entry = path.join(name?);
                size = size.saturating_add(entry.as_os_str().len());
                if size > most {
                    return Ok(Found::Large(Kind::Directory));
                }
                entries.push(entry);
            }
            entries.sort_unstable();
            return Ok(Found::Directory(entries));
        }
    };
    let over = u64::try_from(most).map_or(u64::MAX, |most| most.saturating_add(1));
    let mut bytes = Vec::new();
    File::from(file).take(over).read_to_end(&mut bytes)?;
    if bytes.len() > most {
        return Ok(Found::Large(Kind::File));
    }
    Ok(Found::File(bytes))
}

fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // `Path` drops an end in `/` or `.`, and takes a name after an end in `..`, so the
    // hidden file would go beside that directory or in it. Such a path names only a
    // directory, which takes no write. No open: macOS 15 opens the file of a link at
    // a path that ends in `/`.
    if names_directory(path) {
        let found = fs::metadata(path)?;
        let code = if found.is_dir() {
            Errno::ISDIR
        } else {
            Errno::NOTDIR
        };
        return Err(code.into());
    }
    match fs::symlink_metadata(path) {
        Ok(old) if old.is_file() => {}
        Ok(_) => return in_place(path, bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut key = [0; 8];
    entropy::Driver.fill(&mut key);
    let name = format!(".{:016x}.new", u64::from_le_bytes(key));
    let new = path.with_file_name(name);
    fill(open(&new)?, bytes)
        .and_then(|()| fs::rename(&new, path))
        .or_else(|e| fs::remove_file(&new).and(Err(e)))
}

/// Makes a directory at `path` as [`dir::make`] does.
fn make_dir(path: &Path) -> io::Result<()> {
    // macOS makes the target of a link to nothing at a path that ends in `/`, and
    // Linux gives `EEXIST`.
    let bytes = path.as_os_str().as_bytes();
    let end = bytes
        .iter()
        .rposition(|&byte| byte != b'/')
        .map_or(1, |at| at + 1);
    let path = Path::new(OsStr::from_bytes(bytes.get(..end).unwrap_or(bytes)));
    dir::make(rustix::fs::CWD, path).map_err(io::Error::from)
}

/// Makes a file at `path` whose permission bits give only this user access.
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
/// whole file.
fn fill(mut file: File, bytes: &[u8]) -> io::Result<()> {
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
