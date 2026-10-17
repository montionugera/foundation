//! Runs `plan`, `apply`, and `status` of a command line on its node.

use std::path::{Path, PathBuf};

use crate::Failure;
use crate::apply::Applied;
use crate::error::{self, Error, Unread};
use crate::front_end::{self, File};
use crate::plan::Output;
use crate::scope::{Scope, most};
use crate::{serve, status};

#[cfg(test)]
mod tests;

/// The link to one node: what reaches it, and the session of its last reach.
pub(crate) struct Link<R, C> {
    reach: R,
    session: Option<C>,
}

impl<R, C> Link<R, C>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    /// A link that calls `reach` at its first send.
    pub(crate) fn new(reach: R) -> Self {
        Self {
            reach,
            session: None,
        }
    }

    /// The reply of the node to `body`. It calls `reach` when it has no session, and
    /// keeps the session until a send through it fails.
    async fn send(&mut self, body: &[u8]) -> Result<Vec<u8>, Failure> {
        let send = match &self.session {
            Some(send) => send,
            None => self.session.insert((self.reach)().await?),
        };
        let reply = send(body).await;
        if reply.is_err() {
            self.session = None;
        }
        reply
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
/// - [`Error::Outside`], [`Error::Hidden`], [`Error::NoData`], or
///   [`Error::FolderChanged`] for the first path past the limits of `scope`.
/// - [`Error::PlanName`] when `scope` has limits and `out` does not resolve to a name
///   that ends in `.plan`, before it reads a file.
pub(crate) async fn plan<R, C>(
    files: &[PathBuf],
    out: Option<&Path>,
    scope: &Scope,
    link: &mut Link<R, C>,
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
    if let Some(out) = out {
        scope.out(out)?;
    }
    let mut total = 0;
    let mut read = Vec::with_capacity(files.len());
    for path in files {
        let bytes = scope.read(path)?;
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
        scope.write(out, plan)?;
    }
    Ok(output)
}

/// Applies on the node of `link` the plan file at `plan`, read through `scope`.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` when `plan` is not UTF-8.
/// - [`Error::Read`] when the plan file does not read.
/// - [`Error::Outside`], [`Error::Hidden`], [`Error::NoData`], or
///   [`Error::FolderChanged`] when `plan` is past the limits of `scope`.
/// - [`Error::LargeRequest`] when the plan file does not fit one request.
/// - [`Error::Node`] with the failure of the reach or of the request.
/// - Each error of [`serve::reply`].
pub(crate) async fn apply<R, C>(
    plan: &Path,
    scope: &Scope,
    link: &mut Link<R, C>,
) -> Result<Applied, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let Some(name) = plan.to_str() else {
        let not_utf8 = vec![front_end::not_utf8(plan)];
        return Err(Error::config(not_utf8, &error::Sources::default()));
    };
    let bytes = scope.read(plan)?;
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
pub(crate) async fn status<R, C>(link: &mut Link<R, C>) -> Result<status::Output, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let reply = request(serve::STATUS_REQUEST, link).await?;
    serve::reply(&reply, "status")
}

/// The reply of the node of `link` to `body`.
async fn request<R, C>(body: &[u8], link: &mut Link<R, C>) -> Result<Vec<u8>, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    link.send(body).await.map_err(Error::Node)
}
