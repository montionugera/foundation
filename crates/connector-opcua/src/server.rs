//! The test server of open62541, on the loop of a connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::ffi::CString;
use std::ptr::NonNull;
use std::task::Poll;

use crate::connection::Manager;
use crate::ffi::{self, Status};

/// A started test server of open62541 on the loop of a connection manager.
/// [`Server::close`] deletes it, and a drop leaks it.
pub(crate) struct Server {
    raw: NonNull<ffi::test::Server>,
    /// The loop that the server lives on.
    events: *mut ffi::EventLoop,
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
    /// If `manager` has no listener that a listen open did not take, `host` holds a
    /// NUL, or open62541 refuses the server or its start.
    pub(crate) unsafe fn start(manager: &Manager, host: &str, sessions: u16) -> Self {
        let port = manager
            .port()
            .expect("a server starts on a manager with a listener that no server took");
        let host = CString::new(host).expect("no NUL");
        let events = manager.events().raw();
        // SAFETY: the loop outlives the server, as the caller ensures.
        let raw = unsafe {
            ffi::test::shim_server_new(events, port, host.as_ptr(), sessions)
        };
        let raw = NonNull::new(raw).expect("open62541 refused the server");
        // SAFETY: the server lives.
        let status = Status(unsafe { ffi::test::UA_Server_run_startup(raw.as_ptr()) });
        assert_eq!(status, Status::GOOD, "open62541 refused the server start");
        Self { raw, events }
    }

    /// Gives the server, for a call that reads or changes it.
    pub(crate) fn raw(&self) -> *mut ffi::test::Server {
        self.raw.as_ptr()
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
    }
}
