//! The CA certificates of the OS store.

use std::ffi::OsStr;
use std::fmt;
use std::path::PathBuf;

/// The CA certificates of the OS store, and each failure of the read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roots {
    /// The DER bytes of each certificate that the read found.
    pub certificates: Vec<Vec<u8>>,
    /// The text of each failure. A failure skips its part of the store, and the read
    /// goes on.
    pub failures: Vec<String>,
    /// Where the read looked.
    pub source: Source,
}

/// Where a read of the roots looks, which `SSL_CERT_FILE` and `SSL_CERT_DIR` pick.
/// Its text is `the OS store`, or `SSL_CERT_FILE=<path>` when the file is set and
/// `SSL_CERT_DIR=<paths>`, the paths joined by `:`, when a directory is, with a space
/// between the two.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// The store of the OS: the certificate files on Linux, the keychain on macOS.
    #[default]
    Store,
    /// The file and the directories that the environment names, in place of the store.
    /// It holds a file or at least one directory.
    Paths {
        /// The path of `SSL_CERT_FILE`, when it is set.
        file: Option<PathBuf>,
        /// Each path of `SSL_CERT_DIR` that is not empty, in order.
        dirs: Vec<PathBuf>,
    },
}

impl Source {
    /// The source that the values of `SSL_CERT_FILE` and `SSL_CERT_DIR` pick: their
    /// paths when `file` is set or `dirs` holds a path that is not empty, else the
    /// store.
    pub(crate) fn of(file: Option<&OsStr>, dirs: Option<&OsStr>) -> Self {
        let dirs: Vec<PathBuf> = dirs
            .map(|dirs| {
                std::env::split_paths(dirs)
                    .filter(|path| !path.as_os_str().is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if file.is_none() && dirs.is_empty() {
            return Self::Store;
        }
        Self::Paths {
            file: file.map(PathBuf::from),
            dirs,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (file, dirs) = match self {
            Self::Store => return f.write_str("the OS store"),
            Self::Paths { file, dirs } => (file, dirs),
        };
        if let Some(file) = file {
            write!(f, "SSL_CERT_FILE={}", file.display())?;
            if !dirs.is_empty() {
                f.write_str(" ")?;
            }
        }
        if let Some((first, rest)) = dirs.split_first() {
            write!(f, "SSL_CERT_DIR={}", first.display())?;
            for dir in rest {
                write!(f, ":{}", dir.display())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::Source;

    fn of(file: Option<&str>, dirs: Option<&str>) -> Source {
        Source::of(file.map(OsStr::new), dirs.map(OsStr::new))
    }

    #[test]
    fn no_path_of_the_environment_picks_the_store() {
        for dirs in [None, Some(""), Some(":"), Some("::")] {
            assert_eq!(of(None, dirs), Source::Store, "SSL_CERT_DIR={dirs:?}");
        }
        assert_eq!(of(None, None).to_string(), "the OS store");
    }

    #[test]
    fn a_set_file_picks_its_path_also_when_it_is_empty() {
        let source = of(Some(""), Some(":"));
        assert_eq!(
            source,
            Source::Paths {
                file: Some(PathBuf::new()),
                dirs: Vec::new(),
            }
        );
        assert_eq!(source.to_string(), "SSL_CERT_FILE=");
    }

    #[test]
    fn the_paths_of_ssl_cert_dir_keep_their_order_with_no_empty_one() {
        let source = of(None, Some(":/b::/a:"));
        assert_eq!(
            source,
            Source::Paths {
                file: None,
                dirs: vec![PathBuf::from("/b"), PathBuf::from("/a")],
            }
        );
        assert_eq!(source.to_string(), "SSL_CERT_DIR=/b:/a");
    }

    #[test]
    fn a_file_and_directories_give_both_with_a_space() {
        let source = of(Some("/roots.pem"), Some("/a:/b"));
        assert_eq!(
            source.to_string(),
            "SSL_CERT_FILE=/roots.pem SSL_CERT_DIR=/a:/b"
        );
    }
}
