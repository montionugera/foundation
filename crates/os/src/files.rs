//! The `env::files` driver on the real disk. One I/O thread runs every call of a
//! disk and its files, in the order they reach its queue.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::fmt;
use std::io::IoSlice;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use block::{Block, Unique};
use env::files::{Descriptor, Error, Mode, Operation, Request};
use env::thread::Handle;
use env::threads::Threads;
use rustix::fs::{self, AtFlags, FileType, FlockOperation, OFlags, RenameFlags};
use rustix::io::{self, Errno};
use tokio::sync::{mpsc, oneshot};

/// The most parts that one `pwritev` takes: `IOV_MAX` on Linux and macOS.
const PARTS: usize = 1_024;

/// The calls that the queue of the I/O thread holds. A call past them waits for room,
/// so a stalled disk does not grow the queue.
const DEPTH: usize = 64;

/// The mode of a new file: it gives the group and other users no access.
const FILE: fs::Mode = fs::Mode::from_raw_mode(0o600);

/// The mode of a new directory: it gives the group and other users no access.
const DIR: fs::Mode = fs::Mode::from_raw_mode(0o700);

/// The flags that open a directory to read.
const READ_DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);

/// The real disk under one data directory: the [`env::files::Driver`] that
/// [`files`](crate::files) gives. It is `Send`, so one thread can make it and another
/// wrap it with [`env::files::Files::new`].
pub struct Disk {
    /// The data directory.
    data: Arc<OwnedFd>,
    queue: Queue,
}

impl Disk {
    /// Opens or makes `dir` and `dir/data`, and starts I/O thread `name`.
    pub(crate) fn new(
        dir: &Path,
        threads: &Threads,
        name: &str,
    ) -> Result<(Self, Handle), crate::Error> {
        Self::start(data(dir), threads, name)
    }

    /// Opens `dir/data`, which must be there, and starts I/O thread `name`.
    pub(crate) fn stored(
        dir: &Path,
        threads: &Threads,
        name: &str,
    ) -> Result<(Self, Handle), crate::Error> {
        let data = fs::open(dir.join("data"), READ_DIR, fs::Mode::empty());
        Self::start(data, threads, name)
    }

    /// Starts I/O thread `name` on the data directory `data`.
    fn start(
        data: io::Result<OwnedFd>,
        threads: &Threads,
        name: &str,
    ) -> Result<(Self, Handle), crate::Error> {
        let data = data.map_err(|errno| crate::Error::Dir(errno.into()))?;
        let (queue, thread) =
            Queue::start(threads, name).map_err(crate::Error::Thread)?;
        let disk = Self {
            data: Arc::new(data),
            queue,
        };
        Ok((disk, thread))
    }

    /// Runs `call` on the data directory and `path` on the I/O thread, as
    /// `operation`.
    fn call<T: Send + 'static>(
        &self,
        path: &Path,
        operation: Operation,
        call: impl FnOnce(&OwnedFd, &Path) -> io::Result<T> + Send + 'static,
    ) -> Request<'_, T> {
        let (data, path) = (Arc::clone(&self.data), path.to_path_buf());
        Box::pin(
            self.queue
                .run(move || call(&data, &path).map_err(fail(&path, operation))),
        )
    }
}

impl fmt::Debug for Disk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Disk").finish_non_exhaustive()
    }
}

impl env::files::Driver for Disk {
    fn open<'a>(
        &'a self,
        path: &'a Path,
        mode: Mode,
    ) -> Request<'a, Box<dyn Descriptor>> {
        let (data, owned) = (Arc::clone(&self.data), path.to_path_buf());
        let opened = self.queue.run(move || open(&data, &owned, mode));
        let (data, queue) = (Arc::clone(&self.data), self.queue.clone());
        Box::pin(async move {
            let (fd, len) = opened.await?;
            let file: Box<dyn Descriptor> = Box::new(File {
                fd: Arc::new(fd),
                data,
                len,
                path: RefCell::new(Arc::from(path)),
                queue,
            });
            Ok(file)
        })
    }

    /// `openat` ignores the data directory for an absolute path.
    fn open_absolute<'a>(&'a self, path: &'a Path) -> Request<'a, Box<dyn Descriptor>> {
        self.open(path, Mode::Read)
    }

    fn list<'a>(&'a self, dir: &'a Path) -> Request<'a, Vec<PathBuf>> {
        self.call(dir, Operation::List, list)
    }

    fn create_dir<'a>(&'a self, dir: &'a Path) -> Request<'a, ()> {
        self.call(dir, Operation::CreateDir, create_dir)
    }

    fn remove<'a>(&'a self, path: &'a Path) -> Request<'a, ()> {
        self.call(path, Operation::Remove, |data, path| {
            fs::unlinkat(data, path, AtFlags::empty())
        })
    }

    fn sync_dir<'a>(&'a self, dir: &'a Path) -> Request<'a, ()> {
        self.call(dir, Operation::SyncDir, |data, dir| {
            fs::openat(data, at(dir), READ_DIR, fs::Mode::empty())
                .and_then(|fd| sync_all(fd.as_fd()))
        })
    }

    fn free(&self) -> Request<'_, u64> {
        self.call(Path::new(""), Operation::Free, |data, _| {
            fs::fstatvfs(data).map(|stat| stat.f_bavail * stat.f_frsize)
        })
    }
}

/// One open file. Each call keeps the file open until it ends.
struct File {
    fd: Arc<Opened>,
    /// The data directory, which a rename acts in.
    data: Arc<OwnedFd>,
    len: u64,
    /// The path of the file now: a rename changes it.
    path: RefCell<Arc<Path>>,
    queue: Queue,
}

impl File {
    /// Runs `call` on the file on the I/O thread, as `operation`.
    fn call<T: Send + 'static>(
        &self,
        operation: Operation,
        call: impl FnOnce(BorrowedFd<'_>) -> io::Result<T> + Send + 'static,
    ) -> Request<'_, T> {
        let (fd, path) = (Arc::clone(&self.fd), Arc::clone(&self.path.borrow()));
        Box::pin(
            self.queue
                .run(move || call(fd.as_fd()).map_err(fail(&path, operation))),
        )
    }
}

impl Descriptor for File {
    fn len(&self) -> u64 {
        self.len
    }

    fn write_at<'a>(&'a self, offset: u64, parts: &'a [Block]) -> Request<'a, ()> {
        let parts = parts.to_vec();
        self.call(Operation::WriteAt, move |fd| write(fd, offset, &parts))
    }

    fn read_at(&self, offset: u64, into: Unique) -> Request<'_, Unique> {
        self.call(Operation::ReadAt, move |fd| {
            let mut into = into;
            read(fd, offset, &mut into)?;
            Ok(into)
        })
    }

    fn sync(&self) -> Request<'_, ()> {
        self.call(Operation::Sync, sync_data)
    }

    fn rename<'a>(&'a self, from: &'a Path, to: &'a Path) -> Request<'a, ()> {
        let (fd, data) = (Arc::clone(&self.fd), Arc::clone(&self.data));
        let (old, new) = (from.to_path_buf(), to.to_path_buf());
        let renamed = self
            .queue
            .run(move || rename(fd.as_fd(), &data, &old, &new));
        Box::pin(async move {
            renamed.await?;
            *self.path.borrow_mut() = Arc::from(to);
            Ok(())
        })
    }

    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()>>> {
        let Self { fd, queue, .. } = *self;
        // The calls before this one have ended, so this drop closes the file.
        Box::pin(async move { queue.run(move || drop(fd)).await })
    }

    fn remove(
        self: Box<Self>,
        path: PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
        let Self {
            fd, data, queue, ..
        } = *self;
        // The drop releases the lock after the unlink, so the lock holds until then.
        Box::pin(async move {
            queue
                .run(move || {
                    let removed = remove(fd.as_fd(), &data, &path);
                    drop(fd);
                    removed
                })
                .await
        })
    }
}

/// A call that the I/O thread runs.
type Job = Box<dyn FnOnce() + Send>;

/// The queue of one I/O thread, which ends after the last clone drops.
#[derive(Clone)]
struct Queue(mpsc::Sender<Job>);

impl Queue {
    /// Starts I/O thread `name` and gives its queue and its handle.
    fn start(
        threads: &Threads,
        name: &str,
    ) -> Result<(Self, Handle), env::thread::Error> {
        let (jobs, mut queued) = mpsc::channel::<Job>(DEPTH);
        let thread = threads.start(name, move || async move {
            while let Some(job) = queued.recv().await {
                job();
            }
        })?;
        Ok((Self(jobs), thread))
    }

    /// Runs `call` after every call that the queue took before it. The queue takes
    /// `call` when it has room and the task has Tokio budget left. After that, `call`
    /// runs also when the future drops; before that, a drop stops it. This order makes
    /// a write open, a remove, a rename, a list, and a `free` wait for each call that a
    /// drop left to run, as `env::files` requires.
    ///
    /// # Panics
    ///
    /// When a panic ended the I/O thread.
    async fn run<T: Send + 'static>(
        &self,
        call: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (reply, result) = oneshot::channel();
        // A dropped future does not want the result.
        let job: Job = Box::new(move || drop(reply.send(call())));
        let ended = "a panic ended the I/O thread of os::files";
        self.0.send(job).await.expect(ended);
        result.await.expect(ended)
    }
}

/// The descriptor of an open file. Its drop releases the lock of a write open before
/// it closes the descriptor: a `flock` lock lasts until each copy of the descriptor
/// closes, and a child of the process holds a copy from its fork to its exec, or to its
/// end. An unlock that the filesystem fails, as NFS or a FUSE daemon can, leaves the
/// lock to the close.
struct Opened {
    fd: OwnedFd,
    locked: bool,
}

impl Opened {
    /// Takes the exclusive lock of the file, or gives `WOULDBLOCK` when another
    /// descriptor holds it.
    fn lock(&mut self) -> io::Result<()> {
        fs::flock(&self.fd, FlockOperation::NonBlockingLockExclusive)?;
        self.locked = true;
        Ok(())
    }
}

impl AsFd for Opened {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        if self.locked
            && let Err(e @ (Errno::BADF | Errno::INVAL)) =
                fs::flock(&self.fd, FlockOperation::Unlock)
        {
            panic!("invariant: the lock of {:?} is released: {e:?}", self.fd);
        }
    }
}

/// Opens the file at `path`, and gives it and its length. A write open locks the
/// file first, so a held file gives `Busy` before any other check.
fn open(data: &OwnedFd, path: &Path, mode: Mode) -> Result<(Opened, u64), Error> {
    let failed = fail(path, Operation::Open);
    let flags = match mode {
        Mode::Read => OFlags::RDONLY,
        Mode::Write => OFlags::RDWR,
        Mode::Create { .. } => OFlags::RDWR.union(OFlags::CREATE),
    };
    let (fd, stat) = loop {
        let mut fd = Opened {
            fd: fs::openat(data, path, flags.union(OFlags::CLOEXEC), FILE)
                .map_err(&failed)?,
            locked: false,
        };
        if mode != Mode::Read {
            match fd.lock() {
                Err(Errno::WOULDBLOCK) => {
                    return Err(Error::Busy {
                        path: path.to_path_buf(),
                    });
                }
                locked => locked.map_err(&failed)?,
            }
        }
        let stat = fs::fstat(&fd).map_err(&failed)?;
        let flags = AtFlags::empty();
        if mode == Mode::Read || named(data, path, flags, &stat).map_err(&failed)? {
            break (fd, stat);
        }
    };
    if FileType::from_raw_mode(stat.st_mode).is_dir() {
        return Err(failed(Errno::ISDIR));
    }
    let found = stat.st_size.cast_unsigned();
    match mode {
        // Not atomic: a crash before `allocate` sets the length leaves an empty file,
        // which this allocates.
        Mode::Create { len } if found == 0 && len != 0 => {
            allocate(fd.as_fd(), len)
                .and_then(|()| sync_all(fd.as_fd()))
                // A child process can hold a copy of `fd`, so the blocks are freed
                // here, not at its close. A failed create leaves no file, so a later
                // write open gives `NotFound`, not an empty file.
                .or_else(|errno| {
                    let freed = fs::ftruncate(&fd, 0);
                    fs::unlinkat(data, path, AtFlags::empty())?;
                    freed.and(Err(errno))
                })
                .map_err(&failed)?;
            Ok((fd, len))
        }
        _ => Ok((fd, found)),
    }
}

/// Opens `dir/data`, and first makes `dir` and `dir/data` when they are not there.
fn data(dir: &Path) -> io::Result<OwnedFd> {
    let dir = make(fs::CWD, dir)?;
    make(&dir, Path::new("data"))
}

/// Opens directory `path` of `at`, first makes it when it is not there, and syncs the
/// directory that holds it, unless `path` is on another device than its holder.
fn make(at: impl AsFd, path: &Path) -> io::Result<OwnedFd> {
    match fs::mkdirat(&at, path, DIR) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(errno) => return Err(errno),
    }
    let made = fs::openat(&at, path, READ_DIR, fs::Mode::empty())?;
    let holder = fs::openat(&made, "..", READ_DIR, fs::Mode::empty())?;
    // An entry made by this or an earlier call is not durable until its holder syncs.
    // `mkdirat` makes no entry on another device: that is a mount point, the mount's
    // to keep.
    if fs::fstat(&holder)?.st_dev == fs::fstat(&made)?.st_dev {
        sync_all(holder.as_fd())?;
    }
    Ok(made)
}

/// The names in `dir`, without `.` and `..`.
fn list(data: &OwnedFd, dir: &Path) -> io::Result<Vec<PathBuf>> {
    let fd = fs::openat(data, at(dir), READ_DIR, fs::Mode::empty())?;
    let mut names = Vec::new();
    for entry in fs::Dir::new(fd)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(PathBuf::from(OsStr::from_bytes(name)));
        }
    }
    Ok(names)
}

/// Makes the directory `dir`, or gives `Ok` when a directory is there.
fn create_dir(data: &OwnedFd, dir: &Path) -> io::Result<()> {
    match fs::mkdirat(data, at(dir), DIR) {
        Err(Errno::EXIST)
            if fs::statat(data, at(dir), AtFlags::empty())
                .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode).is_dir()) =>
        {
            Ok(())
        }
        made => made,
    }
}

/// Writes `parts` back to back at `offset`, in as many calls as the OS needs.
fn write(fd: BorrowedFd<'_>, mut offset: u64, parts: &[Block]) -> io::Result<()> {
    let mut slices: Vec<IoSlice<'_>> =
        parts.iter().map(|part| IoSlice::new(part)).collect();
    let mut rest = &mut slices[..];
    while !rest.is_empty() {
        let written = io::pwritev(fd, &rest[..rest.len().min(PARTS)], offset)?;
        offset += written as u64;
        IoSlice::advance_slices(&mut rest, written);
    }
    Ok(())
}

/// Fills `into` from `offset`. A file that something outside Foundation cut short
/// gives `EIO`.
fn read(fd: BorrowedFd<'_>, offset: u64, into: &mut [u8]) -> io::Result<()> {
    let mut done = 0;
    while done < into.len() {
        match io::pread(fd, &mut into[done..], offset + done as u64)? {
            0 => return Err(Errno::IO),
            read => done += read,
        }
    }
    Ok(())
}

/// Makes the bytes of `fd` durable, through the cache of the disk.
fn sync_data(fd: BorrowedFd<'_>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return fs::fdatasync(fd);
    #[cfg(target_os = "macos")]
    return fs::fcntl_fullfsync(fd);
}

/// Makes the bytes and the metadata of `fd` durable, through the cache of the disk.
fn sync_all(fd: BorrowedFd<'_>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return fs::fsync(fd);
    #[cfg(target_os = "macos")]
    return fs::fcntl_fullfsync(fd);
}

/// Renames `from` to `to` when `from` names the file `fd`, with no replace. The I/O
/// thread of a shard runs its calls in order, and each shard writes only its own
/// directory, so nothing changes `from` between the check and the rename.
fn rename(
    fd: BorrowedFd<'_>,
    data: &OwnedFd,
    from: &Path,
    to: &Path,
) -> Result<(), Error> {
    let failed = fail(from, Operation::Rename);
    check_named(fd, data, from, &failed)?;
    match fs::renameat_with(data, from, data, to, RenameFlags::NOREPLACE) {
        Err(Errno::EXIST) => Err(Error::Exists {
            path: to.to_path_buf(),
        }),
        renamed => renamed.map_err(&failed),
    }
}

/// Unlinks `path` when it names the file `fd`, as [`rename`] checks it.
fn remove(fd: BorrowedFd<'_>, data: &OwnedFd, path: &Path) -> Result<(), Error> {
    let failed = fail(path, Operation::Remove);
    check_named(fd, data, path, &failed)?;
    fs::unlinkat(data, path, AtFlags::empty()).map_err(&failed)
}

/// `NotFound` from `failed` unless `path`, a link itself and not what it points at,
/// names the file `fd`.
fn check_named(
    fd: BorrowedFd<'_>,
    data: &OwnedFd,
    path: &Path,
    failed: &impl Fn(Errno) -> Error,
) -> Result<(), Error> {
    let stat = fs::fstat(fd).map_err(failed)?;
    let flags = AtFlags::SYMLINK_NOFOLLOW;
    if named(data, path, flags, &stat).map_err(failed)? {
        Ok(())
    } else {
        Err(failed(Errno::NOENT))
    }
}

/// Whether `path`, looked up by `flags`, names the file of `stat`. A failed create of
/// another handle unlinks its file, also after a write open found it and before that
/// open locked it.
fn named(
    data: &OwnedFd,
    path: &Path,
    flags: AtFlags,
    stat: &fs::Stat,
) -> io::Result<bool> {
    match fs::statat(data, path, flags) {
        Ok(found) => Ok((found.st_dev, found.st_ino) == (stat.st_dev, stat.st_ino)),
        Err(Errno::NOENT) => Ok(false),
        Err(errno) => Err(errno),
    }
}

/// Allocates the first `len` bytes of the empty file `fd` on disk, all or none, and
/// sets its length to `len`.
fn allocate(fd: BorrowedFd<'_>, len: u64) -> io::Result<()> {
    // The length stays 0 until each block is there, also after a crash. The first
    // `ftruncate` frees the blocks that such a crash left past the end.
    #[cfg(target_os = "linux")]
    return fs::ftruncate(fd, 0)
        .and_then(|()| fs::fallocate(fd, fs::FallocateFlags::KEEP_SIZE, 0, len))
        .and_then(|()| fs::ftruncate(fd, len));
    #[cfg(target_os = "macos")]
    return crate::allocate::all(fd, len);
}

/// The path of `path` from the data directory, where an empty path is the data
/// directory itself.
fn at(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}

/// The error of `operation` on `path` for an OS error code. Only an operation that
/// takes space gives `Full`.
fn fail(path: &Path, operation: Operation) -> impl Fn(Errno) -> Error + '_ {
    let grows = matches!(
        operation,
        Operation::Open | Operation::CreateDir | Operation::WriteAt
    );
    move |errno| match errno {
        Errno::NOENT => Error::NotFound {
            path: path.to_path_buf(),
        },
        Errno::NOSPC | Errno::DQUOT if grows => Error::Full {
            path: path.to_path_buf(),
        },
        _ => Error::Io {
            path: path.to_path_buf(),
            operation,
            code: errno.raw_os_error(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::task::{Context, Waker};

    use env::files::Driver as _;

    use super::*;

    /// The OS cannot sync a pipe.
    #[cfg(target_os = "linux")]
    const PIPE: Errno = Errno::INVAL;
    #[cfg(target_os = "macos")]
    const PIPE: Errno = Errno::BADF;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    /// Queues a call that holds the I/O thread. Gives its future after the thread
    /// starts it, and the sender that ends it.
    #[expect(clippy::disallowed_methods, reason = "the test holds the I/O thread")]
    fn hold(queue: &Queue) -> (Pin<Box<impl Future<Output = ()>>>, Sender<()>) {
        let mut context = Context::from_waker(Waker::noop());
        let (started, running) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let mut call = Box::pin(queue.run(move || {
            started.send(()).unwrap();
            released.recv().unwrap();
        }));
        assert!(call.as_mut().poll(&mut context).is_pending());
        running.recv().unwrap();
        (call, release)
    }

    #[test]
    fn a_sync_reaches_the_os() {
        let (pipe, _writer) = std::io::pipe().unwrap();
        let pipe = OwnedFd::from(pipe);
        assert_eq!(sync_data(pipe.as_fd()), Err(PIPE));
        assert_eq!(sync_all(pipe.as_fd()), Err(PIPE));
    }

    /// `/sys/fs/cgroup` is a mount point under sysfs, which cannot sync a directory.
    /// Runs without root, on a holder that cannot sync (`EINVAL`), the case that the
    /// root test of `os::files` does not reach.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs sysfs")]
    fn make_of_a_mount_point_does_not_sync_its_holder() {
        let point = Path::new("/sys/fs/cgroup");
        let device = |path: &str| fs::stat(path).unwrap().st_dev;
        assert_ne!(device("/sys/fs"), device("/sys/fs/cgroup"), "a mount point");
        let holder = fs::open("/sys/fs", READ_DIR, fs::Mode::empty()).unwrap();
        assert_eq!(sync_all(holder.as_fd()), Err(Errno::INVAL));
        make(fs::CWD, point).unwrap();
    }

    #[test]
    fn a_full_disk_gives_full_where_an_operation_takes_space() {
        let full = Error::Full { path: "a".into() };
        for operation in [Operation::Open, Operation::CreateDir, Operation::WriteAt] {
            let fail = fail(Path::new("a"), operation);
            assert_eq!(fail(Errno::NOSPC), full);
            assert_eq!(fail(Errno::DQUOT), full);
        }
        let fail = fail(Path::new("a"), Operation::Sync);
        let io = |code| Error::Io {
            path: "a".into(),
            operation: Operation::Sync,
            code,
        };
        assert_eq!(fail(Errno::NOSPC), io(Errno::NOSPC.raw_os_error()));
        assert_eq!(fail(Errno::DQUOT), io(Errno::DQUOT.raw_os_error()));
        assert_eq!(fail(Errno::NOENT), Error::NotFound { path: "a".into() });
        assert_eq!(fail(Errno::IO), io(5));
    }

    #[test]
    fn a_call_past_the_depth_waits_outside_the_queue() {
        let (queue, thread) =
            Queue::start(&crate::threads().unwrap(), "files").unwrap();
        let mut context = Context::from_waker(Waker::noop());
        let (held, release) = hold(&queue);
        let ran = Arc::new(AtomicUsize::new(0));
        let mut calls: Vec<_> = (0..=DEPTH)
            .map(|_| {
                let ran = Arc::clone(&ran);
                Box::pin(queue.run(move || ran.fetch_add(1, Ordering::Relaxed)))
            })
            .collect();
        for call in &mut calls {
            assert!(call.as_mut().poll(&mut context).is_pending());
        }
        drop(calls);
        release.send(()).unwrap();
        runtime().block_on(async {
            held.await;
            queue.run(|| ()).await;
        });
        assert_eq!(ran.load(Ordering::Relaxed), DEPTH);
        drop(queue);
        thread.join().unwrap();
    }

    #[test]
    fn close_ends_after_the_queued_calls_and_closes_the_file() {
        let dir = std::env::temp_dir()
            .join(format!("foundation-os-files-close-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let threads = crate::threads().unwrap();
        let (disk, thread) = Disk::new(&dir, &threads, "files").unwrap();
        let runtime = runtime();
        let file = runtime
            .block_on(disk.open(Path::new("a"), Mode::Create { len: 4_096 }))
            .unwrap();
        let mut context = Context::from_waker(Waker::noop());
        let (held, release) = hold(&disk.queue);
        let parts: [Block; 0] = [];
        let mut write = file.write_at(0, &parts);
        assert!(write.as_mut().poll(&mut context).is_pending());
        drop(write);
        let mut close = file.close();
        assert!(close.as_mut().poll(&mut context).is_pending());
        release.send(()).unwrap();
        runtime.block_on(async {
            held.await;
            close.await;
        });
        let other = fs::open(dir.join("data/a"), OFlags::RDWR, fs::Mode::empty());
        let lock = FlockOperation::NonBlockingLockExclusive;
        fs::flock(other.unwrap(), lock).unwrap();
        drop(disk);
        thread.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
