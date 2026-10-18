//! A node on the real OS whose log writer blocks, as on a reader of stderr that stops.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use crate::common::{PATIENCE, config, free, wait};
use crate::rig::Rig;

/// A writer whose first write blocks until `free` gets a message. It sends on
/// `entered` when it blocks, and keeps each line in `lines`.
struct Stuck {
    lines: Arc<Mutex<Vec<u8>>>,
    entered: Option<mpsc::Sender<()>>,
    free: mpsc::Receiver<()>,
}

impl io::Write for Stuck {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap_or_default();
            #[expect(clippy::disallowed_methods, reason = "a writer that blocks")]
            self.free.recv().unwrap_or_default();
        }
        let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// While the writer blocks at its first line, a shard serves and the node joins.
/// Once the writer frees, the log writes each line up to `stopped`.
#[test]
fn a_node_serves_and_joins_while_its_writer_blocks() {
    let rig = Rig::new();
    let (shards, threads) = (os::shards().unwrap(), os::threads().unwrap());
    let io = Rc::new(RefCell::new(Vec::new()));
    let lines = Arc::new(Mutex::new(Vec::new()));
    let (entered, blocked) = mpsc::channel();
    let (unblock, freed) = mpsc::channel();
    let writer = Stuck {
        lines: Arc::clone(&lines),
        entered: Some(entered),
        free: freed,
    };
    let log = node::Log {
        writer: Box::new(writer),
        format: ops::Log::default(),
    };
    let region = node::Region::Stored;
    let node = node::Node::start(node::Config {
        log,
        ..config(&rig, &shards, &threads, &io, free(), region, |disk| disk)
    });
    wait(&blocked);
    let (serves, serving) = mpsc::channel();
    node.spawn(move |_| {
        serves.send(()).unwrap();
        async {}
    });
    wait(&serving);
    node.stop();
    let flush = node.flush();
    assert_eq!(node.join(), Ok(()));
    unblock.send(()).unwrap();
    assert!(flush.wait(PATIENCE));
    for thread in io.take() {
        thread.join().unwrap();
    }
    let bytes = lines.lock().unwrap_or_else(PoisonError::into_inner);
    let text = std::str::from_utf8(&bytes).expect("UTF-8");
    let last = text.lines().last().expect("a line");
    assert!(last.contains(" INFO node: stopped"), "{text}");
    assert!(text.contains(" INFO node: started name=edge "), "{text}");
}
