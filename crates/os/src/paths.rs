//! The files of the host by the paths that a user gave.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use env::entropy::Driver as _;
use env::paths::dir::{Entry, Opened};
use env::paths::{Error, Key, Refusal};
use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, Stat};
use rustix::io::Errno;

use crate::{dir, entropy};

/// The longest path that the host takes, with its end NUL.
const PATH_MAX: usize = libc::PATH_MAX.unsigned_abs() as usize;

/// The open of a directory to search and not read, as a walk of the host needs no read
/// permission. On macOS it needs search permission.
#[cfg(target_os = "linux")]
const SEARCH: OFlags = OFlags::PATH;
#[cfg(target_os = "macos")]
const SEARCH: OFlags = OFlags::from_bits_retain(libc::O_SEARCH.cast_unsigned());

/// The flags of each open of a directory. With no access flag, an open reads.
const DIR: OFlags = OFlags::NOFOLLOW
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);

/// The flags of each open of a file. `NOCTTY` keeps a terminal from becoming that of a
/// node that leads its session.
const FILE: OFlags = OFlags::NOFOLLOW
    .union(OFlags::NOCTTY)
    .union(OFlags::CLOEXEC);

#[derive(Debug)]
pub(crate) struct Driver;

impl env::paths::Driver for Driver {
    fn home(&self) -> Option<PathBuf> {
        std::env::home_dir()
    }

    fn root(&self, _: &Path) -> Result<Box<dyn env::paths::dir::Driver>, Error> {
        let root = Dir::open(CWD, OsStr::new("/")).map_err(errno)?;
        Ok(Box::new(root))
    }

    fn absolute(&self, path: &Path) -> Result<PathBuf, Error> {
        // The walk takes a step for each name, so this bounds its work.
        if path.as_os_str().len() >= PATH_MAX {
            return Err(errno(Errno::NAMETOOLONG));
        }
        std::path::absolute(path).map_err(|e| error(&e))
    }

    fn error(&self, refusal: Refusal) -> Error {
        errno(match refusal {
            Refusal::NotDirectory => Errno::NOTDIR,
            Refusal::Directory => Errno::ISDIR,
            Refusal::Taken => Errno::EXIST,
            Refusal::Looped => Errno::LOOP,
            Refusal::Irregular => Errno::INVAL,
        })
    }
}

/// A directory of the host, open.
#[derive(Debug)]
struct Dir {
    /// `None` on macOS for a directory that this user cannot search: the host refuses
    /// each name in it with `EACCES`.
    fd: Option<OwnedFd>,
    /// Whether `fd` was opened to read, not with `SEARCH`.
    readable: bool,
    key: Key,
}

impl Dir {
    /// Opens the directory `name` of `at`, with no link followed. `ENOTDIR` or
    /// `ELOOP` when another file or a link is there.
    fn open(at: BorrowedFd<'_>, name: &OsStr) -> Result<Self, Errno> {
        match rustix::fs::openat(at, name, SEARCH | DIR, Mode::empty()) {
            // The key of the open, not of a stat before it, which a swap can change.
            Ok(fd) => Ok(Self {
                key: key(&rustix::fs::fstat(&fd)?),
                fd: Some(fd),
                readable: false,
            }),
            #[cfg(target_os = "macos")]
            Err(Errno::ACCESS) => {
                let stat = rustix::fs::statat(at, name, AtFlags::SYMLINK_NOFOLLOW)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                    return Err(Errno::NOTDIR);
                }
                Ok(Self {
                    fd: None,
                    readable: false,
                    key: key(&stat),
                })
            }
            Err(e) => Err(e),
        }
    }

    fn fd(&self) -> Result<BorrowedFd<'_>, Errno> {
        self.fd.as_ref().map(AsFd::as_fd).ok_or(Errno::ACCESS)
    }

    fn entry(&self, name: &OsStr) -> Result<Entry, Errno> {
        let fd = self.fd()?;
        // The open comes first, so no swap after a stat makes it follow a link.
        match Self::open(fd, name) {
            Ok(dir) => return Ok(Entry::Directory(Box::new(dir))),
            Err(Errno::NOTDIR | Errno::LOOP) => {}
            Err(e) => return Err(e),
        }
        let stat = rustix::fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let key = key(&stat);
        Ok(match FileType::from_raw_mode(stat.st_mode) {
            // A directory that a program puts at `name` after the open is a file to the
            // walk, which the open refused as a directory.
            FileType::RegularFile | FileType::Directory => Entry::File(key),
            FileType::Symlink => {
                let target = rustix::fs::readlinkat(fd, name, Vec::new())?.into_bytes();
                Entry::Link(key, OsString::from_vec(target).into())
            }
            _ => Entry::Special(key),
        })
    }

    fn names(&self, most: usize) -> Result<Option<Vec<OsString>>, Errno> {
        let fd = self.fd()?;
        // The listing takes its own descriptor, and the walk keeps `fd`.
        let listed = if self.readable {
            rustix::io::fcntl_dupfd_cloexec(fd, 0)?
        } else {
            rustix::fs::openat(fd, ".", DIR, Mode::empty())?
        };
        let (mut names, mut size) = (Vec::new(), 0_usize);
        for name in dir::names(listed)? {
            let name = name?;
            size = size.saturating_add(name.len());
            if size > most {
                return Ok(None);
            }
            names.push(name);
        }
        Ok(Some(names))
    }

    /// Opens `name` to read, with no wait for the writer of a FIFO.
    fn read(&self, name: &OsStr) -> Result<Opened, Errno> {
        let flags = FILE | OFlags::RDONLY | OFlags::NONBLOCK;
        let fd = rustix::fs::openat(self.fd()?, name, flags, Mode::empty())?;
        let stat = rustix::fs::fstat(&fd)?;
        let key = key(&stat);
        Ok(match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => Opened::Directory(Box::new(Self {
                fd: Some(fd),
                readable: true,
                key,
            })),
            kind => Opened::File(Box::new(File {
                handle: fd.into(),
                regular: kind == FileType::RegularFile,
                key,
            })),
        })
    }

    /// Opens `name` to write in place. `EISDIR` for a directory.
    fn write(&self, name: &OsStr) -> Result<File, Errno> {
        let flags = FILE | OFlags::WRONLY;
        let fd = rustix::fs::openat(self.fd()?, name, flags, Mode::empty())?;
        let stat = rustix::fs::fstat(&fd)?;
        Ok(File {
            handle: fd.into(),
            regular: FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile,
            key: key(&stat),
        })
    }

    fn replace(&self, name: &OsStr, bytes: &[u8]) -> Result<(), Errno> {
        let fd = self.fd()?;
        let mut key = [0; 8];
        entropy::Driver.fill(&mut key);
        let new = format!(".{:016x}.new", u64::from_le_bytes(key));
        let flags = FILE | OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL;
        let file = rustix::fs::openat(fd, &new, flags, Mode::from_raw_mode(0o600))?;
        fill(file.into(), bytes)
            .and_then(|()| rustix::fs::renameat(fd, &new, fd, name))
            .or_else(|e| rustix::fs::unlinkat(fd, &new, AtFlags::empty()).and(Err(e)))
    }

    fn make_dir(&self, name: &OsStr) -> Result<(), Errno> {
        dir::make(self.fd()?, Path::new(name), AtFlags::SYMLINK_NOFOLLOW)
    }
}

impl env::paths::dir::Driver for Dir {
    fn key(&self) -> Key {
        self.key
    }

    fn entry(&self, name: &OsStr) -> Result<Entry, Error> {
        Dir::entry(self, name).map_err(errno)
    }

    fn open(&self, name: &OsStr) -> Result<Opened, Error> {
        Dir::read(self, name).map_err(errno)
    }

    fn up(&self) -> Result<Box<dyn env::paths::dir::Driver>, Error> {
        let up =
            Self::open(self.fd().map_err(errno)?, OsStr::new("..")).map_err(errno)?;
        Ok(Box::new(up))
    }

    fn path(&self) -> Result<PathBuf, Error> {
        let fd = self.fd().map_err(errno)?;
        #[cfg(target_os = "linux")]
        let path = proc_path(fd).map_err(errno)?;
        #[cfg(target_os = "macos")]
        let path = getpath(fd).map_err(errno)?;
        let path = PathBuf::from(OsString::from_vec(path));
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(Error::NotFound)
        }
    }

    fn names(&self, most: usize) -> Result<Option<Vec<OsString>>, Error> {
        Dir::names(self, most).map_err(errno)
    }

    fn file(&self, name: &OsStr) -> Result<Box<dyn env::paths::file::Driver>, Error> {
        Ok(Box::new(Dir::write(self, name).map_err(errno)?))
    }

    fn replace(&self, name: &OsStr, bytes: &[u8]) -> Result<(), Error> {
        Dir::replace(self, name, bytes).map_err(errno)
    }

    fn make_dir(&self, name: &OsStr) -> Result<(), Error> {
        Dir::make_dir(self, name).map_err(errno)
    }
}

/// A file of the host, open.
#[derive(Debug)]
struct File {
    handle: fs::File,
    /// Whether it was a regular file at the open.
    regular: bool,
    key: Key,
}

impl env::paths::file::Driver for File {
    fn key(&self) -> Key {
        self.key
    }

    fn read(&self, most: usize) -> Result<Option<Vec<u8>>, Error> {
        if !self.regular {
            return Err(errno(Errno::INVAL));
        }
        let over = u64::try_from(most).map_or(u64::MAX, |most| most.saturating_add(1));
        let mut bytes = Vec::new();
        (&self.handle)
            .take(over)
            .read_to_end(&mut bytes)
            .map_err(|e| error(&e))?;
        Ok((bytes.len() <= most).then_some(bytes))
    }

    fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        // A device may refuse a cut or a sync, so it gets neither.
        if self.regular {
            self.handle.set_len(0).map_err(|e| error(&e))?;
        }
        (&self.handle).write_all(bytes).map_err(|e| error(&e))
    }
}

/// The path that Linux gives for `fd`.
#[cfg(target_os = "linux")]
fn proc_path(fd: BorrowedFd<'_>) -> Result<Vec<u8>, Errno> {
    use std::os::fd::AsRawFd;
    let link = format!("/proc/self/fd/{}", fd.as_raw_fd());
    Ok(rustix::fs::readlinkat(CWD, link, Vec::new())?.into_bytes())
}

/// The path that macOS gives for `fd`.
#[cfg(target_os = "macos")]
fn getpath(fd: BorrowedFd<'_>) -> Result<Vec<u8>, Errno> {
    rustix::fs::getpath(fd).map(std::ffi::CString::into_bytes)
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

/// Writes `bytes` to `file`, synced, so a power cut after the rename leaves the
/// whole file.
fn fill(mut file: fs::File, bytes: &[u8]) -> Result<(), Errno> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| Errno::from_io_error(&e).unwrap_or(Errno::IO))
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

#[cfg(test)]
mod tests {
    use env::paths::dir::Driver as _;

    use super::*;

    #[test]
    fn a_directory_gives_the_path_at_which_the_host_finds_it_now() {
        let name = format!("foundation-os-dir-path-{}", std::process::id());
        // The path that the host gives, also where the temporary directory is a link.
        let scratch = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        fs::create_dir_all(scratch.join("a")).unwrap();
        let a = Dir::open(CWD, scratch.join("a").as_os_str()).unwrap();
        assert_eq!(a.path(), Ok(scratch.join("a")));
        fs::rename(scratch.join("a"), scratch.join("b")).unwrap();
        assert_eq!(a.path(), Ok(scratch.join("b")));
        fs::remove_dir(scratch.join("b")).unwrap();
        fs::remove_dir(&scratch).unwrap();
        // A host can give the last path of a removed directory, where nothing is now.
        #[cfg(target_os = "linux")]
        let removed = Ok(PathBuf::from(format!(
            "{} (deleted)",
            scratch.join("b").display()
        )));
        #[cfg(target_os = "linux")]
        assert_eq!(a.path(), removed);
        #[cfg(target_os = "macos")]
        assert!(!a.path().is_ok_and(|path| path.exists()));
    }

    /// A new empty directory under the temporary directory, named for `case`.
    fn scratch(case: &str) -> PathBuf {
        let name = format!("foundation-os-{case}-{}", std::process::id());
        let scratch = std::env::temp_dir().join(name);
        if scratch.exists() {
            fs::remove_dir_all(&scratch).unwrap();
        }
        fs::create_dir(&scratch).unwrap();
        scratch
    }

    #[test]
    fn each_listing_gives_each_name_up_to_its_bound() {
        let scratch = scratch("dir-names");
        fs::write(scratch.join("ab"), b"").unwrap();
        fs::write(scratch.join("cd"), b"").unwrap();
        let dir = Dir::open(CWD, scratch.as_os_str()).unwrap();
        let names = [4, 3, 4].map(|most| Ok(dir.names(most)?.map(|names| names.len())));
        fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(names, [Ok::<_, Errno>(Some(2)), Ok(None), Ok(Some(2))]);
    }

    #[test]
    fn a_read_follows_no_link_at_its_name() {
        let scratch = scratch("dir-read-link");
        fs::write(scratch.join("a"), b"").unwrap();
        std::os::unix::fs::symlink(scratch.join("a"), scratch.join("l")).unwrap();
        let dir = Dir::open(CWD, scratch.as_os_str()).unwrap();
        let read = dir.read(OsStr::new("l")).err();
        fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(read, Some(Errno::LOOP));
    }

    #[test]
    fn a_read_of_a_fifo_waits_for_no_writer() {
        let scratch = scratch("dir-read-fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(scratch.join("p"))
            .status()
            .unwrap();
        assert!(made.success());
        let dir = Dir::open(CWD, scratch.as_os_str()).unwrap();
        let read = dir.read(OsStr::new("p"));
        let read = read.map(|opened| matches!(opened, Opened::File(_)));
        fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(read, Ok(true));
    }

    #[test]
    fn make_dir_is_ok_on_a_directory_there_and_fails_on_a_file() {
        let scratch = scratch("dir-make");
        fs::write(scratch.join("f"), b"").unwrap();
        let dir = Dir::open(CWD, scratch.as_os_str()).unwrap();
        let made = ["d", "d", "f"].map(|name| dir.make_dir(OsStr::new(name)));
        fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(made, [Ok(()), Ok(()), Err(Errno::EXIST)]);
    }

    #[test]
    fn a_directory_with_no_handle_gives_the_refusal_of_the_host_for_its_path() {
        let dir = Dir {
            fd: None,
            readable: false,
            key: Key::new(0, 0),
        };
        assert_eq!(dir.path(), Err(Error::Io { code: 13 }));
    }

    /// Only macOS: Linux opens a directory that this user cannot search with `O_PATH`.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_directory_held_by_its_key_alone_gives_no_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let name = format!("foundation-os-dir-path-closed-{}", std::process::id());
        let scratch = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        fs::create_dir(&scratch).unwrap();
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0)).unwrap();
        let path = Dir::open(CWD, scratch.as_os_str()).unwrap().path();
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(&scratch).unwrap();
        assert_eq!(path, Err(Error::Io { code: 13 }));
    }
}
