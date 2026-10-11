//! Shard 0's sessions: each stream goes to the server of the protocol its header names.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::poll_fn;
use std::pin::pin;
use std::rc::{Rc, Weak};
use std::task::Poll;

use hub::{Hub, Link};
use mesh::Mesh;
use transport::stream::Incoming;
use transport::{Code, Error, Peer, Session, Transport};
use types::time::Span;
use wire::Protocol;

use crate::scope::Scope;

/// How long a stream may take to give its header, a patch as [`crate::WINDOW`] is.
const HEADER: Span = Span::from_nanos(10_000_000_000);

/// How many places the peers outside the region hold at once, a patch as [`HEADER`] is.
/// A program holds one per session, and a node one for its key.
const PLACES: usize = 256;

/// Serves each session of `transport` in its own future on `tasks`, until the transport
/// stops, and gives the error that stopped it. Admits each session of a member of
/// `mesh`'s region. Gives each session of another peer a place, of at most
/// [`PLACES`]; at the bound, the oldest place frees for it ([`Places::take`]).
/// `route` decides each stream.
pub(crate) async fn accept(
    transport: Rc<Transport>,
    mesh: Option<Mesh>,
    hub: Hub,
    clock: env::clock::Clock,
    tasks: env::tasks::Tasks,
) -> Error {
    let mut sessions = Scope::new(tasks.clone());
    let mut places = Places::default();
    loop {
        let session = match transport.accept().await {
            Ok(session) => session,
            Err(error) => return error,
        };
        let held = if member(session.peer(), mesh.as_ref()) {
            None
        } else {
            Some(places.take(&session, |peer| member(peer, mesh.as_ref())))
        };
        let link = hub.link(session.clone());
        let serve = serve(session, mesh.clone(), link, clock.clone(), tasks.clone());
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
#[derive(Default)]
pub(crate) struct Places {
    held: VecDeque<Weak<Place>>,
    serial: u64,
}

impl Places {
    /// The hold of `session`, whose peer is not a member, on its place: the place that
    /// its node key still holds, which keeps its age, else a new place. At [`PLACES`],
    /// the oldest place frees first. When `member` holds for the peer of that place
    /// now, its sessions stay; else each closes with `wire::session::REFUSED`.
    pub(crate) fn take(
        &mut self,
        session: &Session,
        member: impl Fn(Peer) -> bool,
    ) -> Hold {
        self.held.retain(|place| place.strong_count() > 0);
        let peer = session.peer();
        let kept = match peer {
            Peer::Node(_) => (self.held.iter().filter_map(Weak::upgrade))
                .find(|place| place.peer == peer),
            Peer::Client => None,
        };
        let place = kept.unwrap_or_else(|| {
            if self.held.len() >= PLACES
                && let Some(oldest) =
                    self.held.pop_front().and_then(|old| old.upgrade())
                && !member(oldest.peer)
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
        Hold { place, serial }
    }
}

/// Routes each stream that the peer of `session` opens, each in its own future on
/// `tasks`, so a stream whose header is late delays no other. Ends when the session
/// ends.
async fn serve(
    session: Session,
    mesh: Option<Mesh>,
    link: Link,
    clock: env::clock::Clock,
    tasks: env::tasks::Tasks,
) {
    let mut streams = Scope::new(tasks);
    while let Ok(incoming) = session.accept().await {
        let peer = session.peer();
        let route = route(incoming, peer, mesh.clone(), link.clone(), clock.clone());
        streams.spawn(Box::pin(route));
    }
}

/// Reads the header of `incoming`, its first message, and routes the stream that `peer`
/// opened by its protocol. A `Mesh` stream of a node goes to `mesh`, and a `Hub` stream
/// of a member of the region to `link`; each other stream, and one whose header does
/// not arrive within [`HEADER`] on `clock`, is rejected.
async fn route(
    mut incoming: Incoming,
    peer: Peer,
    mesh: Option<Mesh>,
    link: Link,
    clock: env::clock::Clock,
) {
    let first = {
        let mut recv = pin!(incoming.receiver.recv());
        let mut late = pin!(clock.sleep(HEADER));
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
        Protocol::Mesh => match (peer, mesh) {
            (Peer::Node(key), Some(mesh)) => {
                // `serve` stops the stream with the code of its error.
                drop(mesh.serve(key, incoming).await);
            }
            (Peer::Client, _) | (_, None) => reject(incoming),
        },
        Protocol::Hub if member(peer, mesh.as_ref()) => {
            // `serve` stops the stream with the code of its error.
            drop(link.serve(incoming).await);
        }
        // A program's `Hub` stream waits until `node` handles a `Served::Request`
        // (#1744).
        Protocol::Hub | Protocol::Clock | Protocol::Replica | Protocol::Blob => {
            reject(incoming);
        }
    }
}

/// Whether `peer` is a member of the region: a node whose key a member holds in the
/// view of `mesh`.
fn member(peer: Peer, mesh: Option<&Mesh>) -> bool {
    match (peer, mesh) {
        (Peer::Node(key), Some(mesh)) => mesh.holder(key).is_some(),
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
