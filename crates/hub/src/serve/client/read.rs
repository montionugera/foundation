//! The read stream of a client session.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::Poll;

use access::proof;
use transport::Class;
use transport::stream::{Receiver, Sender};
use types::name::{Name, Selector};
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
    let (open, mode) = open(session, class, subject, read, receiver).await?;
    let mut program = pin!(async {
        match receiver.recv().await? {
            None => Ok(()),
            Some(_) => Err(Error::Client(wire::hub::client::Error::Trailing)),
        }
    });
    let state = &session.state;
    let Some((reader, matched, layout)) =
        here(state, open, mode, program.as_mut()).await?
    else {
        sender.finish()?;
        return Ok(());
    };
    start(state, sender, &matched).await?;
    relay(state, reader, layout, program, sender).await
}

/// The verified open of a read, which holds its reservation until it drops.
struct Held {
    /// The selector of the first decode, until the read takes it.
    select: Option<Selector>,
    bytes: Vec<u8>,
    _reserved: Reserved,
}

impl Held {
    /// The selector of the open, decoded again after the first take.
    fn select(&mut self) -> Selector {
        self.select.take().unwrap_or_else(|| {
            Open::decode(&self.bytes)
                .expect("invariant: the open decoded once")
                .select
        })
    }
}

/// Reads, verifies, and decodes the open, and gives it with its mode.
async fn open(
    session: &Session,
    class: Class,
    subject: Name,
    read: Read,
    receiver: &mut Receiver,
) -> Result<(Held, reader::Mode), Error> {
    // A read takes no turn.
    let reserved = Reserved::new(&session.state, subject, read.length)?;
    let bytes = body(receiver, read.body()).await?;
    verify(session, proof::Kind::Read, &bytes, &read.signature)?;
    let open = Open::decode(&bytes)?;
    let (wanted, mode) = match open.mode {
        Mode::Latest => (Class::Latest, reader::Mode::Latest),
        Mode::Complete => (Class::Complete, reader::Mode::Complete),
    };
    if class != wanted {
        return Err(Error::Class(class));
    }
    let held = Held {
        select: Some(open.select),
        bytes,
        _reserved: reserved,
    };
    Ok((held, mode))
}

/// Opens the reader of `open` at this node, the home of its index, and gives it with
/// its places and their layout, or `None` when `program` finishes first. Holds the
/// reservation of `open` until the open decodes after the wait for the home.
async fn here<F>(
    state: &Rc<RefCell<State>>,
    mut open: Held,
    mode: reader::Mode,
    mut program: Pin<&mut F>,
) -> Result<Option<(Reader, Matched, Layout)>, Error>
where
    F: Future<Output = Result<(), Error>>,
{
    let refused = |error| Error::Reader(Box::new(error));
    let found = {
        let mut matched = pin!(reader::matched(state, || open.select()));
        poll_fn(|cx| match program.as_mut().poll(cx) {
            Poll::Ready(finished) => Poll::Ready(Err(finished)),
            Poll::Pending => matched.as_mut().poll(cx).map(Ok),
        })
        .await
    };
    drop(open);
    let (matched, home) = match found {
        Ok(found) => found.map_err(refused)?,
        Err(finished) => return finished.map(|()| None),
    };
    let channels = match home {
        Home::Here(channels) => channels,
        Home::Away(home) => return Err(refused(reader::Error::Remote { home })),
    };
    let layout = Layout::new(channels.slots.clone(), channels.index);
    let reader = Reader::here(state, &matched, channels, mode).map_err(refused)?;
    Ok(Some((reader, matched, layout)))
}

/// Sends the places of `matched`, their run, and `Opened`.
async fn start(
    state: &RefCell<State>,
    sender: &mut Sender,
    matched: &Matched,
) -> Result<(), Error> {
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
    Ok(())
}

/// Sends each frame of `reader` until `program` finishes, or `Behind` and the finish
/// when the reader missed a frame.
async fn relay<F>(
    state: &RefCell<State>,
    mut reader: Reader,
    mut layout: Layout,
    mut program: Pin<&mut F>,
    sender: &mut Sender,
) -> Result<(), Error>
where
    F: Future<Output = Result<(), Error>>,
{
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
