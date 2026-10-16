//! Runs `plan`, `apply`, and `status` of a command line on its node.

use std::path::{Path, PathBuf};

use env::paths::{self, Found, Paths};
use wire::hub::client::BODY_BYTES_MAX;

use crate::Failure;
use crate::apply::Applied;
use crate::error::{self, Error, Unread};
use crate::front_end::{self, File};
use crate::plan::Output;
use crate::{serve, status};

#[cfg(test)]
mod tests;

/// Plans on the node that `reach` reaches the change from the config files at `files`,
/// read through `paths`, and writes the plan file to `out` when it is given. `reach`
/// gives what sends the request.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` for each path that is not UTF-8.
/// - [`Error::Read`] for the first file that does not read as UTF-8 text.
/// - [`Error::LargeRequest`] when the files do not fit one request.
/// - [`Error::Node`] with the failure of `reach` or of the request.
/// - Each error of [`serve::plan_reply`].
/// - [`Error::Write`] when the write of `out` fails.
pub(crate) async fn plan<C>(
    files: &[PathBuf],
    out: Option<&Path>,
    paths: &Paths,
    reach: impl AsyncFnOnce() -> Result<C, Failure>,
) -> Result<Output, Error>
where
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
    let mut total = 0;
    let mut read = Vec::with_capacity(files.len());
    for path in files {
        let bytes = file(paths, path)?;
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
    let reply = request(&serve::plan_request(&read)?, reach).await?;
    let (output, plan) = serve::plan_reply(&reply)?;
    if let Some(out) = out {
        paths.write(out, plan).map_err(|error| Error::Write {
            path: out.to_owned(),
            error,
        })?;
    }
    Ok(output)
}

/// Applies on the node that `reach` reaches the plan file at `plan`, read through
/// `paths`. `reach` gives what sends the request.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` when `plan` is not UTF-8.
/// - [`Error::Read`] when the plan file does not read.
/// - [`Error::LargeRequest`] when the plan file does not fit one request.
/// - [`Error::Node`] with the failure of `reach` or of the request.
/// - Each error of [`serve::reply`].
pub(crate) async fn apply<C>(
    plan: &Path,
    paths: &Paths,
    reach: impl AsyncFnOnce() -> Result<C, Failure>,
) -> Result<Applied, Error>
where
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let Some(name) = plan.to_str() else {
        let not_utf8 = vec![front_end::not_utf8(plan)];
        return Err(Error::config(not_utf8, &error::Sources::default()));
    };
    let bytes = file(paths, plan)?;
    let reply = request(&serve::apply_request(&bytes)?, reach).await?;
    let effect = serve::reply(&reply, "apply")?;
    Ok(Applied {
        file: name.to_owned(),
        effect,
    })
}

/// The status of each connector on the node that `reach` reaches. `reach` gives what
/// sends the request.
///
/// # Errors
///
/// - [`Error::Node`] with the failure of `reach` or of the request.
/// - Each error of [`serve::reply`].
pub(crate) async fn status<C>(
    reach: impl AsyncFnOnce() -> Result<C, Failure>,
) -> Result<status::Output, Error>
where
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let reply = request(serve::STATUS_REQUEST, reach).await?;
    serve::reply(&reply, "status")
}

/// The most bytes that one request holds.
fn most() -> usize {
    usize::try_from(BODY_BYTES_MAX)
        .expect("invariant: the bound of a body fits a usize")
}

/// The bytes of the file at `path`, of at most the bytes of one request.
fn file(paths: &Paths, path: &Path) -> Result<Vec<u8>, Error> {
    let unread = |cause| Error::Read {
        path: path.to_owned(),
        cause,
    };
    match paths.read(path, most()) {
        Ok(Found::File(bytes)) => Ok(bytes),
        Ok(Found::Large) => Err(Error::LargeRequest),
        Ok(Found::Directory(_)) => Err(unread(Unread::Directory)),
        Err(paths::Error::NotFound) => Err(unread(Unread::NotFound)),
        Err(paths::Error::Io { code }) => Err(unread(Unread::Io { code })),
        Err(paths::Error::Outside) => {
            unreachable!("invariant: the command line reads with no limit")
        }
    }
}

/// The reply of the node that `reach` reaches to `body`.
async fn request<C>(
    body: &[u8],
    reach: impl AsyncFnOnce() -> Result<C, Failure>,
) -> Result<Vec<u8>, Error>
where
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let send = reach().await.map_err(Error::Node)?;
    send(body).await.map_err(Error::Node)
}
