//! A bench for process tests: one `foundation` node in a temporary directory. Each
//! method with a `todo!` waits on the issue it names.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use env::clock::Clock;
use types::time::Span;

use crate::status::Connector;

/// How long [`Rig::wait`] and [`Rig::run`] wait: longer than the 60 s cap of a restart
/// backoff.
const PATIENCE: Span = Span::from_nanos(90_000_000_000);

/// The time between two checks of a wait.
const POLL: Duration = Duration::from_millis(100);

/// The pool budget that [`Rig::new`] keeps.
pub(crate) const POOL: u64 = 1 << 30;

/// The start of the name of each directory that [`Rig::new`] makes.
const PREFIX: &str = "foundation-node-";

/// One `foundation` node in a temporary directory of its own. Drop kills the node and
/// removes the directory. A failed test keeps its directory until the next run on Unix:
/// [`Rig::new`] removes each directory of a test process that is gone.
#[derive(Debug)]
pub(crate) struct Rig {
    /// The working directory of each command. It holds `plant.hcl`.
    pub(crate) dir: PathBuf,
    /// The disk budget that [`Rig::new`] keeps: 8 MiB for each core.
    pub(crate) disk: u64,
    clock: Clock,
    node: Option<Running>,
}

impl Rig {
    /// Makes a temporary directory for the test on this thread, with a name that no
    /// directory has: a run that was killed keeps its directory, and a later run can
    /// get the same PID. On Unix, first removes the directory of each test process of
    /// this user that is gone, also when a node that it started still runs there. A
    /// directory in which such a node makes a file during the removal stays until a
    /// later rig, and one whose PID the OS gave to another process stays while that
    /// process runs. Its data directory keeps a pool budget of 1 GiB and a disk budget of
    /// 8 MiB for each core ([`Rig::keep`]), so a first start does not take a quarter of
    /// the host's free disk, and each shard's ring fits.
    pub(crate) fn new() -> Self {
        Rig::with_cores(os::shards().expect("read the cores").cores().get())
    }

    /// [`Rig::new`] for a host of `cores` cores.
    fn with_cores(cores: usize) -> Self {
        let thread = std::thread::current();
        let test = thread.name().expect("invariant: libtest names the thread");
        let name = format!("{PREFIX}{}-{test}", std::process::id());
        let name = name.replace("::", "-");
        let mut n = 0;
        let dir = loop {
            let dir = std::env::temp_dir().join(format!("{name}-{n}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => n += 1,
                Err(error) => panic!("make {}: {error}", dir.display()),
            }
        };
        #[cfg(unix)]
        sweep(&dir, remove);
        let rig = Self {
            dir,
            disk: u64::try_from(cores).expect("invariant: a core count fits u64")
                * (8 << 20),
            clock: os::clock(),
            node: None,
        };
        rig.keep(POOL, rig.disk);
        rig
    }

    /// The path of the file `budget` of the data directory `foundation-data`.
    pub(crate) fn budget(&self) -> PathBuf {
        self.dir.join("foundation-data/data/budget")
    }

    /// Writes the file [`Rig::budget`] that keeps `pool` and `disk`.
    pub(crate) fn keep(&self, pool: u64, disk: u64) {
        keep(&self.budget(), pool, disk);
    }

    /// Writes `hcl` to `plant.hcl`.
    pub(crate) fn config(&self, hcl: &str) {
        std::fs::write(self.dir.join("plant.hcl"), hcl).expect("write plant.hcl");
    }

    /// Starts `foundation start --name edge`, with each listener of the node on port 0
    /// of loopback, and waits until it prints that the node runs.
    pub(crate) fn start(&mut self) {
        self.start_with(&["--name", "edge"]);
    }

    /// [`Rig::start`] with `args` in place of `--name edge`. Panics when a node runs,
    /// when the node exits before it prints a line, or when 90 s pass first.
    pub(crate) fn start_with(&mut self, args: &[&str]) {
        assert!(self.node.is_none(), "a node runs");
        let command = self.starting(&[&["start"], args].concat());
        // The node reads no input.
        let (mut node, _) = Running::new(command);
        let started = poll(&self.clock, PATIENCE, || match node.ended() {
            Some(status) => Ok(Some(status)),
            None if node.stdout.text().contains('\n') => Ok(None),
            None => Err(node.seen()),
        });
        match started {
            Ok(None) => self.node = Some(node),
            Ok(Some(status)) => {
                panic!("the node exited with {status}:\n{}", node.seen())
            }
            Err(seen) => {
                node.process.end();
                late("the node prints that it runs", PATIENCE, &seen)
            }
        }
    }

    /// Stops the node of [`Rig::start`] with SIGTERM, and gives its output when it
    /// exits and closes its pipes. Panics when no node runs, or when 90 s pass first.
    #[cfg(unix)]
    pub(crate) fn stop(&mut self) -> Output {
        let mut node = self.node.take().expect("a node runs");
        node.process.term();
        let ended = poll(&self.clock, PATIENCE, || {
            node.ended().ok_or_else(|| node.seen())
        });
        match ended {
            Ok(status) => node.output(status),
            Err(seen) => {
                node.process.end();
                late("the node exits at SIGTERM", PATIENCE, &seen)
            }
        }
    }

    /// Starts `foundation` with `args` in [`Rig::dir`], with `stdout` as its standard
    /// output, no standard input, and its standard error on a pipe that
    /// [`Process::errors`] reads. Waits for nothing.
    #[cfg(target_os = "linux")]
    pub(crate) fn spawn(&self, args: &[&str], stdout: Stdio) -> Process {
        let mut command = self.starting(args);
        command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::piped());
        Process(command.spawn().expect("start the command"))
    }

    /// [`Rig::command`] for a command that starts a node that runs. When `args` hold
    /// `--data` and a path after it, and that data directory holds no budget file,
    /// first writes the one of [`Rig::new`] there. Panics on `--data=<path>`, which it
    /// does not read.
    fn starting(&self, args: &[&str]) -> Command {
        let joined = args.iter().find(|arg| arg.starts_with("--data="));
        assert!(joined.is_none(), "give `--data <path>`, not {joined:?}");
        let data = (args.windows(2)).find_map(|pair| match pair {
            ["--data", data] => Some(*data),
            _ => None,
        });
        if let Some(data) = data {
            let path = self.dir.join(data).join("data/budget");
            if !path.exists() {
                keep(&path, POOL, self.disk);
            }
        }
        self.command(args)
    }

    /// `foundation` with `args` in [`Rig::dir`].
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_foundation"));
        command.args(args).current_dir(&self.dir);
        command
    }

    /// Runs `foundation` with `args` in [`Rig::dir`], with `input` on its standard
    /// input, and gives its output when it exits and closes its pipes. When 90 s pass
    /// first, kills it and panics with the output so far. It writes no budget file, so
    /// a start on a `--data` path through it must fail before the shards open.
    pub(crate) fn run(&self, args: &[&str], input: &[u8]) -> Output {
        run(&self.clock, PATIENCE, self.command(args), input)
    }

    /// [`Rig::run`] under `taskset`, on the first CPU of the test's affinity set, so
    /// the node runs one shard on each host. Linux only: it reads `/proc`.
    pub(crate) fn run_on_one_core(&self, args: &[&str], input: &[u8]) -> Output {
        let command = self.on_one_core(Command::new("taskset"), args);
        run(&self.clock, PATIENCE, command, input)
    }

    /// [`Rig::run_on_one_core`] under `prlimit`, which limits the address space of the
    /// node to `space` bytes.
    pub(crate) fn run_on_one_core_in(
        &self,
        space: u64,
        args: &[&str],
        input: &[u8],
    ) -> Output {
        let mut prlimit = Command::new("prlimit");
        prlimit.args([&format!("--as={space}"), "--", "taskset"]);
        run(
            &self.clock,
            PATIENCE,
            self.on_one_core(prlimit, args),
            input,
        )
    }

    /// Gives `taskset` in `command` the first CPU of the test's affinity set,
    /// `foundation`, and `args`.
    fn on_one_core(&self, mut command: Command, args: &[&str]) -> Command {
        let status = std::fs::read_to_string("/proc/self/status").expect("read status");
        let cpus = (status.lines())
            .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
            .expect("a list of the allowed CPUs");
        let first = (cpus.trim().split([',', '-']).next()).expect("one CPU at least");
        command
            .args(["-c", first, env!("CARGO_BIN_EXE_foundation")])
            .args(args)
            .current_dir(&self.dir);
        command
    }

    /// Starts `foundation` with `args` in [`Rig::dir`], for a test that writes its
    /// standard input while it runs.
    pub(crate) fn talk(&self, args: &[&str]) -> Talk {
        let (running, stdin) = Running::new(self.command(args));
        Talk {
            running,
            stdin,
            clock: self.clock.clone(),
        }
    }

    /// Calls `check` until it gives `Ok`, and gives that value. When 90 s pass first,
    /// panics with `what` and the last `Err`: the state that `check` saw.
    pub(crate) fn wait<T>(
        &self,
        what: &str,
        check: impl FnMut() -> Result<T, String>,
    ) -> T {
        wait(&self.clock, PATIENCE, what, check)
    }

    /// Plans `plant.hcl` into `plant.plan`, and applies that plan. Panics when either
    /// command does not exit 0.
    #[expect(clippy::todo, reason = "waits on #337, #1744")]
    pub(crate) fn apply(&self) {
        todo!("waits on #337, #1744")
    }

    /// The connectors of `foundation status --json`, by name.
    #[expect(clippy::todo, reason = "waits on #1735")]
    pub(crate) fn status(&self) -> BTreeMap<String, Connector> {
        todo!("waits on #1735")
    }
}

/// The bytes of a budget file as a node keeps its budgets: the tag, `pool` and `disk`
/// (little-endian), and the CRC32C of those 35 bytes.
pub(crate) fn budget(pool: u64, disk: u64) -> Vec<u8> {
    let mut bytes = b"foundation/budget/1".to_vec();
    bytes.extend(pool.to_le_bytes());
    bytes.extend(disk.to_le_bytes());
    bytes.extend(crc32c::crc32c(&bytes).to_le_bytes());
    bytes
}

/// Writes the [`budget`] of `pool` and `disk` at `path`, with its directories.
fn keep(path: &Path, pool: u64, disk: u64) {
    let data = path.parent().expect("invariant: a file of a directory");
    std::fs::create_dir_all(data).expect("make the data directory");
    std::fs::write(path, budget(pool, disk)).expect("write the budget file");
}

/// Removes with `remove` each directory `foundation-node-<pid>-*` beside `made` that
/// the owner of `made` owns and whose PID names no process. The files of a node that
/// still runs in one stay on the disk until that node ends.
#[cfg(unix)]
fn sweep(made: &Path, remove: impl Fn(&Path) -> std::io::Result<()>) {
    use std::os::unix::fs::MetadataExt;

    let temp = made
        .parent()
        .expect("invariant: in the temporary directory");
    let owner = std::fs::metadata(made)
        .expect("read the new directory")
        .uid();
    let mut alive = BTreeMap::new();
    for entry in std::fs::read_dir(temp).expect("read the temporary directory") {
        let entry = entry.expect("read the temporary directory");
        let name = entry.file_name();
        let pid = (name.to_str())
            .and_then(|name| name.strip_prefix(PREFIX))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok());
        let Some(pid) = pid else {
            continue;
        };
        // A test thread of this run or of another run can remove it first.
        let gone = |error: &std::io::Error| error.kind() == ErrorKind::NotFound;
        // A node that the test started can still make files in it. The next sweep
        // removes it.
        let busy =
            |error: &std::io::Error| error.kind() == ErrorKind::DirectoryNotEmpty;
        let path = entry.path();
        let mine = match entry.metadata() {
            Ok(metadata) => metadata.is_dir() && metadata.uid() == owner,
            Err(error) if gone(&error) => continue,
            Err(error) => panic!("read {}: {error}", path.display()),
        };
        if !mine || *alive.entry(pid).or_insert_with(|| runs(pid)) {
            continue;
        }
        match remove(&path) {
            Err(error) if !gone(&error) && !busy(&error) => {
                panic!("remove {}: {error}", path.display())
            }
            _ => {}
        }
    }
}

/// Removes the directory `path` of this user, also when a test that took the write bit
/// of a directory in it ended before it gave it back.
#[cfg(unix)]
fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            writable(path)?;
            std::fs::remove_dir_all(path)
        }
        removed => removed,
    }
}

/// Gives the owner each permission on the directory `path` and each directory in it.
/// Gives `NotFound` only when `path` is gone.
#[cfg(unix)]
fn writable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            match writable(&entry.path()) {
                // Another sweep removed it.
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                walked => walked?,
            }
        }
    }
    Ok(())
}

/// Whether a process with `pid` runs, by `kill -0`.
#[cfg(unix)]
fn runs(pid: u32) -> bool {
    let status = (Command::new("kill"))
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("run kill");
    status.success()
}

/// [`Rig::wait`], with `limit` in place of 90 s.
pub(crate) fn wait<T>(
    clock: &Clock,
    limit: Span,
    what: &str,
    check: impl FnMut() -> Result<T, String>,
) -> T {
    poll(clock, limit, check).unwrap_or_else(|seen| late(what, limit, &seen))
}

/// Panics: `what` did not happen within `limit`, and the last check saw `seen`.
fn late(what: &str, limit: Span, seen: &str) -> ! {
    panic!("{what}: not within {limit}. The last check saw:\n{seen}")
}

/// Gives the first value of `check`, or what its last check saw once `limit` passes.
fn poll<T>(
    clock: &Clock,
    limit: Span,
    mut check: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let deadline = clock.now() + limit;
    loop {
        let seen = match check() {
            Ok(value) => return Ok(value),
            Err(seen) => seen,
        };
        if clock.now() >= deadline {
            return Err(seen);
        }
        #[expect(
            clippy::disallowed_methods,
            reason = "a process test waits on another process in real time"
        )]
        std::thread::sleep(POLL);
    }
}

/// Runs `command` with `input` on its standard input, and gives its output when it
/// exits and closes its pipes. When `limit` passes first, kills it and panics with the
/// output so far.
fn run(clock: &Clock, limit: Span, command: Command, input: &[u8]) -> Output {
    let (mut running, mut stdin) = Running::new(command);
    let input = input.to_vec();
    #[expect(
        clippy::disallowed_methods,
        reason = "a process test writes to another process while it runs"
    )]
    let input = std::thread::spawn(move || match stdin.write_all(&input) {
        // The command closed its standard input: it reads no more.
        Err(error) if error.kind() == ErrorKind::BrokenPipe => {}
        written => written.expect("write the input"),
    });
    running.input = Some(input);
    end(clock, limit, running)
}

/// Gives the output of `running` when it exits and closes its pipes. When `limit`
/// passes first, kills it and panics with the output so far.
fn end(clock: &Clock, limit: Span, mut running: Running) -> Output {
    let ended = poll(clock, limit, || {
        running.ended().ok_or_else(|| running.seen())
    });
    match ended {
        Ok(status) => running.output(status),
        Err(seen) => {
            running.process.end();
            late("the command exits and closes its pipes", limit, &seen)
        }
    }
}

/// A command that a test writes to while it runs: [`Rig::talk`].
#[derive(Debug)]
pub(crate) struct Talk {
    running: Running,
    stdin: ChildStdin,
    clock: Clock,
}

impl Talk {
    /// Writes `line` to the standard input of the command, and waits until all of
    /// its standard output so far is `output`. When 90 s pass first, panics with the
    /// output so far.
    pub(crate) fn ask(&mut self, line: &str, output: &str) {
        self.stdin
            .write_all(line.as_bytes())
            .expect("write the input");
        wait(&self.clock, PATIENCE, "the command answers", || {
            let seen = self.running.stdout.text();
            if seen == output {
                Ok(())
            } else {
                Err(self.running.seen())
            }
        });
    }

    /// Closes the standard input of the command, and gives its output when it exits
    /// and closes its pipes. When 90 s pass first, kills it and panics with the output
    /// so far.
    pub(crate) fn close(self) -> Output {
        drop(self.stdin);
        end(&self.clock, PATIENCE, self.running)
    }
}

/// A command that runs, with a thread for each of its output pipes and for the
/// input that [`run`] writes.
#[derive(Debug)]
struct Running {
    process: Process,
    input: Option<JoinHandle<()>>,
    stdout: Capture,
    stderr: Capture,
}

impl Running {
    /// Starts `command`, and gives it with its standard input.
    fn new(mut command: Command) -> (Self, ChildStdin) {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the command");
        let mut process = Process(child);
        let stdin = process.0.stdin.take().expect("piped");
        let running = Self {
            input: None,
            stdout: Capture::new(process.0.stdout.take().expect("piped")),
            stderr: Capture::new(process.0.stderr.take().expect("piped")),
            process,
        };
        (running, stdin)
    }

    /// The exit status, once the command has exited and each of its pipes has closed.
    fn ended(&mut self) -> Option<ExitStatus> {
        let status = self.process.exited()?;
        let written = self.input.as_ref().is_none_or(JoinHandle::is_finished);
        let closed = written && self.stdout.ended() && self.stderr.ended();
        closed.then_some(status)
    }

    /// What the command has written so far.
    fn seen(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            self.stdout.text(),
            self.stderr.text()
        )
    }

    /// The output of a command that [`Running::ended`] with `status`.
    fn output(self, status: ExitStatus) -> Output {
        if let Some(input) = self.input {
            input.join().expect("write the input");
        }
        Output {
            status,
            stdout: self.stdout.end(),
            stderr: self.stderr.end(),
        }
    }
}

/// The process of a command. Drop kills it, so a test that panics leaves no
/// `foundation` process. A process that it starts lives on: `foundation` starts none.
#[derive(Debug)]
pub(crate) struct Process(Child);

impl Process {
    /// The PID of the command.
    #[cfg(target_os = "linux")]
    pub(crate) fn pid(&self) -> u32 {
        self.0.id()
    }

    /// The standard error of a command that [`Rig::spawn`] started and that exited.
    #[cfg(target_os = "linux")]
    pub(crate) fn errors(&mut self) -> String {
        let mut errors = String::new();
        let mut pipe = self.0.stderr.take().expect("spawn pipes standard error");
        std::io::Read::read_to_string(&mut pipe, &mut errors).expect("read the errors");
        errors
    }

    /// Sends SIGTERM to the command. Panics when `kill` fails.
    #[cfg(unix)]
    pub(crate) fn term(&self) {
        let pid = self.0.id().to_string();
        let sent = Command::new("kill").arg(&pid).status().expect("run kill");
        assert!(sent.success(), "send SIGTERM to {pid}");
    }

    /// The exit status of the command, or `None` while it runs.
    pub(crate) fn exited(&mut self) -> Option<ExitStatus> {
        self.0.try_wait().expect("check the command")
    }

    /// Kills the command and waits for it to exit.
    fn end(&mut self) {
        // `kill` gives `Ok` for a command that exited.
        (self.0.kill())
            .and_then(|()| self.0.wait())
            .expect("kill the command");
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // `kill` gives `Ok` for a command that exited. No wait: while the thread
        // panics, a wait can block, and `run` waits for its command itself.
        match self.0.kill() {
            Err(error) if std::thread::panicking() => {
                let line = format!("kill the command: {error}");
                #[expect(
                    clippy::print_stderr,
                    reason = "only `eprintln!` writes to the output that the harness \
                              captures for the report of the test"
                )]
                match std::panic::catch_unwind(move || eprintln!("{line}")) {
                    Ok(()) => {}
                    // The test fails already, and a panic out of this drop aborts
                    // the test binary.
                    Err(_closed) => {}
                }
            }
            kill => kill.expect("kill the command"),
        }
    }
}

/// One output of a command, which a thread reads as the command writes it.
#[derive(Debug)]
struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<()>,
}

impl Capture {
    fn new(mut from: impl Read + Send + 'static) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let to = Arc::clone(&bytes);
        #[expect(
            clippy::disallowed_methods,
            reason = "a process test reads another process while it runs"
        )]
        let reader = std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            loop {
                match from.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(n) => {
                        to.lock().expect("no panic").extend_from_slice(&chunk[..n]);
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => panic!("read the output: {error}"),
                }
            }
        });
        Self { bytes, reader }
    }

    /// The bytes read so far, as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().expect("no panic")).into_owned()
    }

    /// Whether the output has ended.
    fn ended(&self) -> bool {
        self.reader.is_finished()
    }

    /// All of the output. Blocks until it ends.
    fn end(self) -> Vec<u8> {
        self.reader.join().expect("read the output");
        std::mem::take(&mut *self.bytes.lock().expect("no panic"))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // While the thread panics, a remove can block and a second panic aborts the
        // test binary. The drop of the node kills it.
        if !std::thread::panicking() {
            if let Some(mut node) = self.node.take() {
                node.process.end();
            }
            std::fs::remove_dir_all(&self.dir).expect("remove the directory");
        }
    }
}

#[test]
fn a_rig_takes_a_new_directory_when_its_name_is_in_use() {
    let kept = Rig::new();
    let rig = Rig::new();
    assert_ne!(rig.dir, kept.dir);
}

/// A PID that names no process: the PID of a `true` that ended and was waited for.
#[cfg(unix)]
fn gone() -> u32 {
    let mut gone = Command::new("true").spawn().expect("start true");
    gone.wait().expect("wait for true");
    gone.id()
}

/// The path of the temporary directory `name` of this run, which a sweep keeps while
/// the run lives and removes once it is gone.
#[cfg(unix)]
fn draft(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{PREFIX}{}-{name}", std::process::id()))
}

/// Makes the directory `foundation-node-<pid>-<test>-0` with the data directory that
/// `fill` fills. A sweep on another test thread sees it only when it is complete.
#[cfg(unix)]
fn create_left(pid: u32, test: &str, fill: impl FnOnce(&Path)) -> PathBuf {
    let temp = std::env::temp_dir();
    let draft = draft(&format!("left-{pid}-{test}"));
    let data = draft.join("foundation-data/data");
    std::fs::create_dir_all(&data).expect("make the directory");
    fill(&data);
    let dir = temp.join(format!("{PREFIX}{pid}-{test}-0"));
    std::fs::rename(draft, &dir).expect("move the directory");
    dir
}

/// No test checks that the sweep keeps a directory of another user: that needs one.
#[test]
#[cfg(unix)]
fn a_rig_removes_the_directory_of_a_test_process_that_is_gone() {
    let sleep = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("start sleep");
    let sleep = Process(sleep);
    let dead = create_left(gone(), "sweep", |_| {});
    let own = create_left(std::process::id(), "sweep", |_| {});
    let other = create_left(sleep.0.id(), "sweep", |_| {});
    let _rig = Rig::new();
    assert!(!dead.exists(), "{} is still there", dead.display());
    for live in [own, other] {
        assert!(live.exists(), "{} is gone", live.display());
        std::fs::remove_dir_all(live).expect("remove the directory");
    }
}

#[test]
#[cfg(unix)]
fn a_rig_removes_a_directory_that_a_test_left_read_only() {
    use std::os::unix::fs::PermissionsExt;

    let dead = create_left(gone(), "read-only", |data| {
        keep(&data.join("budget"), POOL, 1);
        std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o555))
            .expect("make it read-only");
    });
    let _rig = Rig::new();
    assert!(!dead.exists(), "{} is still there", dead.display());
}

/// A sweep that fails to remove a directory because a process still makes files in it
/// leaves the directory for a later sweep.
#[test]
#[cfg(unix)]
fn a_sweep_passes_over_a_directory_that_a_process_writes_in() {
    // In the rig's directory, which no other sweep reads.
    let rig = Rig::new();
    let made = rig
        .dir
        .join(format!("{PREFIX}{}-made-0", std::process::id()));
    let dead = rig.dir.join(format!("{PREFIX}{}-busy-0", gone()));
    for dir in [&made, &dead] {
        std::fs::create_dir(dir).expect("make the directory");
    }
    sweep(&made, |_| Err(ErrorKind::DirectoryNotEmpty.into()));
    assert!(dead.exists(), "{} is gone", dead.display());
    sweep(&made, remove);
    assert!(!dead.exists(), "{} is still there", dead.display());
}

/// A sweep passes over a directory that is gone before its remove, and panics on any
/// other error of the remove.
#[test]
#[cfg(unix)]
fn a_sweep_panics_when_a_remove_fails_with_another_error() {
    let rig = Rig::new();
    let made = rig
        .dir
        .join(format!("{PREFIX}{}-made-0", std::process::id()));
    let dead = rig.dir.join(format!("{PREFIX}{}-denied-0", gone()));
    for dir in [&made, &dead] {
        std::fs::create_dir(dir).expect("make the directory");
    }
    sweep(&made, |_| Err(ErrorKind::NotFound.into()));
    let swept = std::panic::AssertUnwindSafe(|| {
        sweep(&made, |_| Err(ErrorKind::PermissionDenied.into()));
    });
    let panic = std::panic::catch_unwind(swept).expect_err("the sweep panics");
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some(&*format!("remove {}: permission denied", dead.display()))
    );
}

#[test]
#[cfg(unix)]
fn a_rig_keeps_a_file_with_the_name_of_a_directory() {
    let file = std::env::temp_dir().join(format!("{PREFIX}{}-file-0", gone()));
    std::fs::write(&file, "").expect("write the file");
    let _rig = Rig::new();
    assert!(file.exists(), "{} is gone", file.display());
    std::fs::remove_file(file).expect("remove the file");
}

#[test]
#[cfg(unix)]
fn a_start_on_a_data_directory_keeps_the_budget_that_it_holds() {
    let mut rig = Rig::new();
    rig.keep(POOL / 2, rig.disk);
    rig.start_with(&["--name", "edge", "--data", "foundation-data"]);
    rig.stop();
    #[expect(
        clippy::disallowed_methods,
        reason = "the test reads what the rig wrote to the disk"
    )]
    let kept = std::fs::read(rig.budget()).expect("read the budget file");
    assert_eq!(kept, budget(POOL / 2, rig.disk));
}

#[test]
fn a_start_with_a_joined_data_path_panics() {
    let mut rig = Rig::new();
    let start = std::panic::AssertUnwindSafe(|| rig.start_with(&["--data=x"]));
    let panic = std::panic::catch_unwind(start).expect_err("the start panics");
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some("give `--data <path>`, not Some(\"--data=x\")")
    );
}

#[test]
fn a_rig_writes_the_config_as_given() {
    let rig = Rig::new();
    rig.config("node \"edge\" {}\n");
    assert_eq!(
        std::fs::read_to_string(rig.dir.join("plant.hcl")).expect("read plant.hcl"),
        "node \"edge\" {}\n"
    );
}

#[test]
fn a_rig_removes_its_directory_with_its_files() {
    let rig = Rig::new();
    rig.config("");
    let dir = rig.dir.clone();
    drop(rig);
    assert!(!dir.exists(), "{} is still there", dir.display());
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "needs /proc")]
fn a_test_that_panics_kills_its_command() {
    let mut command = Command::new("sh");
    command.args(["-c", "exec sleep 60"]);
    let (running, _) = Running::new(command);
    let pid = running.process.0.id();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _running = running;
        panic!("the test fails");
    }))
    .expect_err("the test panics");
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"the test fails"));
    // Killed and not waited for, the command stays a zombie.
    wait(
        &os::clock(),
        Span::from_nanos(10_000_000_000),
        "the command ends",
        || {
            let stat =
                std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("stat");
            let state = stat.rsplit(") ").next().expect("a state").chars().next();
            if state == Some('Z') {
                Ok(())
            } else {
                Err(stat)
            }
        },
    );
}

/// Set to `drop` or `panic` in the test binary that
/// [`drop_a_command_that_the_kernel_reaped`] runs.
const REAPED: &str = "RIG_REAPED";

/// Drops a command that the kernel reaped, so its kill fails, and panics after the
/// drop when [`REAPED`] is `panic`. Does nothing when [`REAPED`] is not set.
#[test]
fn a_command_that_the_kernel_reaped_drops() {
    #[expect(
        clippy::disallowed_methods,
        reason = "the parent test sets it for the child"
    )]
    let Some(reaped) = std::env::var_os(REAPED) else {
        return;
    };
    let mut command = Command::new("sh");
    command.args(["-c", "exit 0"]);
    let (running, _) = Running::new(command);
    let pid = running.process.0.id();
    wait(
        &os::clock(),
        Span::from_nanos(10_000_000_000),
        "the kernel reaps the command",
        || {
            let exists = std::fs::exists(format!("/proc/{pid}")).expect("check /proc");
            if exists {
                Err(format!("{pid} runs"))
            } else {
                Ok(())
            }
        },
    );
    let _running = running;
    assert_ne!(reaped, "panic", "the test fails");
}

/// The test binary that runs [`a_command_that_the_kernel_reaped_drops`] with
/// [`REAPED`] set to `reaped`. It ignores `SIGCHLD`, so the kernel reaps each command
/// it starts.
fn reaped(reaped: &str) -> Command {
    let binary = std::env::current_exe().expect("the test binary");
    let mut command = Command::new("perl");
    command
        .args(["-e", "$SIG{CHLD} = 'IGNORE'; exec @ARGV or die"])
        .arg(binary)
        .args(["--exact", "rig::a_command_that_the_kernel_reaped_drops"])
        .env(REAPED, reaped)
        // With it, the harness of the child writes the panic to stderr, not stdout.
        .env_remove("RUST_TEST_NOCAPTURE");
    command
}

/// Runs [`reaped`] and gives the report of the test harness.
fn drop_a_command_that_the_kernel_reaped(reaped: &str) -> String {
    let output = self::reaped(reaped).output().expect("run the test binary");
    assert_eq!(
        output.status.code(),
        Some(101),
        "the test fails with no abort"
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "needs perl and /proc")]
fn a_failed_kill_panics() {
    let report = drop_a_command_that_the_kernel_reaped("drop");
    assert!(
        report.contains(concat!(
            "kill the command: Os { code: 3, kind: Uncategorized, ",
            "message: \"No such process\" }\n",
        )),
        "{report}"
    );
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "needs perl and /proc")]
fn a_failed_kill_while_the_test_panics_with_a_closed_stderr_does_not_abort() {
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    let output = reaped("panic")
        .arg("--nocapture")
        .stderr(writer)
        .output()
        .expect("run the test binary");
    let report = String::from_utf8(output.stdout).expect("UTF-8");
    assert_eq!(output.status.code(), Some(101), "{report}");
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "needs perl and /proc")]
fn a_failed_kill_while_the_test_panics_reports_its_error() {
    let report = drop_a_command_that_the_kernel_reaped("panic");
    assert!(report.contains("the test fails\n"), "{report}");
    assert!(
        report.contains("kill the command: No such process (os error 3)\n"),
        "{report}"
    );
}

/// A developer can set `RUST_TEST_NOCAPTURE` for the whole run.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "needs perl and /proc")]
fn a_failed_kill_panics_in_a_run_that_does_not_capture() {
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", "rig::a_failed_kill_panics"])
        .env("RUST_TEST_NOCAPTURE", "1")
        .output()
        .expect("run the test binary");
    let report = String::from_utf8(output.stdout).expect("UTF-8");
    assert!(report.contains("test result: ok. 1 passed"), "{report}");
}

#[test]
fn a_rig_keeps_its_directory_when_the_test_fails() {
    let mut dir = PathBuf::new();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let rig = Rig::new();
        rig.config("");
        dir.clone_from(&rig.dir);
        panic!("the test fails");
    }))
    .expect_err("the test panics");
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"the test fails"));
    assert!(dir.join("plant.hcl").exists(), "{} is gone", dir.display());
    std::fs::remove_dir_all(&dir).expect("remove the directory");
}

#[test]
#[cfg(unix)]
fn a_run_past_its_limit_kills_the_command_and_panics_with_its_output() {
    let mut command = Command::new("sh");
    command.args(["-c", "echo $$; echo err >&2; exec sleep 60"]);
    let clock = os::clock();
    let started = clock.now();
    let limited =
        std::panic::AssertUnwindSafe(|| run(&clock, Span::SECOND, command, &[]));
    let panic = std::panic::catch_unwind(limited).expect_err("the run panics");
    let took = clock.now() - started;
    let message = panic.downcast_ref::<String>().expect("a message");
    let pid = message.lines().nth(2).expect("the PID");
    assert_eq!(
        message,
        &format!(
            "the command exits and closes its pipes: not within 1s. The last check \
             saw:\nstdout:\n{pid}\n\nstderr:\nerr\n"
        )
    );
    assert!(
        took < Span::from_nanos(30_000_000_000),
        "the kill took {took}"
    );
    let alive = Command::new("kill")
        .args(["-0", pid])
        .output()
        .expect("kill -0");
    assert_eq!(alive.status.code(), Some(1), "{pid} still runs");
}

/// Runs `script`, which starts a `sleep 60` that keeps one pipe of the command and
/// prints its PID, and asserts that the run panics at its limit. Then kills the
/// `sleep`.
#[cfg(unix)]
fn assert_a_run_ends_at_its_limit(script: &str, input: &[u8]) {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    let limited = std::panic::AssertUnwindSafe(|| {
        run(&os::clock(), Span::SECOND, command, input)
    });
    let panic = std::panic::catch_unwind(limited).expect_err("the run panics");
    let message = panic.downcast_ref::<String>().expect("a message");
    let pid = message.lines().nth(2).expect("the PID");
    let killed = Command::new("kill").arg(pid).status().expect("kill");
    assert_eq!(
        message,
        &format!(
            "the command exits and closes its pipes: not within 1s. The last check \
             saw:\nstdout:\n{pid}\n\nstderr:\n"
        )
    );
    assert!(killed.success(), "{pid} was gone");
}

#[test]
#[cfg(unix)]
fn a_run_ends_at_its_limit_when_a_process_of_the_command_keeps_its_output() {
    assert_a_run_ends_at_its_limit("sleep 60 2>/dev/null & echo $!", &[]);
}

#[test]
#[cfg(unix)]
fn a_run_ends_at_its_limit_when_a_process_of_the_command_keeps_its_errors() {
    assert_a_run_ends_at_its_limit("sleep 60 >/dev/null & echo $!", &[]);
}

#[test]
#[cfg(unix)]
fn a_run_ends_at_its_limit_when_a_process_of_the_command_keeps_its_input() {
    // A background job of `sh` reads `/dev/null` unless it names its input.
    assert_a_run_ends_at_its_limit(
        "exec 3<&0; sleep 60 <&3 >/dev/null 2>&1 & echo $!",
        &vec![0; 1 << 20],
    );
}

#[test]
#[cfg(unix)]
fn a_run_ends_at_its_limit_when_the_command_reads_none_of_its_input() {
    let mut command = Command::new("sh");
    command.args(["-c", "exec sleep 60"]);
    let input = vec![0; 1 << 20];
    let limited = std::panic::AssertUnwindSafe(|| {
        run(&os::clock(), Span::SECOND, command, &input)
    });
    let panic = std::panic::catch_unwind(limited).expect_err("the run panics");
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some(
            "the command exits and closes its pipes: not within 1s. The last check \
             saw:\nstdout:\n\nstderr:\n"
        )
    );
}

#[test]
#[cfg(unix)]
fn a_run_gives_the_output_of_a_command_that_exits_before_it_reads_its_input() {
    let mut command = Command::new("sh");
    command.args(["-c", "echo done"]);
    let output = run(&os::clock(), PATIENCE, command, &vec![0; 1 << 20]);
    assert_eq!(
        (
            output.status.code(),
            crate::text(&output.stdout),
            crate::text(&output.stderr)
        ),
        (Some(0), "done\n", "")
    );
}

#[test]
fn a_wait_gives_the_first_ok() {
    let mut calls = 0;
    let value = wait(&os::clock(), PATIENCE, "the third call", || {
        calls += 1;
        if calls == 3 {
            Ok(calls)
        } else {
            Err(format!("call {calls}"))
        }
    });
    assert_eq!(value, 3);
}

#[test]
fn a_wait_past_its_limit_panics_with_the_last_state() {
    let panic = std::panic::catch_unwind(|| {
        wait(&os::clock(), Span::ZERO, "the node runs", || {
            Err::<(), _>("state: stopped".to_owned())
        });
    })
    .expect_err("the wait panics");
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some("the node runs: not within 0s. The last check saw:\nstate: stopped")
    );
}

#[test]
fn a_node_that_exits_before_it_runs_panics_with_its_output() {
    let mut rig = Rig::new();
    let start = std::panic::AssertUnwindSafe(|| rig.start_with(&[]));
    let panic = std::panic::catch_unwind(start).expect_err("the start panics");
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some(
            "the node exited with exit status: 1:\nstdout:\n\nstderr:\n\
             error[node.unnamed]: the data directory foundation-data holds no node\n\
             fix: Give the new node a name with `--name`\n"
        )
    );
}

#[test]
#[cfg(unix)]
fn a_rig_ends_its_node_before_it_removes_the_directory() {
    let mut rig = Rig::new();
    rig.start();
    let pid = rig
        .node
        .as_ref()
        .expect("a node")
        .process
        .0
        .id()
        .to_string();
    let dir = rig.dir.clone();
    drop(rig);
    assert!(!dir.exists(), "{} is still there", dir.display());
    let alive = Command::new("kill")
        .args(["-0", &pid])
        .output()
        .expect("kill -0");
    assert_eq!(alive.status.code(), Some(1), "{pid} still runs");
}

/// The budget that `a_disk_budget_of_8_mib_for_each_core_starts_a_host_of_any_size`
/// in `node` starts. A fixed budget of 256 MiB holds no ring on each of 64 shards.
#[test]
fn a_rig_keeps_8_mib_of_disk_for_each_core() {
    for cores in [1, 64, 1024] {
        let rig = Rig::with_cores(cores);
        #[expect(
            clippy::disallowed_methods,
            reason = "the test reads what the rig wrote to the disk"
        )]
        let kept = std::fs::read(rig.budget()).expect("read the budget file");
        let disk = u64::from_le_bytes(kept[27..35].try_into().expect("8 bytes"));
        assert_eq!(
            disk,
            u64::try_from(cores).unwrap() * (8 << 20),
            "{cores} cores"
        );
    }
}

/// A sweep sees a directory that a test leaves only once it is whole.
#[test]
#[cfg(unix)]
fn a_rig_does_not_sweep_a_directory_that_a_test_still_leaves() {
    create_left(gone(), "draft", |data| {
        drop(Rig::new());
        assert!(data.exists(), "{} is gone", data.display());
    });
}

/// Set in the test binary that [`a_sweep_removes_the_draft_of_a_run_that_died`] runs.
const DIES: &str = "RIG_DIES";

/// The status with which [`a_run_dies_while_it_leaves_a_directory`] exits. The test
/// binary gives 0 or 101 in each other case.
const DIED: i32 = 3;

/// Exits with [`DIED`] after it made the tree of [`create_tree`] and while
/// [`create_left`] fills its draft, as a killed run does: `exit` runs no destructor.
/// Does nothing when [`DIES`] is not set. It does not abort, because on macOS each
/// abort writes a crash report that no sweep removes.
#[test]
#[cfg(unix)]
fn a_run_dies_while_it_leaves_a_directory() {
    #[expect(
        clippy::disallowed_methods,
        reason = "the parent test sets it for the child"
    )]
    if std::env::var_os(DIES).is_some() {
        create_tree(0);
        #[expect(
            clippy::disallowed_methods,
            clippy::exit,
            reason = "the child dies with no destructor, as a killed run does"
        )]
        create_left(gone(), "died", |_| std::process::exit(DIED));
    }
}

/// The drafts of a run that died while it left a directory go at the next sweep.
#[test]
#[cfg(unix)]
fn a_sweep_removes_the_draft_of_a_run_that_died() {
    let mut child = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", "rig::a_run_dies_while_it_leaves_a_directory"])
        .env(DIES, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run the test binary");
    let status = child.wait().expect("wait for the test binary");
    assert_eq!(
        status.code(),
        Some(DIED),
        "the run does not exit in `create_left`"
    );
    drop(Rig::new());
    // Any draft of the child, whatever its name.
    let pid = format!("{}-", child.id());
    let left: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .expect("read the temporary directory")
        .map(|entry| entry.expect("read the temporary directory").file_name())
        .filter(|name| {
            let name = name.to_string_lossy();
            name.contains(&pid) && (name.ends_with("-died") || name.contains("walked-"))
        })
        .collect();
    assert_eq!(left, Vec::<std::ffi::OsString>::new());
}

/// A sweep that makes a directory writable while another sweep removes it gives
/// `NotFound` only once the directory is gone, so it never leaves the directory.
#[test]
#[cfg(unix)]
fn a_walk_that_another_sweep_removes_ends_only_when_it_is_gone() {
    for run in 0..20 {
        let dir = create_tree(run);
        let path = dir.clone();
        #[expect(
            clippy::disallowed_methods,
            reason = "another sweep removes the directory during the walk"
        )]
        let removal = std::thread::spawn(move || std::fs::remove_dir_all(path));
        let ended = loop {
            let finished = removal.is_finished();
            if let Err(error) = writable(&dir) {
                break (error.kind(), dir.exists());
            }
            assert!(
                !finished,
                "a walk after the removal of {} gave no error",
                dir.display()
            );
        };
        removal.join().expect("join the removal").expect("remove");
        assert_eq!(ended, (ErrorKind::NotFound, false), "run {run}");
    }
}

/// Makes the tree that [`a_walk_that_another_sweep_removes_ends_only_when_it_is_gone`]
/// walks in its `run`: 300 directories of 10 files each.
#[cfg(unix)]
fn create_tree(run: u32) -> PathBuf {
    let dir = draft(&format!("walked-{run}"));
    for sub in 0..300 {
        let sub = dir.join(format!("d{sub}"));
        std::fs::create_dir_all(&sub).expect("make a directory");
        for file in 0..10 {
            std::fs::write(sub.join(format!("f{file}")), "").expect("make a file");
        }
    }
    dir
}
