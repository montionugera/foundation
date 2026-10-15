//! The peer end of a HUB WIRE session, for the tests and benches that read through
//! one. The frames of a client read come in the same messages.

use block::Pool;
use transport::Class;
use transport::stream::{Receiver, Sender};
use types::channel;
use wire::Protocol;
use wire::hub::{FromHome, Mode, Open, Reader, keys};

/// Opens a stream of the class of `mode` on `session`, sends the header and the open of
/// a session of `mode` on `keys`, and reads the reply. Gives the stream and the decoder
/// of its frames.
pub(crate) async fn open(
    pool: &Pool,
    session: &transport::Session,
    mode: Mode,
    keys: &[u128],
) -> (Sender, Receiver, Reader) {
    let class = match mode {
        Mode::Latest => Class::Latest,
        Mode::Complete { .. } => Class::Complete,
    };
    let (mut sender, mut receiver) = session.open(class).await.expect("opens");
    send(pool, &mut sender, &wire::header::encode(Protocol::Hub)).await;
    let keys: Vec<_> = keys.iter().copied().map(channel::Key::from_u128).collect();
    let open = Open {
        mode,
        channels: u32::try_from(keys.len()).expect("the keys fit a u32"),
    };
    let mut out = vec![0; open.encoded_len()];
    open.encode(&mut out);
    send(pool, &mut sender, &out).await;
    let mut out = vec![0; keys.len() * keys::LEN];
    keys::encode(&keys, &mut out);
    send(pool, &mut sender, &out).await;
    let mut reader = Reader::new(&open);
    let opened = receiver.recv().await.expect("receives").expect("a reply");
    assert!(
        matches!(reader.decode(&opened), Ok(FromHome::Opened)),
        "the home opens the session"
    );
    (sender, receiver, reader)
}

/// Reads the messages of one whole frame from `receiver` with `reader`, and gives how
/// many it read.
pub(crate) async fn frame(receiver: &mut Receiver, reader: &mut Reader) -> usize {
    let mut messages = 0;
    loop {
        let message = receiver.recv().await.expect("receives").expect("a message");
        let decoded = reader.decode(&message).expect("a valid message");
        messages += 1;
        let ended = matches!(
            decoded,
            FromHome::Body { .. } | FromHome::Ends { last: true, .. }
        );
        if ended && reader.body().is_none() {
            return messages;
        }
    }
}

/// Sends `bytes` in one message.
async fn send(pool: &Pool, sender: &mut Sender, bytes: &[u8]) {
    let mut block = pool.alloc(bytes.len()).expect("the pool has room");
    block.copy_from_slice(bytes);
    sender.send(block.freeze()).await.expect("sends");
}
