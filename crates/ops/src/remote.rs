//! Runs `plan`, `apply`, and `status` of a command line on its node.

use std::path::{Path, PathBuf};

use env::paths::{self, Found, Paths};
use wire::hub::client::BODY_BYTES_MAX;

use crate::Failure;
use crate::apply::Applied;
use crate::error::{self, Error, Unread, Unwritten};
use crate::front_end::{self, File};
use crate::plan::Output;
use crate::{serve, status};

#[cfg(test)]
mod tests;

/// The files that a run reaches: each one for a command line, and for `foundation mcp`
/// only those under its folders, with its data directory hidden.
pub(crate) struct Scope {
    paths: Paths,
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
    pub(crate) fn free(paths: &Paths) -> Self {
        Self {
            paths: paths.clone(),
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
        files: &[PathBuf],
        data: &Path,
    ) -> Result<Self, Error> {
        let unmade = |error| Error::Data {
            path: data.to_owned(),
            error,
        };
        let folder = |path: &Path, error| Error::Folder {
            path: path.to_owned(),
            error,
        };
        let resolve =
            |path: &Path| paths.resolve(path).map_err(|e| folder(path, Some(e)));
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
                // A directory with entries is `Large` too, and only it reads with an
                // end in `/`.
                match paths.read(&resolved, 0) {
                    Ok(paths::Found::Directory(_)) => folders.push(resolved),
                    Ok(paths::Found::Large)
                        if paths.read(&resolved.join(""), 0).is_ok() =>
                    {
                        folders.push(resolved);
                    }
                    Ok(paths::Found::File(_) | paths::Found::Large) => {
                        return Err(folder(file, None));
                    }
                    Err(error) => return Err(folder(file, Some(error))),
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
            limits: Some(Limits {
                folders,
                data,
                working,
            }),
        })
    }

    /// The error of a call at `path` that its limits refused with `error`, `Outside`
    /// or `Hidden`.
    fn refused(&self, path: &Path, error: paths::Error) -> Error {
        let path = path.to_owned();
        match (error, &self.limits) {
            (paths::Error::Hidden, Some(limits)) => Error::Hidden {
                path,
                data: limits.data.clone(),
            },
            _ => Error::Outside {
                path,
                limits: self.limits.clone(),
            },
        }
    }

    /// The error of a resolve or a write of the plan file `out` that failed.
    fn unwritten(&self, out: &Path, error: paths::Error) -> Error {
        let cause = match error {
            paths::Error::Outside | paths::Error::Hidden => {
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
}

/// The link to one node: what reaches it.
pub(crate) struct Link<R> {
    reach: R,
}

impl<R> Link<R> {
    /// A link that calls `reach` at each send.
    pub(crate) fn new(reach: R) -> Self {
        Self { reach }
    }

    /// The reply of the node to `body`, through what a new call of `reach` gives,
    /// which it then drops. A session kept between sends would end while its thread
    /// blocks on the next line of MCP input.
    async fn send<C>(&mut self, body: &[u8]) -> Result<Vec<u8>, Failure>
    where
        R: AsyncFnMut() -> Result<C, Failure>,
        C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
    {
        let send = (self.reach)().await?;
        send(body).await
    }
}

/// Plans on the node of `link` the change from the config files at `files`, read
/// through `scope`, and writes the plan file to `out` when it is given.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` for each path that is not UTF-8.
/// - [`Error::Read`] for the first file that does not read as UTF-8 text.
/// - [`Error::LargeRequest`] when the files do not fit one request.
/// - [`Error::Node`] with the failure of the reach or of the request.
/// - Each error of [`serve::plan_reply`].
/// - [`Error::Write`] when the write of `out` fails.
/// - [`Error::Outside`] for the first path past the limits of `scope`.
/// - [`Error::PlanName`] when `scope` has limits and `out` does not resolve to a name
///   that ends in `.plan`, before it reads a file.
pub(crate) async fn plan<R, C>(
    files: &[PathBuf],
    out: Option<&Path>,
    scope: &Scope,
    link: &mut Link<R>,
) -> Result<Output, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let not_utf8: Vec<_> = files
        .iter()
        .filter(|path| path.to_str().is_none())
        .map(|path| front_end::not_utf8(path))
        .collect();
    if !not_utf8.is_empty() {
        return Err(Error::config(not_utf8, &error::Sources::default()));
    }
    if let (Some(out), Some(_)) = (out, &scope.limits) {
        let resolved = scope
            .paths
            .resolve(out)
            .map_err(|error| scope.unwritten(out, error))?;
        if resolved.extension() != Some("plan".as_ref()) {
            return Err(Error::PlanName {
                path: out.to_owned(),
                resolved,
            });
        }
    }
    let mut total = 0;
    let mut read = Vec::with_capacity(files.len());
    for path in files {
        let bytes = file(scope, path)?;
        total += bytes.len();
        if total > most() {
            return Err(Error::LargeRequest);
        }
        let text = String::from_utf8(bytes).map_err(|_not_utf8| Error::Read {
            path: path.clone(),
            cause: Unread::NotUtf8,
        })?;
        read.push(File {
            path: path.clone(),
            text,
        });
    }
    let reply = request(&serve::plan_request(&read)?, link).await?;
    let (output, plan) = serve::plan_reply(&reply)?;
    if let Some(out) = out {
        scope
            .paths
            .write(out, plan)
            .map_err(|error| scope.unwritten(out, error))?;
    }
    Ok(output)
}

/// Applies on the node of `link` the plan file at `plan`, read through `scope`.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` when `plan` is not UTF-8.
/// - [`Error::Read`] when the plan file does not read.
/// - [`Error::Outside`] when `plan` is past the limits of `scope`.
/// - [`Error::LargeRequest`] when the plan file does not fit one request.
/// - [`Error::Node`] with the failure of the reach or of the request.
/// - Each error of [`serve::reply`].
pub(crate) async fn apply<R, C>(
    plan: &Path,
    scope: &Scope,
    link: &mut Link<R>,
) -> Result<Applied, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let Some(name) = plan.to_str() else {
        let not_utf8 = vec![front_end::not_utf8(plan)];
        return Err(Error::config(not_utf8, &error::Sources::default()));
    };
    let bytes = file(scope, plan)?;
    let reply = request(&serve::apply_request(&bytes)?, link).await?;
    let effect = serve::reply(&reply, "apply")?;
    Ok(Applied {
        file: name.to_owned(),
        effect,
    })
}

/// The status of each connector on the node of `link`.
///
/// # Errors
///
/// - [`Error::Node`] with the failure of the reach or of the request.
/// - Each error of [`serve::reply`].
pub(crate) async fn status<R, C>(link: &mut Link<R>) -> Result<status::Output, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let reply = request(serve::STATUS_REQUEST, link).await?;
    serve::reply(&reply, "status")
}

/// The most bytes that one request holds.
fn most() -> usize {
    usize::try_from(BODY_BYTES_MAX)
        .expect("invariant: the bound of a body fits a usize")
}

/// The bytes of the file at `path`, of at most the bytes of one request.
fn file(scope: &Scope, path: &Path) -> Result<Vec<u8>, Error> {
    let unread = |cause| Error::Read {
        path: path.to_owned(),
        cause,
    };
    match scope.paths.read(path, most()) {
        Ok(Found::File(bytes)) => Ok(bytes),
        Ok(Found::Large) => Err(Error::LargeRequest),
        Ok(Found::Directory(_)) => Err(unread(Unread::Directory)),
        Err(paths::Error::NotFound) => Err(unread(Unread::NotFound)),
        Err(paths::Error::Io { code }) => Err(unread(Unread::Io { code })),
        Err(error @ (paths::Error::Outside | paths::Error::Hidden)) => {
            Err(scope.refused(path, error))
        }
    }
}

/// The reply of the node of `link` to `body`.
async fn request<R, C>(body: &[u8], link: &mut Link<R>) -> Result<Vec<u8>, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    link.send(body).await.map_err(Error::Node)
}
