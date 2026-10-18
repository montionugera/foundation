//! An `env` stream, plain or under TLS, as `hyper` I/O.

use std::cell::Cell;
use std::io::{self, IoSlice};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use env::net::Tcp;
use hyper::rt::{Read, ReadBufCursor, Write};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::{TlsConnector, TlsStream};

/// The most bytes one read copies into `hyper`'s buffer.
const READ_MAX: usize = 8192;

/// A TCP stream that `hyper` reads and writes, plain or under TLS. Its errors wrap
/// the `env` or the rustls error, so the client's error sources reach it.
pub(super) struct Stream {
    io: Io,
    /// The count of bytes that `hyper` read, which wraps.
    received: Rc<Cell<u64>>,
}

enum Io {
    Plain(Tcp),
    Tls(Box<TlsStream<Adapter>>),
}

impl Stream {
    /// `tcp` with no TLS. Each read adds its count to `received`.
    pub(super) fn plain(tcp: Tcp, received: Rc<Cell<u64>>) -> Self {
        Self {
            io: Io::Plain(tcp),
            received,
        }
    }

    /// Runs the client handshake of `config` for the server `name` over `tcp`. Each
    /// read adds its count to `received`.
    ///
    /// # Errors
    ///
    /// The rustls error when the server's certificate is not trusted or the server
    /// breaks TLS, the `env` error when the stream fails, and `UnexpectedEof` when the
    /// server closes the stream first.
    pub(super) async fn client(
        tcp: Tcp,
        config: Arc<ClientConfig>,
        name: ServerName<'static>,
        received: Rc<Cell<u64>>,
    ) -> io::Result<Self> {
        let tls = TlsConnector::from(config)
            .connect(name, Adapter(tcp))
            .await?;
        Ok(Self {
            io: Io::Tls(Box::new(tls.into())),
            received,
        })
    }

    /// Runs the server handshake of `config` over `tcp`.
    ///
    /// # Errors
    ///
    /// As [`Stream::client`], for the client.
    #[cfg(any(test, feature = "sim"))]
    pub(super) async fn server(
        tcp: Tcp,
        config: Arc<rustls::ServerConfig>,
    ) -> io::Result<Self> {
        let tls = tokio_rustls::TlsAcceptor::from(config)
            .accept(Adapter(tcp))
            .await?;
        Ok(Self {
            io: Io::Tls(Box::new(tls.into())),
            received: Rc::default(),
        })
    }
}

impl Read for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        let mut bytes = [0; READ_MAX];
        let Some(bytes) = bytes.get_mut(..buf.remaining().min(READ_MAX)) else {
            unreachable!("the length is at most READ_MAX")
        };
        if bytes.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let stream = self.get_mut();
        let n = match &mut stream.io {
            Io::Plain(tcp) => {
                ready!(tcp.poll_read(cx, bytes)).map_err(io::Error::other)?
            }
            Io::Tls(tls) => {
                let mut read = ReadBuf::new(bytes);
                ready!(Pin::new(&mut **tls).poll_read(cx, &mut read))?;
                read.filled().len()
            }
        };
        buf.put_slice(bytes.get(..n).expect("a read fits its buffer"));
        let n = u64::try_from(n).expect("a read fits in u64");
        stream.received.set(stream.received.get().wrapping_add(n));
        Poll::Ready(Ok(()))
    }
}

impl Write for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored(cx, &[IoSlice::new(buf)])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().io {
            Io::Plain(tcp) => tcp.poll_write(cx, bufs).map_err(io::Error::other),
            Io::Tls(tls) => Pin::new(&mut **tls).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Plain(_) => Poll::Ready(Ok(())),
            Io::Tls(tls) => Pin::new(&mut **tls).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Plain(tcp) => tcp.poll_close(cx).map_err(io::Error::other),
            Io::Tls(tls) => Pin::new(&mut **tls).poll_shutdown(cx),
        }
    }
}

/// `Tcp` as Tokio I/O, for `tokio-rustls`.
struct Adapter(Tcp);

impl AsyncRead for Adapter {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = ready!(self.get_mut().0.poll_read(cx, buf.initialize_unfilled()))
            .map_err(io::Error::other)?;
        buf.advance(n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Adapter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored(cx, &[IoSlice::new(buf)])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.get_mut()
            .0
            .poll_write(cx, bufs)
            .map_err(io::Error::other)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut().0.poll_close(cx).map_err(io::Error::other)
    }
}
