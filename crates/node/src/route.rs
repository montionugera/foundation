//! Shard 0's sessions: each stream goes to the server of the protocol its header names.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::poll_fn;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::rc::{Rc, Weak};
use std::task::Poll;

use hub::{Link, Served};
use transport::stream::Incoming;
use transport::{Code, Error, Peer, Session, Transport};
use types::time::Span;
use wire::Protocol;

use crate::scope::Scope;
use crate::task::Handles;

/// Serves each session of `transport` in its own future on `tasks`, until the transport
/// stops, and gives the error that stopped it. Admits each session of a member of the
/// region of the mesh of `handles.ops`. Gives each session of another peer a place, of
/// at most `places_max`; at the bound, the oldest place frees for it
/// ([`Places::take`]). `route` decides each stream with `handles`, and rejects one
/// whose header takes longer than `header_wait`.
pub(crate) async fn accept(
    transport: Rc<Transport>,
    handles: Handles,
    clock: env::clock::Clock,
    tasks: env::tasks::Tasks,
    header_wait: Span,
    places_max: NonZeroUsize,
) -> Error {
    let mut sessions = Scope::new(tasks.clone());
    let mut places = Places::new(places_max);
    loop {
        let session = match transport.accept().await {
            Ok(session) => session,
            Err(error) => return error,
        };
        let ops = handles.ops.as_deref();
        let held = places.take(&session, |peer| member(peer, ops));
        let link = handles.hub.link(session.clone());
        let serve = serve(
            session,
            link,
            handles.ops.clone(),
            clock.clone(),
            header_wait,
            tasks.clone(),
        );
        sessions.spawn(Box::pin(async move {
            serve.await;
            drop(held);
        }));
    }
}

/// A place that a peer outside the region took, held while a session of it runs.
struct Place {
    peer: Peer,
    /// Each session of the peer that runs, by serial. A node key can have two open:
    /// the transport keeps the older session when the newer one has already ended.
    sessions: RefCell<Vec<(u64, Session)>>,
}

/// A session's hold on its place. The place frees when its last hold drops.
pub(crate) struct Hold {
    place: Rc<Place>,
    serial: u64,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut sessions = self.place.sessions.borrow_mut();
        sessions.retain(|(serial, _)| *serial != self.serial);
    }
}

/// The places that the peers outside the region hold, oldest first.
pub(crate) struct Places {
    held: VecDeque<Weak<Place>>,
    serial: u64,
    max: NonZeroUsize,
}

impl Places {
    /// No places held, of at most `max`.
    pub(crate) const fn new(max: NonZeroUsize) -> Self {
        Self {
            held: VecDeque::new(),
            serial: 0,
            max,
        }
    }

    /// The hold of `session` on its place: the place that its node key still holds,
    /// which keeps its age, else a new place. `None` when `member` holds for the peer.
    /// Each place whose peer `member` now holds frees, and its sessions stay. At `max`
    /// places, the oldest place frees first, and each session that holds it closes with
    /// `wire::session::REFUSED`.
    pub(crate) fn take(
        &mut self,
        session: &Session,
        member: impl Fn(Peer) -> bool,
    ) -> Option<Hold> {
        let peer = session.peer();
        if member(peer) {
            return None;
        }
        self.held
            .retain(|place| place.upgrade().is_some_and(|place| !member(place.peer)));
        let kept = match peer {
            Peer::Node(_) => (self.held.iter().filter_map(Weak::upgrade))
                .find(|place| place.peer == peer),
            Peer::Client => None,
        };
        let place = kept.unwrap_or_else(|| {
            if self.held.len() >= self.max.get()
                && let Some(oldest) =
                    self.held.pop_front().and_then(|old| old.upgrade())
            {
                for (_, open) in oldest.sessions.borrow().iter() {
                    open.close(Code(wire::session::REFUSED));
                }
            }
            let place = Rc::new(Place {
                peer,
                sessions: RefCell::default(),
            });
            self.held.push_back(Rc::downgrade(&place));
            place
        });
        let serial = self.serial;
        self.serial += 1;
        place.sessions.borrow_mut().push((serial, session.clone()));
        Some(Hold { place, serial })
    }
}

/// Routes each stream that the peer of `session` opens, each in its own future on
/// `tasks`, so a stream whose header is late delays no other. Ends when the session
/// ends.
async fn serve(
    session: Session,
    link: Link,
    ops: Option<Rc<ops::Node>>,
    clock: env::clock::Clock,
    header_wait: Span,
    tasks: env::tasks::Tasks,
) {
    let mut streams = Scope::new(tasks);
    while let Ok(incoming) = session.accept().await {
        let peer = session.peer();
        let link = link.clone();
        let route = route(
            incoming,
            peer,
            link,
            ops.clone(),
            clock.clone(),
            header_wait,
        );
        streams.spawn(Box::pin(route));
    }
}

/// Reads the header of `incoming`, its first message, and routes the stream that `peer`
/// opened by its protocol. With `ops`, a `Mesh` stream of a node goes to the mesh of
/// `ops`, a `Hub` stream of a member of its region to `link`, and a `Hub` stream of a
/// program to `link`, with its request answered by `ops`. Each other stream, and one
/// whose header does not arrive within `header_wait` on `clock`, is rejected.
async fn route(
    mut incoming: Incoming,
    peer: Peer,
    link: Link,
    ops: Option<Rc<ops::Node>>,
    clock: env::clock::Clock,
    header_wait: Span,
) {
    let first = {
        let mut recv = pin!(incoming.receiver.recv());
        let mut late = pin!(clock.sleep(header_wait));
        poll_fn(|cx| match recv.as_mut().poll(cx) {
            Poll::Ready(first) => Poll::Ready(Some(first)),
            Poll::Pending => late.as_mut().poll(cx).map(|()| None),
        })
        .await
    };
    let first = match first {
        Some(Ok(first)) => first,
        Some(Err(_)) => return,
        None => return reject(incoming),
    };
    let Some(protocol) = first.as_deref().and_then(header) else {
        return reject(incoming);
    };
    match protocol {
        Protocol::Mesh => match (peer, ops) {
            (Peer::Node(key), Some(ops)) => {
                // `serve` stops the stream with the code of its error.
                drop(ops.mesh().serve(key, incoming).await);
            }
            (Peer::Client, _) | (_, None) => reject(incoming),
        },
        Protocol::Hub if member(peer, ops.as_deref()) => {
            // `serve` stops the stream with the code of its error.
            drop(link.serve(incoming).await);
        }
        Protocol::Hub => match (peer, ops) {
            (Peer::Client, Some(ops)) => answer(&link, incoming, &ops).await,
            (Peer::Client, None) | (Peer::Node(_), _) => reject(incoming),
        },
        Protocol::Clock | Protocol::Replica | Protocol::Blob => reject(incoming),
    }
}

/// Serves the `Hub` stream `incoming` of a program on `link`, and answers its request,
/// when it is one, with the reply of `ops`.
async fn answer(link: &Link, incoming: Incoming, ops: &ops::Node) {
    // `serve` stops the stream with the code of its error.
    let Ok(Served::Request(request)) = link.serve(incoming).await else {
        return;
    };
    let subject = &request.admitted.hello.subject;
    let reply = ops::serve(ops, subject, &request.body).await;
    // `send` stops the stream with the code of its error. `ops::serve` keeps the reply
    // within the bound of a body, so `send` does not panic.
    drop(request.reply.send(&reply).await);
}

/// Whether `peer` is a member of the region: a node whose key a member holds in the
/// view of the mesh of `ops`.
fn member(peer: Peer, ops: Option<&ops::Node>) -> bool {
    match (peer, ops) {
        (Peer::Node(key), Some(ops)) => ops.mesh().holder(key).is_some(),
        (Peer::Node(_), None) | (Peer::Client, _) => false,
    }
}

/// The protocol that `message` names when it is a whole header, with no byte after it.
fn header(message: &[u8]) -> Option<Protocol> {
    match wire::header::decode(message) {
        Ok((protocol, [])) => Some(protocol),
        Ok(_) | Err(_) => None,
    }
}

/// Stops `incoming`, and resets its reply half, with the code of a rejected header.
fn reject(incoming: Incoming) {
    let code = Code(wire::header::REJECTED);
    incoming.receiver.stop(code);
    if let Some(sender) = incoming.sender {
        sender.reset(code);
    }
}

#[cfg(test)]
mod tests {
    use wire::Protocol;

    use super::header;

    /// A first message is a header only when it is the whole message.
    #[test]
    fn a_header_with_a_byte_after_it_names_no_protocol() {
        let mut message = wire::header::encode(Protocol::Mesh).to_vec();
        assert_eq!(header(&message), Some(Protocol::Mesh));
        message.push(0);
        assert_eq!(header(&message), None);
    }
}
