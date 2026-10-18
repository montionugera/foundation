//! The files that `plan` and `apply` reach, and the limits of `foundation mcp`.

use std::path::{Path, PathBuf};

use env::paths::{self, Found, Kind, Paths};
use wire::hub::client::BODY_BYTES_MAX;

use crate::error::{self, Error, Unread, Unusable, Unwritten};
use crate::front_end::{self, File, FrontEnds};

/// The files that a run reaches: each one for a command line, and for `foundation mcp`
/// only those under its folders, with its data directory hidden. The front ends pick
/// the config files of a directory.
pub(crate) struct Scope {
    paths: Paths,
    front_ends: FrontEnds,
    limits: Option<Limits>,
}

/// The resolved folders of `foundation mcp`, its resolved data directory, and the
/// resolved working directory, where a relative path starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) folders: Vec<PathBuf>,
    pub(crate) data: PathBuf,
    pub(crate) working: PathBuf,
}

impl Scope {
    /// Each file of `paths`, for a command line.
    pub(crate) fn free(paths: &Paths, front_ends: &FrontEnds) -> Self {
        Self {
            paths: paths.clone(),
            front_ends: front_ends.clone(),
            limits: None,
        }
    }

    /// The files of `foundation mcp` in `paths`: those under the folders `files`, else
    /// under the working directory, and none under the data directory `data`, which it
    /// makes after it checks the folders, when it is not there.
    ///
    /// # Errors
    ///
    /// - [`Error::WideFolder`] with no `files`, when the working directory is `/` or
    ///   the home folder.
    /// - [`Error::Folder`] for the first folder of `files` that does not resolve, does
    ///   not read, or is a file.
    /// - [`Error::Data`] when `data` is not made, or does not resolve.
    pub(crate) fn mcp(
        paths: &Paths,
        front_ends: &FrontEnds,
        files: &[PathBuf],
        data: &Path,
    ) -> Result<Self, Error> {
        let unmade = |error| Error::Data {
            path: data.to_owned(),
            error,
        };
        let folder = |path: &Path, cause| Error::Folder {
            path: path.to_owned(),
            cause,
        };
        let resolve = |path: &Path| {
            let failed = |error| folder(path, Unusable::Failed(error));
            paths.resolve(path).map_err(failed)
        };
        let working = resolve(Path::new("."))?;
        let folders = if files.is_empty() {
            if working.parent().is_none() || paths.home().as_ref() == Some(&working) {
                return Err(Error::WideFolder { working });
            }
            vec![working.clone()]
        } else {
            let mut folders = Vec::with_capacity(files.len());
            for file in files {
                let resolved = resolve(file)?;
                match paths.read(&resolved, 0) {
                    Ok(Found::Directory(_) | Found::Large(Kind::Directory)) => {
                        folders.push(resolved);
                    }
                    Ok(Found::File(_) | Found::Large(Kind::File)) => {
                        return Err(folder(file, Unusable::File));
                    }
                    Err(error) => return Err(folder(file, Unusable::Failed(error))),
                }
            }
            folders
        };
        paths.make_dir(data).map_err(unmade)?;
        let data = paths.resolve(data).map_err(unmade)?;
        let within = paths
            .within(&folders, std::slice::from_ref(&data))
            .map_err(unmade)?;
        Ok(Self {
            paths: within,
            front_ends: front_ends.clone(),
            limits: Some(Limits {
                folders,
                data,
                working,
            }),
        })
    }

    /// The error of a call at `path` that its limits refused with `error`, `Outside`,
    /// `Hidden`, `Vacant`, or `Replaced`.
    fn refused(&self, path: &Path, error: paths::Error) -> Error {
        let path = path.to_owned();
        let limits = self
            .limits
            .clone()
            .expect("invariant: only the paths of a scope with limits refuse a path");
        match error {
            paths::Error::Hidden => Error::Hidden {
                path,
                data: limits.data,
            },
            paths::Error::Vacant => Error::NoData {
                path,
                data: limits.data,
            },
            paths::Error::Replaced => Error::FolderChanged {
                path,
                folders: limits.folders,
            },
            _ => Error::Outside { path, limits },
        }
    }

    /// The error of a resolve or a write of the file `out` that failed.
    fn unwritten(&self, out: &Path, error: paths::Error) -> Error {
        let cause = match error {
            paths::Error::Outside
            | paths::Error::Hidden
            | paths::Error::Vacant
            | paths::Error::Replaced => {
                return self.refused(out, error);
            }
            paths::Error::NotFound => Unwritten::NotFound,
            paths::Error::Io { code } => Unwritten::Io { code },
        };
        Error::Write {
            path: out.to_owned(),
            cause,
        }
    }

    /// Checks that the plan file `out` resolves to a name that ends in `.plan`, for a
    /// scope with limits.
    ///
    /// # Errors
    ///
    /// - [`Error::PlanName`] when it does not.
    /// - [`Error::Outside`], [`Error::Hidden`], [`Error::NoData`],
    ///   [`Error::FolderChanged`], or [`Error::Write`] when `out` does not resolve.
    pub(crate) fn out(&self, out: &Path) -> Result<(), Error> {
        if self.limits.is_none() {
            return Ok(());
        }
        let resolved = self
            .paths
            .resolve(out)
            .map_err(|error| self.unwritten(out, error))?;
        if resolved.extension() != Some("plan".as_ref()) {
            return Err(Error::PlanName {
                path: out.to_owned(),
                resolved,
            });
        }
        Ok(())
    }

    /// The path and text of each config file of `files`, in order: a file, or each
    /// file of a directory whose name has the extension of a front end and does not
    /// start with `.`, in name order. The files of a directory in it are not read.
    ///
    /// # Errors
    ///
    /// - [`Error::Config`] with `ops.path-not-utf8` for each such file of the first
    ///   directory that has one whose path is not UTF-8.
    /// - [`Error::Read`] for the first file that does not read as UTF-8 text, or a
    ///   directory whose entries pass the bytes of one read.
    /// - [`Error::NoConfig`] for the first directory that gives no file.
    /// - [`Error::LargeRequest`] once the files that it read pass the most of a
    ///   request.
    /// - The other errors of [`Scope::read`].
    pub(crate) fn config(&self, files: &[PathBuf]) -> Result<Vec<File>, Error> {
        let mut read = Vec::with_capacity(files.len());
        let mut total = 0;
        let mut add = |path: &Path, bytes: Vec<u8>| {
            total += bytes.len();
            if total > most() {
                return Err(Error::LargeRequest);
            }
            let text = String::from_utf8(bytes).map_err(|_not_utf8| Error::Read {
                path: path.to_owned(),
                cause: Unread::NotUtf8,
            })?;
            read.push(File {
                path: path.to_owned(),
                text,
            });
            Ok(())
        };
        for path in files {
            match self.found(path)? {
                Found::File(bytes) => add(path, bytes)?,
                Found::Large(Kind::File) => return Err(Error::LargeRequest),
                Found::Large(Kind::Directory) => {
                    return Err(Error::Read {
                        path: path.clone(),
                        cause: Unread::ManyEntries,
                    });
                }
                Found::Directory(entries) => {
                    let listed: Vec<_> = entries
                        .into_iter()
                        .filter(|entry| {
                            entry
                                .file_name()
                                .is_some_and(|name| self.front_ends.reads(name))
                        })
                        .collect();
                    if listed.is_empty() {
                        return Err(Error::NoConfig {
                            path: path.clone(),
                            extensions: self.front_ends.extensions().collect(),
                        });
                    }
                    utf8(&listed)?;
                    for entry in listed {
                        add(&entry, self.read(&entry)?)?;
                    }
                }
            }
        }
        Ok(read)
    }

    /// The bytes of the file at `path`, of at most the bytes of one request.
    ///
    /// # Errors
    ///
    /// - [`Error::LargeRequest`] when it has more.
    /// - [`Error::Read`] when it is a directory, is not there, or does not read.
    /// - [`Error::Outside`], [`Error::Hidden`], or [`Error::FolderChanged`] when the
    ///   limits refuse it.
    pub(crate) fn read(&self, path: &Path) -> Result<Vec<u8>, Error> {
        match self.found(path)? {
            Found::File(bytes) => Ok(bytes),
            Found::Large(Kind::File) => Err(Error::LargeRequest),
            Found::Directory(_) | Found::Large(Kind::Directory) => Err(Error::Read {
                path: path.to_owned(),
                cause: Unread::Directory,
            }),
        }
    }

    /// What is at `path`: the bytes of a file, or the path of each entry of a
    /// directory in name order, each list of at most the bytes of one request.
    ///
    /// # Errors
    ///
    /// - [`Error::Read`] when it is not there or does not read.
    /// - [`Error::Outside`], [`Error::Hidden`], or [`Error::FolderChanged`] when the
    ///   limits refuse it.
    fn found(&self, path: &Path) -> Result<Found, Error> {
        let unread = |cause| Error::Read {
            path: path.to_owned(),
            cause,
        };
        match self.paths.read(path, most()) {
            Ok(found) => Ok(found),
            Err(paths::Error::NotFound) => Err(unread(Unread::NotFound)),
            Err(paths::Error::Io { code }) => Err(unread(Unread::Io { code })),
            Err(
                error @ (paths::Error::Outside
                | paths::Error::Hidden
                | paths::Error::Replaced),
            ) => Err(self.refused(path, error)),
            Err(paths::Error::Vacant) => {
                unreachable!("invariant: a read is never vacant")
            }
        }
    }

    /// Writes `bytes` to the file `out`, a plan file or a config file.
    ///
    /// # Errors
    ///
    /// [`Error::Write`], [`Error::Outside`], [`Error::Hidden`], [`Error::NoData`], or
    /// [`Error::FolderChanged`] when the write fails.
    pub(crate) fn write(&self, out: &Path, bytes: &[u8]) -> Result<(), Error> {
        self.paths
            .write(out, bytes)
            .map_err(|error| self.unwritten(out, error))
    }
}

/// The most bytes that one request holds.
pub(crate) fn most() -> usize {
    usize::try_from(BODY_BYTES_MAX)
        .expect("invariant: the bound of a body fits a usize")
}

/// `Config` with `ops.path-not-utf8` for each of `files` whose path is not UTF-8.
pub(crate) fn utf8(files: &[PathBuf]) -> Result<(), Error> {
    let not_utf8: Vec<_> = files
        .iter()
        .filter(|path| path.to_str().is_none())
        .map(|path| front_end::not_utf8(path))
        .collect();
    if not_utf8.is_empty() {
        Ok(())
    } else {
        Err(Error::config(not_utf8, &error::Sources::default()))
    }
}
