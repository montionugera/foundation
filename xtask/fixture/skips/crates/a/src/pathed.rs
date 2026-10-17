mod inline {}

// In a file that is not `mod.rs`, a `#[path]` outside an inline module is relative to
// the directory of the file.
#[path = "pathed_file.rs"]
mod file;
