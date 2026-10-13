//! The test server of open62541, on the loop of a connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::ffi::CStr;
use std::future::poll_fn;
use std::pin::pin;
use std::ptr::NonNull;
use std::task::Poll;

use crate::connection::Manager;
use crate::ffi::{self, Status};
use crate::node::{Key, Node};

/// A started test server of open62541 on the loop of a connection manager.
/// [`Server::close`] deletes it, and a drop leaks it.
pub(crate) struct Server {
    raw: NonNull<ffi::test::Server>,
    /// The loop that the server lives on.
    events: *mut ffi::EventLoop,
    /// The context of each variable of [`Server::add_source`], until the delete.
    #[cfg(feature = "sim")]
    sources: Vec<NonNull<Source>>,
}

/// The node context of a variable of [`Server::add_source`]: `shim_source` of
/// `server.c`, then the read.
#[cfg(feature = "sim")]
#[repr(C)]
struct Source {
    read: unsafe extern "C" fn(*mut Source, *mut f64, *mut u32),
    answer: Box<dyn FnMut() -> (f64, types::quality::Quality)>,
    /// False while the add reads the variable to check its type.
    added: bool,
}

/// Answers a read of the variable of `source` into `value` and `status`.
///
/// # Safety
///
/// `source` is live, nothing else holds it, and `value` and `status` are places to
/// write.
#[cfg(feature = "sim")]
unsafe extern "C" fn read(source: *mut Source, value: *mut f64, status: *mut u32) {
    // SAFETY: as the caller ensures.
    let source = unsafe { &mut *source };
    let (double, quality) = if source.added {
        (source.answer)()
    } else {
        (0.0, types::quality::Quality::GOOD)
    };
    // SAFETY: as the caller ensures.
    unsafe { value.write(double) };
    // SAFETY: as the caller ensures.
    unsafe { status.write(quality.0) };
}

impl Server {
    /// Makes the test server on the loop of `manager`, with the minimal config, the one
    /// server URL `opc.tcp://{host}:{port}` on the port of the listener of `manager`,
    /// and room for `sessions` sessions and secure channels, and starts it. With an
    /// empty `host`, the discovery URL of the server has the address of the listener as
    /// its host, and a listener on an unspecified address gives the server no discovery
    /// URL.
    ///
    /// # Safety
    ///
    /// The server is deleted before `manager` drops: by [`Server::close`], or by a
    /// delete through [`Server::raw`].
    ///
    /// # Panics
    ///
    /// If `manager` has no listener that a listen open did not take, or open62541
    /// refuses the server or its start.
    pub(crate) unsafe fn start(manager: &Manager, host: &CStr, sessions: u16) -> Self {
        let port = manager
            .port()
            .expect("a server starts on a manager with a listener that no server took");
        let events = manager.events().raw();
        // SAFETY: the loop outlives the server, as the caller ensures.
        let raw = unsafe {
            ffi::test::shim_server_new(
                events,
                port,
                host.as_ptr(),
                sessions,
                crate::connection::CHUNKS,
            )
        };
        let raw = NonNull::new(raw).expect("open62541 refused the server");
        // SAFETY: the server lives.
        let status = Status(unsafe { ffi::test::UA_Server_run_startup(raw.as_ptr()) });
        assert_eq!(status, Status::GOOD, "open62541 refused the server start");
        Self {
            raw,
            events,
            #[cfg(feature = "sim")]
            sources: Vec::new(),
        }
    }

    /// Adds the namespace `uri`, and gives its index.
    #[cfg(any(test, feature = "sim"))]
    pub(crate) fn add_namespace(&self, uri: &CStr) -> u16 {
        // SAFETY: the server lives, and it copies the name.
        unsafe { ffi::test::UA_Server_addNamespace(self.raw(), uri.as_ptr()) }
    }

    /// Adds the variable `node` with the Double `value`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node.
    pub(crate) fn add_double(&self, node: &Node, value: f64) {
        let (namespace, number, name, length) = key(node);
        // SAFETY: the server lives, and it copies the name of `node`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_double(
                self.raw(),
                namespace,
                number,
                name,
                length,
                value,
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the Double variable `node`, and answers each read of a client with the
    /// value and the status that `answer` gives, on the thread that drives the server.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node.
    #[cfg(feature = "sim")]
    pub(crate) fn add_source(
        &mut self,
        node: &Node,
        answer: impl FnMut() -> (f64, types::quality::Quality) + 'static,
    ) {
        let (namespace, number, name, length) = key(node);
        let source = NonNull::from(Box::leak(Box::new(Source {
            read,
            answer: Box::new(answer),
            added: false,
        })));
        self.sources.push(source);
        // SAFETY: the server lives, it copies the name of `node`, and the close frees
        // `source` after the delete.
        let status = Status(unsafe {
            ffi::test::shim_server_add_source(
                self.raw(),
                namespace,
                number,
                name,
                length,
                source.as_ptr().cast(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
        // SAFETY: it lives, and the server reads it only while the loop runs.
        unsafe { (*source.as_ptr()).added = true };
    }

    /// Adds the variable `node` with a `ByteString` of `length` zero bytes.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node.
    #[cfg(test)]
    pub(crate) fn add_bytes(&self, node: &Node, size: usize) {
        let (namespace, number, name, length) = key(node);
        // SAFETY: the server lives, and it copies the name of `node`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_bytes(
                self.raw(),
                namespace,
                number,
                name,
                length,
                size,
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Sets the most chunks in a message of each later channel of the server, with 0 as
    /// the default of open62541, 16384.
    #[cfg(test)]
    pub(crate) fn set_chunks(&self, chunks: u32) {
        // SAFETY: the server lives.
        unsafe { ffi::test::shim_server_chunks(self.raw(), chunks) };
    }

    /// Gives the server, for a call that reads or changes it.
    pub(crate) fn raw(&self) -> *mut ffi::test::Server {
        self.raw.as_ptr()
    }

    /// Drives `manager` until `until` completes, then closes the server as
    /// [`Server::close`] does, and gives the output of `until`.
    pub(crate) async fn serve<T>(
        self,
        manager: &Manager,
        until: impl Future<Output = T>,
    ) -> T {
        let out = {
            let mut drive = pin!(manager.drive(|_| {
                manager.events().run();
                Poll::<()>::Pending
            }));
            let mut until = pin!(until);
            poll_fn(|cx| {
                assert!(drive.as_mut().poll(cx).is_pending(), "a drive never ends");
                until.as_mut().poll(cx)
            })
            .await
        };
        self.close(manager).await;
        out
    }

    /// Stops the server, drives `manager` until no delayed callback waits on its loop,
    /// then deletes the server. It does not wait for a timer.
    ///
    /// # Panics
    ///
    /// If `manager` does not hold the loop of the server. If open62541 refuses the stop
    /// or the delete, or fails a run of the loop. If a delayed callback still waits
    /// after 64 runs of the loop: a loop whose delayed callbacks do not drain is a
    /// defect. Each panic but the refused delete comes before the delete, so the
    /// server still lives after it.
    pub(crate) async fn close(self, manager: &Manager) {
        let events = manager.events();
        assert!(
            events.raw() == self.events,
            "a server closes through the manager it started on"
        );
        // SAFETY: the server lives.
        let status = Status(unsafe { ffi::test::UA_Server_run_shutdown(self.raw()) });
        assert_eq!(status, Status::GOOD, "open62541 refused the server stop");
        // SAFETY: a constant of `shim.c`.
        let passes = unsafe { ffi::shim_free_passes };
        let mut runs = 0;
        manager
            .drive(|_| {
                events.run();
                runs += 1;
                if !events.delayed() {
                    return Poll::Ready(());
                }
                assert!(
                    runs < passes,
                    "open62541 queued delayed callbacks for {passes} runs of a server \
                     close"
                );
                Poll::Pending
            })
            .await;
        // SAFETY: the server lives.
        let state = unsafe { ffi::test::UA_Server_getLifecycleState(self.raw()) };
        assert_eq!(
            state,
            ffi::test::Lifecycle::STOPPED,
            "invariant: a close queues its `CLOSING` at once"
        );
        // SAFETY: the server is stopped, and nothing holds it.
        let status = Status(unsafe { ffi::test::UA_Server_delete(self.raw()) });
        assert_eq!(status, Status::GOOD, "open62541 refused the server delete");
        #[cfg(feature = "sim")]
        for source in self.sources {
            // SAFETY: the server that read it is deleted, and nothing else holds it.
            drop(unsafe { Box::from_raw(source.as_ptr()) });
        }
    }
}

/// The parts of `node` for an add.
///
/// # Panics
///
/// If its key is the number 0, since open62541 then adds the node at a free number.
fn key(node: &Node) -> (u16, u32, *const u8, usize) {
    assert!(
        node.key != Key::Number(0),
        "{node}: open62541 adds a node of the number 0 at a free number"
    );
    ffi::node(node)
}
