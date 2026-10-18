//! How a program on the node's host reaches the node: the node holds the file
//! [`sector::address`] of the data directory while shard 0 serves the node's port, and
//! after a crash, and writes it empty after a clean stop.

use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;

use env::files::{File, Files, Mode};
pub(crate) use sector::address::Address;
use sector::address::{FILE, LEN, encode};

use crate::Error;

/// Where `port` is bound: its UDP socket.
pub(crate) fn bound(port: &transport::Port) -> SocketAddr {
    let udp = port
        .addresses()
        .into_iter()
        .find_map(|address| match address {
            transport::Address::Udp(at) => Some(at),
            transport::Address::Tcp(_) | transport::Address::Relay { .. } => None,
        });
    udp.expect("invariant: a port binds a UDP socket")
}

/// Holds the file `address` of `files` with `address` while `serve` runs: writes it in
/// place, awaits `serve`, and writes it empty. The first hold on a data directory
/// makes the file, and a later one writes in its room, so it needs no free bytes.
/// Writes over a file of another length after it removes it. Does not read the file.
/// When the open or the write fails, does not poll `serve`, and writes the file empty.
/// Does not drop `serve`: the caller drops it after it has read the error. Call it
/// under the lock of the data directory.
///
/// # Errors
///
/// [`Error::Directory`] for a file call that fails: one of the remove, the open, or
/// the write, else the empty write.
pub(crate) async fn hold(
    files: &Files,
    address: &Address,
    serve: Pin<&mut impl Future<Output = ()>>,
) -> Result<(), Error> {
    let written = write(files, address).await;
    let emptied = match &written {
        Ok(file) => {
            serve.await;
            sector::clear::<LEN>(file).await
        }
        Err(_) => empty(files).await,
    };
    written.and(emptied).map_err(Error::Directory)
}

/// Writes `address` in the file, made of zero bytes when it is not there, and made
/// again when it has another length.
async fn write(files: &Files, address: &Address) -> Result<File, env::files::Error> {
    let path = Path::new(FILE);
    let mode = Mode::Create { len: LEN as u64 };
    let file = match files.open(path, mode).await {
        Err(env::files::Error::Length { .. }) => {
            files.remove(path).await?;
            files.open(path, mode).await?
        }
        opened => opened?,
    };
    sector::write(files, &file, &encode(address)).await?;
    Ok(file)
}

/// Writes the file empty after a failed open or write, which can leave the address
/// of a crash or of this write. A file of another length or with no bytes holds no
/// address. Opens it again, since a failed sync poisons the handle.
async fn empty(files: &Files) -> Result<(), env::files::Error> {
    match files.open(Path::new(FILE), Mode::Write).await? {
        file if file.len() == LEN as u64 => sector::clear::<LEN>(&file).await,
        _ => Ok(()),
    }
}
