//! The files of the host by the paths that a user gave.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use env::entropy::Driver as _;
use env::paths::{Entry, Error, Found, Key, Kind, names_directory};
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, Stat};
use rustix::io::Errno;

use crate::{dir, entropy, host};

/// The longest path that the host takes, with its end NUL.
const PATH_MAX: usize = libc::PATH_MAX.unsigned_abs() as usize;

/// The open of a directory of a walk, which needs no read permission on it, as a walk
/// of the host needs none. On macOS it needs search permission on it.
#[cfg(target_os = "linux")]
const SEARCH: OFlags = OFlags::PATH;
#[cfg(target_os = "macos")]
const SEARCH: OFlags = OFlags::from_bits_retain(libc::O_SEARCH.cast_unsigned());

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

    fn root(&self, _: &Path) -> Result<Box<dyn env::paths::dir::Driver>, Error> {
        let root = Dir::open(CWD, OsStr::new("/")).map_err(errno)?;
        Ok(Box::new(root))
    }

    fn not_directory(&self) -> Error {
        Error::Io {
            code: Errno::NOTDIR.raw_os_error(),
        }
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

/// A directory of the host, open to walk.
#[derive(Debug)]
struct Dir {
    /// `None` on macOS for a directory that this user cannot search: the host refuses
    /// each name in it with `EACCES`.
    fd: Option<OwnedFd>,
    key: Key,
}

impl Dir {
    /// Opens the directory `name` of `at`, with no link followed.
    fn open(at: BorrowedFd<'_>, name: &OsStr) -> Result<Self, Errno> {
        let flags = SEARCH | OFlags::NOFOLLOW | OFlags::DIRECTORY | OFlags::CLOEXEC;
        match rustix::fs::openat(at, name, flags, Mode::empty()) {
            // The key of the open, not of a stat before it, which a swap can change.
            Ok(fd) => Ok(Self {
                key: key(&rustix::fs::fstat(&fd)?),
                fd: Some(fd),
            }),
            #[cfg(target_os = "macos")]
            Err(Errno::ACCESS) => {
                let stat = rustix::fs::statat(at, name, AtFlags::SYMLINK_NOFOLLOW)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                    return Err(Errno::ACCESS);
                }
                Ok(Self {
                    fd: None,
                    key: key(&stat),
                })
            }
            Err(e) => Err(e),
        }
    }

    fn entry(&self, name: &OsStr) -> Result<Entry, Errno> {
        let fd = self.fd.as_ref().ok_or(Errno::ACCESS)?;
        let stat = rustix::fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let key = key(&stat);
        Ok(match FileType::from_raw_mode(stat.st_mode) {
            FileType::Symlink => {
                let target = rustix::fs::readlinkat(fd, name, Vec::new())?;
                Entry::Link(key, OsString::from_vec(target.into_bytes()).into())
            }
            FileType::Directory => {
                Entry::Directory(Box::new(Self::open(fd.as_fd(), name)?))
            }
            FileType::RegularFile => Entry::File(key),
            _ => Entry::Special(key),
        })
    }
}

impl env::paths::dir::Driver for Dir {
    fn key(&self) -> Key {
        self.key
    }

    fn open(&self, name: &OsStr) -> Result<Entry, Error> {
        self.entry(name).map_err(errno)
    }
}

/// The key of the file of `stat`.
fn key(stat: &Stat) -> Key {
    #[cfg(target_os = "linux")]
    let device = stat.st_dev;
    // `dev_t` is `i32` on macOS.
    #[cfg(target_os = "macos")]
    let device = u64::from(stat.st_dev.cast_unsigned());
    Key::new(device, stat.st_ino)
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

fn errno(e: Errno) -> Error {
    error(&e.into())
}

fn error(e: &io::Error) -> Error {
    if e.kind() == io::ErrorKind::NotFound {
        return Error::NotFound;
    }
    // Std gives no code for a path with a nul byte.
    let code = e.raw_os_error().unwrap_or(Errno::INVAL.raw_os_error());
    Error::Io { code }
}
