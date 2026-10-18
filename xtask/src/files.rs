//! Paths and the files under a directory.

use std::path::{Component, Path, PathBuf};

/// Every file under `dir` with the extension `extension`, sorted.
///
/// # Errors
///
/// A directory that cannot be read.
pub(crate) fn under(dir: &Path, extension: &str) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Removes `.` and `..` components without reading the disk. Cargo reports a
/// `[[test]] path` such as `../../oracles/x.rs` joined to the manifest directory.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_lists_the_files_with_the_extension_in_subdirectories() {
        let oracles = crate::fixture().join("oracles");
        let found = under(&oracles, "rs").unwrap();
        let names: Vec<_> = found.iter().map(|f| f.strip_prefix(&oracles)).collect();
        assert_eq!(
            names,
            [
                Ok(Path::new("a/common.rs")),
                Ok(Path::new("a/main.rs")),
                Ok(Path::new("ignored.rs")),
                Ok(Path::new("orphan.rs")),
            ]
        );
    }

    #[test]
    fn normalize_removes_current_and_parent_components() {
        let path = Path::new("/w/crates/raft/./../../oracles/x.rs");
        assert_eq!(normalize(path), PathBuf::from("/w/oracles/x.rs"));
    }
}
