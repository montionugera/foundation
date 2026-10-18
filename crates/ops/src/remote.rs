//! Runs `plan`, `apply`, `status`, `browse`, and `import` of a command line on its
//! node.

use std::path::{Path, PathBuf};

use crate::Failure;
use crate::apply::Applied;
use crate::error::{self, Error, Unread};
use crate::front_end;
use crate::plan::Output;
use crate::scope::{Scope, utf8};
use crate::{browse, import, operation, serve, status};

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
/// through `scope`, and writes the plan file to `out` when it is given. A directory of
/// `files` gives its config files ([`Scope::config`]).
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` for each path that is not UTF-8, or
///   each such file of the first directory with one.
/// - [`Error::Read`] for the first file that does not read as UTF-8 text, or a
///   directory whose entries pass the bytes of one read.
/// - [`Error::NoConfig`] for the first directory that gives no file.
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
    utf8(files)?;
    if let Some(out) = out {
        scope.out(out)?;
    }
    let read = scope.config(files)?;
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
/// - [`Error::Outside`], [`Error::Hidden`], or [`Error::FolderChanged`] when `plan`
///   is past the limits of `scope`.
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

/// Browses on the node of `link` the device of the connector of `input`, with the
/// config files of `input` read through `scope`.
///
/// # Errors
///
/// - [`Error::Config`] with `ops.path-not-utf8` for each path that is not UTF-8.
/// - [`Error::Read`] for the first file that does not read as UTF-8 text.
/// - [`Error::LargeRequest`] when the files do not fit one request.
/// - [`Error::Outside`], [`Error::Hidden`], [`Error::NoData`], or
///   [`Error::FolderChanged`] for the first path past the limits of `scope`.
/// - [`Error::Node`] with the failure of the reach or of the request.
/// - Each error of [`serve::reply`].
pub(crate) async fn browse<R, C>(
    input: operation::Browse,
    scope: &Scope,
    link: &mut Link<R, C>,
) -> Result<browse::Output, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    utf8(&input.files)?;
    let asked = serve::Browse {
        connector: input.connector,
        files: scope.config(&input.files)?,
        start: input.start,
        pattern: input.pattern,
        cursor: input.cursor,
        values: input.values,
    };
    let reply = request(&serve::browse_request(&asked)?, link).await?;
    serve::reply(&reply, "browse")
}

/// Imports on the node of `link` the points of `input` into the config file of its
/// connector, read and written through `scope`. It writes the new text only when the
/// file still holds the text that it sent, and writes nothing when the text is the
/// same. A regular file becomes a new file of mode 0600; a symbolic link takes the
/// text in place (ENV SEAMS).
///
/// # Errors
///
/// - [`Error::FileChanged`] when the file changed after the read: it holds other
///   text, holds more than the bytes of one request, is gone, or is a directory.
/// - [`Error::Reply`] when the reply names a file that the request did not send.
/// - [`Error::Write`] when the write fails.
/// - Each error of [`browse`], with [`serve::import_reply`] in place of
///   [`serve::reply`].
pub(crate) async fn import<R, C>(
    input: operation::Import,
    scope: &Scope,
    link: &mut Link<R, C>,
) -> Result<import::Output, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    utf8(&input.files)?;
    let asked = serve::Import {
        connector: input.connector,
        files: scope.config(&input.files)?,
        start: input.start,
        points: input.points,
        pattern: input.pattern,
    };
    let reply = request(&serve::import_request(&asked)?, link).await?;
    let (output, text) = serve::import_reply(&reply)?;
    let Some(sent) = asked
        .files
        .iter()
        .find(|file| file.path.to_str() == Some(output.file.as_str()))
    else {
        return Err(Error::Reply {
            message: "the reply of `import` names a file that the request did not send"
                .to_owned(),
        });
    };
    if text == sent.text {
        return Ok(output);
    }
    match scope.read(&sent.path) {
        Ok(bytes) if bytes == sent.text.as_bytes() => {}
        Ok(_)
        | Err(
            Error::LargeRequest
            | Error::Read {
                cause: Unread::NotFound | Unread::Directory,
                ..
            },
        ) => {
            return Err(Error::FileChanged {
                path: sent.path.clone(),
            });
        }
        Err(error) => return Err(error),
    }
    scope.write(&sent.path, text.as_bytes())?;
    Ok(output)
}

/// The reply of the node of `link` to `body`.
async fn request<R, C>(body: &[u8], link: &mut Link<R, C>) -> Result<Vec<u8>, Error>
where
    R: AsyncFnMut() -> Result<C, Failure>,
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    link.send(body).await.map_err(Error::Node)
}
