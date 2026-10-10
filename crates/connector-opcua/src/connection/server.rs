//! The stop and delete of a server on the loop of a manager.

use std::task::Poll;

use super::Manager;
use crate::ffi::{self, Status};

impl Manager {
    /// Stops `server`, drives the manager until no delayed callback waits on the
    /// loop, then deletes `server`. It does not wait for a timer.
    ///
    /// # Safety
    ///
    /// `server` lives on the loop of the manager, and nothing uses it after the call
    /// returns.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the stop or the delete, or fails a run of the loop. If a
    /// delayed callback still waits after 64 runs of the loop: a loop whose delayed
    /// callbacks do not drain is a defect. Each panic but the refused delete comes
    /// before the delete, so `server` still lives after it.
    pub(crate) async unsafe fn close_server(&self, server: *mut ffi::test::Server) {
        // SAFETY: the server lives.
        let status = Status(unsafe { ffi::test::UA_Server_run_shutdown(server) });
        assert_eq!(status, Status::GOOD, "open62541 refused the server stop");
        let events = &self.events;
        // SAFETY: a constant of `shim.c`.
        let passes = unsafe { ffi::shim_free_passes };
        let mut runs = 0;
        self.drive(|_| {
            events.run();
            runs += 1;
            if !events.delayed() {
                return Poll::Ready(());
            }
            assert!(
                runs < passes,
                "open62541 queued delayed callbacks for {passes} runs of a server close"
            );
            Poll::Pending
        })
        .await;
        // SAFETY: the server lives.
        let state = unsafe { ffi::test::UA_Server_getLifecycleState(server) };
        assert_eq!(
            state,
            ffi::test::Lifecycle::STOPPED,
            "invariant: a close queues its `CLOSING` at once"
        );
        // SAFETY: the server is stopped, and nothing holds it.
        let status = Status(unsafe { ffi::test::UA_Server_delete(server) });
        assert_eq!(status, Status::GOOD, "open62541 refused the server delete");
    }
}
