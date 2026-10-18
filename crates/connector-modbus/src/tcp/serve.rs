use std::future::poll_fn;
use std::io::IoSlice;
use std::pin::pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use connector::cancel::Token;
use env::net::{self, Listener, Tcp};

use crate::device::Device;
use crate::pdu::Exception;

/// Accepts streams on `listener` and answers each request frame on the newest from
/// `device`, until `until` is cancelled or the listener fails. A new stream drops the
/// one before it, as a device with one connection does, so that a client that lost
/// its stream with no close gets an answer when it comes back. A frame for another
/// unit than `unit` gets the exception "gateway target failed to respond", as a
/// gateway gives. A stream that fails or is out of step is dropped. Tests and
/// simulations use it as the far side of a connection.
///
/// It then drops `listener` and closes the stream that it answers with a FIN. An
/// answer in flight can be cut. The close waits on no peer: a shutdown that cannot
/// finish at once drops the stream, which resets it. Bytes of the peer that reach
/// the stream after its close reset it. On each return, each stream that it accepted
/// has ended. A drop of the future drops the stream that it answers, which resets it.
///
/// # Errors
///
/// The error of the listener, when it fails before `until` is cancelled.
///
/// # Panics
///
/// When a thread panicked while it held the lock on `device`.
pub async fn serve(
    mut listener: Listener,
    until: Token,
    unit: u8,
    device: &Mutex<Device>,
) -> Result<(), net::Error> {
    let stop = until.child();
    let mut wait = pin!(until.wait());
    let mut cancelled =
        |cx: &mut Context<'_>| until.cancelled() || wait.as_mut().poll(cx).is_ready();
    let first = poll_fn(|cx| next(&mut listener, cancelled(cx), cx));
    let Some(stream) = first.await? else {
        return Ok(());
    };
    let mut answering = pin!(answer(stream, unit, device, &stop));
    let mut ended = false;
    let accepted = loop {
        let accepted = poll_fn(|cx| {
            if !ended {
                ended = answering.as_mut().poll(cx).is_ready();
            }
            next(&mut listener, cancelled(cx), cx)
        });
        match accepted.await {
            Ok(Some(stream)) => {
                answering.set(answer(stream, unit, device, &stop));
                ended = false;
            }
            end => break end.map(drop),
        }
    };
    drop(listener);
    stop.cancel();
    if !ended {
        answering.await;
    }
    accepted
}

/// The next stream of `listener`, or `None` once `cancelled` and it has no stream
/// ready, or its error, which wins.
fn next(
    listener: &mut Listener,
    cancelled: bool,
    cx: &mut Context<'_>,
) -> Poll<Result<Option<Tcp>, net::Error>> {
    match listener.poll_accept(cx) {
        Poll::Ready(accepted) => Poll::Ready(accepted.map(Some)),
        Poll::Pending if cancelled => Poll::Ready(Ok(None)),
        Poll::Pending => Poll::Pending,
    }
}

/// Answers `stream` until it ends, fails, or is out of step, or until `stop`, which
/// closes it.
async fn answer(mut stream: Tcp, unit: u8, device: &Mutex<Device>, stop: &Token) {
    if stop
        .race(exchange(&mut stream, unit, device))
        .await
        .is_none()
    {
        poll_fn(|cx| {
            close(&mut stream, cx);
            Poll::Ready(())
        })
        .await;
    }
}

/// Reads `stream` until it has no bytes, then starts its shutdown, with no wait on
/// the peer. A drop of a stream with bytes unread, or before its shutdown is done,
/// resets it.
fn close(stream: &mut Tcp, cx: &mut Context<'_>) {
    let mut buffer = [0; 260];
    // An error: the stream has failed, and its peer sees it.
    while let Poll::Ready(Ok(1..)) = stream.poll_read(cx, &mut buffer) {}
    // An error, as of a read.
    drop(stream.poll_close(cx));
}

/// Answers each request frame on `stream` until it ends, fails, or is out of step.
async fn exchange(stream: &mut Tcp, unit: u8, device: &Mutex<Device>) {
    let (mut received, mut reply) = (Vec::new(), Vec::new());
    let mut buffer = [0; 260];
    loop {
        let frame = match super::decode(&received) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                match poll_fn(|cx| stream.poll_read(cx, &mut buffer)).await {
                    Ok(0) => {
                        // Errors leave nothing to do: the stream ends either way.
                        drop(poll_fn(|cx| stream.poll_close(cx)).await);
                        return;
                    }
                    Ok(n) => received.extend(buffer.iter().take(n)),
                    Err(_) => return,
                }
                continue;
            }
            Err(_) => return,
        };
        reply.clear();
        let start = super::open(frame.header, &mut reply);
        if frame.header.unit == unit {
            device
                .lock()
                .expect("no panic under the device lock")
                .answer(frame.pdu, &mut reply);
        } else {
            let function = *frame.pdu.first().expect("invariant: a PDU is not empty");
            Exception::GatewayTarget.write_to(function, &mut reply);
        }
        super::seal(&mut reply, start);
        let len = frame.len;
        received.drain(..len);
        if write(stream, &reply).await.is_err() {
            return;
        }
    }
}

/// Sends all of `bytes`.
async fn write(stream: &mut Tcp, bytes: &[u8]) -> Result<(), net::Error> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let parts = [IoSlice::new(rest)];
        let n = poll_fn(|cx| stream.poll_write(cx, &parts)).await?;
        rest = rest.get(n..).unwrap_or_default();
    }
    Ok(())
}
