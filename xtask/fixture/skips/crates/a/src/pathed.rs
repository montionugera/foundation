// A `#[path]` on an inline module names the directory of its modules.
#[path = "inline_dir"]
mod inline {
    mod inner;
}

// In a file that is not `mod.rs`, a `#[path]` outside an inline module is relative to
// the directory of the file.
#[path = "pathed_file.rs"]
mod file;
