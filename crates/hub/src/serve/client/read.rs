//! The read stream of a client session.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;

use access::proof;
use transport::Class;
use transport::stream::{Receiver, Sender};
use types::channel::Slot;
use types::name::Name;
use wire::hub::client::{Mode, Open, Places, Read, Refusal, place};

use super::{Reserved, body, verify};
use crate::State;
use crate::link::Session;
use crate::reader::{self, Home, Matched, Reader};
use crate::serve::{Error, Event, Layout, alloc, reply};

/// Serves the read of `subject` that `read` starts: verifies its open, opens its
/// reader, and sends its places, then each frame of the reader until the program
/// finishes the stream or the reader ends.
pub(super) async fn serve(
    session: &Session,
    class: Class,
    subject: Name,
    read: Read,
    receiver: &mut Receiver,
    sender: &mut Sender,
) -> Result<(), Error> {
    let (reader, matched, slots) =
        open(session, class, subject, read, receiver).await?;
    let layout = start(&session.state, sender, &matched, slots).await?;
    relay(&session.state, reader, layout, receiver, sender).await
}

/// Reads and verifies the open, and opens its reader at this node, the home of its
/// index, with the slot of each place.
async fn open(
    session: &Session,
    class: Class,
    subject: Name,
    read: Read,
    receiver: &mut Receiver,
) -> Result<(Reader, Matched, Box<[Slot]>), Error> {
    let state = &session.state;
    // A read takes no turn, and its open holds its bytes only until it decodes.
    let open = {
        let _body = Reserved::new(state, subject.clone(), read.length)?;
        let bytes = body(receiver, read.body()).await?;
        verify(session, proof::Kind::Read, &bytes, &read.signature)?;
        Open::decode(&bytes)?
    };
    let (wanted, mode) = match open.mode {
        Mode::Latest => (Class::Latest, reader::Mode::Latest),
        Mode::Complete => (Class::Complete, reader::Mode::Complete),
    };
    if class != wanted {
        return Err(Error::Class(class));
    }
    let refused = |error| Error::Reader(Box::new(error));
    let (matched, home) = reader::matched(state, &open.select)
        .await
        .map_err(refused)?;
    let slots = match home {
        Home::Here(slots) => slots,
        Home::Away(home) => return Err(refused(reader::Error::Remote { home })),
    };
    let reader = Reader::here(state, &matched, slots.clone(), mode).map_err(refused)?;
    Ok((reader, matched, slots))
}

/// Sends the places of `matched`, their run, and `Opened`, and gives the layout of
/// the frames at those places, with `slots`.
async fn start(
    state: &RefCell<State>,
    sender: &mut Sender,
    matched: &Matched,
    slots: Box<[Slot]>,
) -> Result<Layout, Error> {
    let mut block = alloc(state, Places::LEN)?;
    Places {
        count: u32::try_from(matched.places.len())
            .expect("invariant: each place has its own slot, a u32"),
        index: u32::try_from(matched.index)
            .expect("invariant: the index is one of the places"),
    }
    .encode(&mut block);
    sender.send(block.freeze()).await?;
    let most = sender.bytes_max().min(state.borrow().home.pool().largest());
    for message in place::messages(&matched.places, most) {
        let mut block = alloc(state, message.encoded_len())?;
        message.encode(&mut block);
        sender.send(block.freeze()).await?;
    }
    sender.send(reply(state, wire::hub::Reply::Opened)?).await?;
    let index = slots[matched.index];
    Ok(Layout::new(slots, index))
}

/// Sends each frame of `reader` until the program finishes the stream, or `Behind`
/// and the finish when the reader missed a frame.
async fn relay(
    state: &RefCell<State>,
    mut reader: Reader,
    mut layout: Layout,
    receiver: &mut Receiver,
    sender: &mut Sender,
) -> Result<(), Error> {
    let mut program = pin!(async {
        match receiver.recv().await? {
            None => Ok(()),
            Some(_) => Err(Error::Client(wire::hub::client::Error::Trailing)),
        }
    });
    loop {
        let event = {
            let mut next = pin!(reader.next());
            // The program polls first, so the node takes no frame after its message.
            poll_fn(|cx| match program.as_mut().poll(cx) {
                Poll::Ready(finished) => Poll::Ready(Event::Finished(finished)),
                Poll::Pending => next.as_mut().poll(cx).map(Event::Frame),
            })
            .await
        };
        match event {
            Event::Finished(finished) => {
                finished?;
                sender.finish()?;
                return Ok(());
            }
            Event::Frame(Ok(taken)) => {
                layout
                    .send(state, sender, taken.view().frame(), taken.set())
                    .await?;
            }
            Event::Frame(Err(reader::Ended::Behind)) => {
                sender.send(reply(state, wire::hub::Reply::Behind)?).await?;
                sender.finish()?;
                return Ok(());
            }
            Event::Frame(Err(ended)) => return Err(Error::Ended(Box::new(ended))),
        }
    }
}

/// The refusal that stops a read whose reader did not open with `error`.
///
/// # Panics
///
/// For `Unsynced`, which only a named reader gives.
pub(in crate::serve) fn refused(error: &reader::Error) -> Refusal {
    match error {
        reader::Error::Empty => Refusal::Unknown,
        reader::Error::Remote { .. } => Refusal::NotHome,
        reader::Error::ManyIndexes { .. } => Refusal::Malformed,
        reader::Error::Pool(_) => Refusal::Busy,
        reader::Error::Refused(refusal) => from_home(*refusal),
        reader::Error::Mesh(_)
        | reader::Error::Transport(_)
        | reader::Error::Message(_) => Refusal::Failed,
        reader::Error::Unsynced => {
            panic!("invariant: an unnamed reader does not give {error:?}")
        }
    }
}

/// The refusal that stops a read whose reader ended as `ended`.
///
/// # Panics
///
/// For `Behind`, which a read sends as a reply, and `Replaced`, which only a named
/// reader gives.
pub(in crate::serve) fn ended(ended: &reader::Ended) -> Refusal {
    match ended {
        reader::Ended::Removed(_) => Refusal::Unknown,
        reader::Ended::Pool(_) => Refusal::Busy,
        reader::Ended::Refused(refusal) => from_home(*refusal),
        reader::Ended::Buffer(_)
        | reader::Ended::Stream(_)
        | reader::Ended::Message(_)
        | reader::Ended::Frame(_)
        | reader::Ended::Credit { .. } => Refusal::Failed,
        reader::Ended::Behind | reader::Ended::Replaced => {
            panic!("invariant: a read does not end as {ended:?}")
        }
    }
}

/// The refusal of a read whose remote home refused it with `refusal`: the same code,
/// but `FAILED` for `MALFORMED`, as the program's open was well formed.
fn from_home(refusal: wire::hub::Refusal) -> Refusal {
    match refusal {
        wire::hub::Refusal::Malformed | wire::hub::Refusal::Failed => Refusal::Failed,
        wire::hub::Refusal::Unknown => Refusal::Unknown,
        wire::hub::Refusal::NotHome => Refusal::NotHome,
        wire::hub::Refusal::Busy => Refusal::Busy,
    }
}
