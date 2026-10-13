//! Runs a test of this binary again in a child process, with the C flags it picks.

use std::process::{Command, Output};

/// The variable that marks a child process.
const MARK: &str = "CONNECTOR_OPCUA_CHILD";

/// Whether this process is a child that `run` or `output` started.
pub(crate) fn running() -> bool {
    #[expect(
        clippy::disallowed_methods,
        reason = "the parent test sets the environment of its child process"
    )]
    let mark = std::env::var_os(MARK);
    mark.is_some()
}

/// Gives the tool that `build` picks for `target` outside a build script.
pub(crate) fn tool(build: &mut cc::Build, target: &str) -> cc::Tool {
    build
        .target(target)
        .host(target)
        .opt_level(0)
        .cargo_metadata(false)
        .cargo_warnings(false)
        .get_compiler()
}

/// Runs the test `name` as `output` does, and asserts that it passes.
pub(crate) fn run(name: &str, envs: &[(&str, &str)]) {
    let output = output(name, envs);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "{name} with {envs:?}: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs the test `name` in a child process, and gives its output. The child gets only
/// `PATH`, `envs`, and, as `CC` unless `envs` sets it, the compiler that `cc` picks
/// here without its flags, so `cc` reads C flags there only from `envs`.
pub(crate) fn output(name: &str, envs: &[(&str, &str)]) -> Output {
    let compiler = tool(&mut cc::Build::new(), env!("CONNECTOR_OPCUA_TARGET"));
    #[expect(
        clippy::disallowed_methods,
        reason = "`cc` finds the compiler on the PATH of the child process"
    )]
    let path = std::env::var_os("PATH").expect("PATH is set");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", name])
        .env_clear()
        .env("PATH", path)
        .env("CC", compiler.path())
        .env(MARK, "1")
        .envs(envs.iter().copied());
    child.output().unwrap()
}

/// Asserts that the process of `output` wrote `line` to stderr and aborted. On Windows
/// the C runtime can write its own message after `line`.
pub(crate) fn assert_aborted(output: &Output, line: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        const SIGABRT: i32 = 6;
        assert_eq!(output.status.signal(), Some(SIGABRT), "{stderr}");
        assert_eq!(stderr, line);
    }
    #[cfg(windows)]
    {
        // The exit code of `abort` in the C runtime of MinGW.
        assert_eq!(output.status.code(), Some(3), "{stderr}");
        assert!(stderr.starts_with(line), "{stderr:?}");
    }
}
