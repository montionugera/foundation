//! The cost of one poll of pending sleeps whose deadlines do not change, which a task
//! pays at each wake for another cause.

#[cfg(any(target_os = "linux", target_os = "macos"))] // #682
mod unix;

fn main() {
    divan::main();
}
