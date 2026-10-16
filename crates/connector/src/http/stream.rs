//! An `env` stream, plain or under TLS, as `hyper` I/O.

use std::cell::Cell;
use std::future::poll_fn;
use std::io::{self, IoSlice, Read as _, Write as _};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, ready};

use env::net::Tcp;
use hyper::rt::{Read, ReadBufCursor, Write};
use rustls::Connection;

/// The most bytes one read copies into `hyper`'s buffer.
const READ_MAX: usize = 8192;

/// A TCP stream that `hyper` reads and writes, plain or under TLS. Its errors wrap
/// the `env` or the rustls error, so the client's error sources reach it.
pub(super) struct Stream {
    pub(super) tcp: Tcp,
    /// The TLS session over `tcp`, or `None` for plain bytes.
    pub(super) tls: Option<Box<Connection>>,
    /// The count of bytes that `hyper` read, which wraps.
    pub(super) received: Rc<Cell<u64>>,
}

impl Stream {
    /// Runs the TLS handshake to its end. A stream with no TLS has none.
    ///
    /// # Errors
    ///
    /// The rustls error when the peer's certificate is not trusted or the peer
    /// breaks TLS, the `env` error when the stream fails, and `UnexpectedEof` when the
    /// peer closes the stream first.
    pub(super) async fn handshake(&mut self) -> io::Result<()> {
        let Some(tls) = self.tls.as_deref_mut() else {
            return Ok(());
        };
        let tcp = &mut self.tcp;
        poll_fn(|cx| {
            loop {
                ready!(flush(tcp, tls, cx))?;
                if !tls.is_handshaking() {
                    return Poll::Ready(Ok(()));
                }
                if ready!(fill(tcp, tls, cx))? == 0 {
                    let closed = "the peer closed the stream in the handshake";
                    let error = io::Error::new(io::ErrorKind::UnexpectedEof, closed);
                    return Poll::Ready(Err(error));
                }
            }
        })
        .await
    }

    /// Reads plaintext into `bytes`. Gives 0 at the end of the stream.
    fn poll_plain(
        &mut self,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let Some(tls) = self.tls.as_deref_mut() else {
            return self.tcp.poll_read(cx, bytes).map_err(io::Error::other);
        };
        loop {
            match tls.reader().read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                read => return Poll::Ready(read),
            }
            ready!(fill(&mut self.tcp, tls, cx))?;
        }
    }
}

/// Writes the TLS records that `tls` holds to `tcp`, until none is left.
fn flush(
    tcp: &mut Tcp,
    tls: &mut Connection,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    while tls.wants_write() {
        match tls.write_tls(&mut Io { tcp, cx }) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Poll::Pending;
            }
            Err(error) => return Poll::Ready(Err(error)),
        }
    }
    Poll::Ready(Ok(()))
}

/// Reads TLS records from `tcp` into `tls`, and processes them. Gives the count of
/// bytes read, 0 at the end of the stream.
fn fill(
    tcp: &mut Tcp,
    tls: &mut Connection,
    cx: &mut Context<'_>,
) -> Poll<io::Result<usize>> {
    let n = match tls.read_tls(&mut Io { tcp, cx }) {
        Ok(n) => n,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            return Poll::Pending;
        }
        Err(error) => return Poll::Ready(Err(error)),
    };
    let processed = tls.process_new_packets();
    Poll::Ready(processed.map(|_| n).map_err(io::Error::other))
}

/// `tcp` in `cx` as blocking I/O for rustls, where a stream that is not ready gives
/// `WouldBlock`.
struct Io<'a, 'b> {
    tcp: &'a mut Tcp,
    cx: &'a mut Context<'b>,
}

impl io::Read for Io<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self.tcp.poll_read(self.cx, bytes) {
            Poll::Ready(read) => read.map_err(io::Error::other),
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

impl io::Write for Io<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_vectored(&[IoSlice::new(bytes)])
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        match self.tcp.poll_write(self.cx, bufs) {
            Poll::Ready(written) => written.map_err(io::Error::other),
            Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
        stream.poll_plain(cx, bytes).map(|read| {
            let n = read?;
            buf.put_slice(bytes.get(..n).expect("a read fits its buffer"));
            let n = u64::try_from(n).expect("a read fits in u64");
            stream.received.set(stream.received.get().wrapping_add(n));
            Ok(())
        })
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
        let stream = self.get_mut();
        let Some(tls) = stream.tls.as_deref_mut() else {
            return stream.tcp.poll_write(cx, bufs).map_err(io::Error::other);
        };
        // rustls takes at most its buffer limit, 64 KiB, and none while the buffer is
        // full, so the records it holds go out first.
        ready!(flush(&mut stream.tcp, tls, cx))?;
        let n = tls.writer().write_vectored(bufs)?;
        if let Poll::Ready(Err(error)) = flush(&mut stream.tcp, tls, cx) {
            return Poll::Ready(Err(error));
        }
        Poll::Ready(Ok(n))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let stream = self.get_mut();
        match stream.tls.as_deref_mut() {
            Some(tls) => flush(&mut stream.tcp, tls, cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let stream = self.get_mut();
        if let Some(tls) = stream.tls.as_deref_mut() {
            tls.send_close_notify();
            ready!(flush(&mut stream.tcp, tls, cx))?;
        }
        stream.tcp.poll_close(cx).map_err(io::Error::other)
    }
}
