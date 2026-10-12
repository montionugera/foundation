//! The drop of `free` on a RAM disk of its own, on macOS. Each test starts `hdiutil`
//! and `diskutil`, and a child process keeps the room of each file that this process
//! removes until its `exec`. So these tests run in a test binary of their own. The root
//! tests check this on Linux, on a filesystem of their own.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};

#[path = "common/mod.rs"]
#[expect(dead_code, reason = "this binary uses some of the helpers")]
mod disk;
#[path = "common/free.rs"]
mod free;

use disk::files;

/// A RAM disk of 64 MiB with an HFS+ volume, which no other process writes. It
/// detaches when it drops, except while the thread panics.
struct Ram {
    device: String,
    mount: PathBuf,
}

impl Ram {
    fn new() -> Self {
        // The size is in sectors of 512 bytes.
        let attach = ["attach", "-nomount", "ram://131072"];
        let device = stdout(std::process::Command::new("hdiutil").args(attach));
        let device = device.trim().to_owned();
        // The device is unique while it is attached, so tests that run at the same
        // time get their own volumes.
        let disk = device.trim_start_matches("/dev/");
        let name = format!("foundation-os-{}-{disk}", std::process::id());
        let erase = ["erasevolume", "HFS+", &name, &device];
        stdout(std::process::Command::new("diskutil").args(erase));
        let mount = Path::new("/Volumes").join(name);
        Self { device, mount }
    }
}

impl Drop for Ram {
    fn drop(&mut self) {
        // While the thread panics, a second panic aborts the test binary.
        if !std::thread::panicking() {
            let detach = ["detach", self.device.as_str()];
            stdout(std::process::Command::new("hdiutil").args(detach));
        }
    }
}

/// Runs `command`, and gives its output. Panics with its error output when it fails.
fn stdout(command: &mut std::process::Command) -> String {
    let output = command.output().unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{command:?}: {error}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn free_drops_by_the_bytes_of_a_created_file() {
    check_free_on_ram(free::Create::Held);
}

#[test]
fn a_free_waits_for_a_dropped_create() {
    check_free_on_ram(free::Create::Dropped);
}

/// Runs `free::check` on a `Ram` of its own.
fn check_free_on_ram(create: free::Create) {
    let ram = Ram::new();
    let (files, thread) = files(&ram.mount, "files");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async move { free::check(&files, create).await });
    thread.join().unwrap();
}
