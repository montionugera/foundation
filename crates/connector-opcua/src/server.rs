//! The test server of open62541, on the loop of a connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::ffi::CStr;
use std::future::poll_fn;
use std::mem::ManuallyDrop;
use std::panic::{self, AssertUnwindSafe};
use std::pin::pin;
use std::ptr::NonNull;
use std::task::Poll;

use crate::connection::Manager;
use crate::ffi::{self, Status};
use crate::node::{Key, Node};
use crate::value::Value;
#[cfg(test)]
use types::time::Span;

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
    read: unsafe extern "C" fn(*mut Source, *mut ffi::Scalar, *mut u16, *mut u32),
    answer: Box<dyn FnMut() -> (Value, types::quality::Quality)>,
    /// False while the add reads the variable to check its type.
    added: bool,
}

/// Answers a read of the variable of `source` into `value`, `kind`, and `status`.
///
/// # Safety
///
/// `source` is live, nothing else holds it, and `value`, `kind`, and `status` are
/// places to write.
#[cfg(feature = "sim")]
unsafe extern "C" fn read(
    source: *mut Source,
    value: *mut ffi::Scalar,
    kind: *mut u16,
    status: *mut u32,
) {
    // SAFETY: as the caller ensures.
    let source = unsafe { &mut *source };
    let (answer, quality) = if source.added {
        (source.answer)()
    } else {
        (Value::F64(0.0), types::quality::Quality::GOOD)
    };
    let (scalar, scalar_kind) = answer.raw();
    // SAFETY: as the caller ensures.
    unsafe { value.write(scalar) };
    // SAFETY: as the caller ensures.
    unsafe { kind.write(scalar_kind) };
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

    /// Adds the variable `node` with `value`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    pub(crate) fn add_scalar(&self, node: &Node, value: Value) {
        let (scalar, kind) = value.raw();
        // SAFETY: the server lives, and it copies the name of `node` and `scalar`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_scalar(
                self.raw(),
                added(node),
                &raw const scalar,
                kind,
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the variable `node` with an array of two Doubles.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_doubles(&self, node: &Node) {
        // SAFETY: the server lives, and it copies the name of `node`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_doubles(self.raw(), added(node))
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the scalar variable `node` of the `DataType` `BaseDataType`, and answers
    /// each read of a client with the value and the status that `answer` gives, on the
    /// thread that drives the server.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(feature = "sim")]
    pub(crate) fn add_source(
        &mut self,
        node: &Node,
        answer: impl FnMut() -> (Value, types::quality::Quality) + 'static,
    ) {
        let source = self.source(Box::new(answer), false);
        // SAFETY: the server lives, it copies the name of `node`, and the close frees
        // `source` after the delete.
        let status = Status(unsafe {
            ffi::test::shim_server_add_source(
                self.raw(),
                added(node),
                source.as_ptr().cast(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
        // SAFETY: it lives, and the server reads it only while the loop runs.
        unsafe { (*source.as_ptr()).added = true };
    }

    /// Makes `answer` answer each later read of the variable `node`, which open62541
    /// adds, such as the state of the server, as [`Server::add_source`] does.
    ///
    /// # Panics
    ///
    /// If open62541 has no variable `node`, or its key is the number 0.
    #[cfg(test)]
    pub(crate) fn set_source(
        &mut self,
        node: &Node,
        answer: impl FnMut() -> (Value, types::quality::Quality) + 'static,
    ) {
        let source = self.source(Box::new(answer), true);
        // SAFETY: the server lives, and the close frees `source` after the delete.
        let status = Status(unsafe {
            ffi::test::shim_server_set_source(
                self.raw(),
                added(node),
                source.as_ptr().cast(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the source");
    }

    /// Makes a [`Source`] of `answer` that the close frees.
    #[cfg(feature = "sim")]
    fn source(
        &mut self,
        answer: Box<dyn FnMut() -> (Value, types::quality::Quality)>,
        added: bool,
    ) -> NonNull<Source> {
        let source = NonNull::from(Box::leak(Box::new(Source {
            read,
            answer,
            added,
        })));
        self.sources.push(source);
        source
    }

    /// Sets the most session timeout that the server gives a later session, in whole
    /// ms.
    ///
    /// # Panics
    ///
    /// If `most` is past `u32::MAX` ms.
    #[cfg(test)]
    pub(crate) fn set_session_timeout(&self, most: Span) {
        let ms = u32::try_from(most.nanos() / 1_000_000).expect("a timeout of u32 ms");
        // SAFETY: the server lives.
        unsafe { ffi::test::shim_server_session_timeout(self.raw(), ms) };
    }

    /// The number of the `DataType` of the variable `node`, in namespace 0.
    ///
    /// # Panics
    ///
    /// If the server has no variable `node`, or its `DataType` is not a number of
    /// namespace 0.
    #[cfg(test)]
    pub(crate) fn data_type(&self, node: &Node) -> u32 {
        let mut id = 0;
        // SAFETY: the server lives, and `id` is a place to write.
        let status = Status(unsafe {
            ffi::test::shim_server_data_type(self.raw(), node.into(), &raw mut id)
        });
        assert_eq!(status, Status::GOOD, "{node}: open62541 gives no DataType");
        id
    }

    /// Adds the variable `node` with the String `text`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_string(&self, node: &Node) {
        // SAFETY: the server lives, and it copies the name of `node`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_string(self.raw(), added(node))
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the variable `node` with a `ByteString` of `length` zero bytes.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_bytes(&self, node: &Node, size: usize) {
        // SAFETY: the server lives, and it copies the name of `node`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_bytes(self.raw(), added(node), size)
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Refuses each Read of more than `most` nodes, 0 for none, with
    /// `BadTooManyOperations`. It adds no variable `ns=0;i=11705`, so a client does
    /// not know the limit.
    pub(crate) fn refuse_reads_past(&self, most: u32) {
        // SAFETY: the server lives.
        unsafe { ffi::test::shim_server_reads(self.raw(), most) };
    }

    /// Sets the limits of each later channel of the server.
    #[cfg(test)]
    pub(crate) fn set_limits(&self, limits: Limits) {
        // SAFETY: the server lives.
        unsafe {
            ffi::test::shim_server_limits(
                self.raw(),
                limits.buffer,
                limits.message,
                limits.chunks,
            );
        };
    }

    /// Sets the `MaxNodesPerRead` of the server to `most`, 0 for none, and adds the
    /// variable `ns=0;i=11705` that gives it, which the build of open62541 lacks.
    ///
    /// # Panics
    ///
    /// On a second call, as open62541 refuses the second add of the variable.
    pub(crate) fn set_per_read(&self, most: u32) {
        self.refuse_reads_past(most);
        let node = Node {
            namespace: 0,
            key: Key::Number(ffi::MAX_NODES_PER_READ),
        };
        self.add_scalar(&node, Value::U32(most));
    }

    /// Sets the `MaxNodesPerBrowse` of the server to `browse` and its
    /// `MaxNodesPerTranslateBrowsePathsToNodeIds` to `translate`, each 0 for none, and
    /// adds the variables `ns=0;i=11710` and `ns=0;i=11712` that give them.
    ///
    /// # Panics
    ///
    /// On a second call, as open62541 refuses the second add of each variable.
    #[cfg(test)]
    pub(crate) fn set_per_browse(&self, browse: u32, translate: u32) {
        // SAFETY: the server lives.
        unsafe { ffi::test::shim_server_browses(self.raw(), browse, translate) };
        for (number, most) in [(11710, browse), (11712, translate)] {
            let node = Node {
                namespace: 0,
                key: Key::Number(number),
            };
            self.add_scalar(&node, Value::U32(most));
        }
    }

    /// Counts each Browse request that the server processes from now.
    ///
    /// # Panics
    ///
    /// If open62541 cannot allocate the count.
    #[cfg(test)]
    pub(crate) fn count_browses(&self) -> Browses<'_> {
        // SAFETY: the server lives.
        let count = unsafe { ffi::test::shim_server_count_browses(self.raw()) };
        Browses {
            count: NonNull::new(count.cast_mut())
                .expect("open62541 allocates the count"),
            server: std::marker::PhantomData,
        }
    }

    /// Adds the folder `node` with the browse name `name` and the display name
    /// `display`, under `parent` by the reference type of the number `reference` in
    /// namespace 0.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_folder(
        &self,
        node: &Node,
        parent: &Node,
        reference: u32,
        name: &CStr,
        display: &CStr,
    ) {
        // SAFETY: the server lives, and it copies each name.
        let status = Status(unsafe {
            ffi::test::shim_server_add_folder(
                self.raw(),
                added(node),
                parent.into(),
                reference,
                name.as_ptr(),
                display.as_ptr(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the folder of a GUID in namespace 1 with the browse and display name
    /// `guid`, under `parent` by `Organizes`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node.
    #[cfg(test)]
    pub(crate) fn add_guid(&self, parent: &Node) {
        // SAFETY: the server lives, and it copies the name of `parent`.
        let status = Status(unsafe {
            ffi::test::shim_server_add_guid(self.raw(), parent.into())
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the variable `node` with no value, the browse and display name `name`,
    /// the `DataType` `data_type`, the `ValueRank` `rank`, and the `AccessLevel` and
    /// `UserAccessLevel` `access`, under `parent` by the reference type of the number
    /// `reference` in namespace 0.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "each is an attribute of the node"
    )]
    pub(crate) fn add_variable(
        &self,
        node: &Node,
        parent: &Node,
        reference: u32,
        name: &CStr,
        data_type: &Node,
        rank: i32,
        access: u8,
    ) {
        // SAFETY: the server lives, and it copies each name.
        let status = Status(unsafe {
            ffi::test::shim_server_add_variable(
                self.raw(),
                added(node),
                parent.into(),
                reference,
                name.as_ptr(),
                data_type.into(),
                rank,
                access,
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the data type `node` with the browse name `name`, a subtype of `parent`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_data_type(&self, node: &Node, parent: &Node, name: &CStr) {
        // SAFETY: the server lives, and it copies each name.
        let status = Status(unsafe {
            ffi::test::shim_server_add_data_type(
                self.raw(),
                added(node),
                parent.into(),
                name.as_ptr(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds the property `0:EngineeringUnits` `node` of `variable`, an
    /// `EUInformation` of the `namespaceUri` `uri`, the display name `text`, and the
    /// `unitId` `code`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_unit(
        &self,
        node: &Node,
        variable: &Node,
        uri: &CStr,
        text: &CStr,
        code: i32,
    ) {
        // SAFETY: the server lives, and it copies each name and text.
        let status = Status(unsafe {
            ffi::test::shim_server_add_unit(
                self.raw(),
                added(node),
                variable.into(),
                uri.as_ptr(),
                text.as_ptr(),
                code,
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Adds a reference of the type of the number `reference` in namespace 0 from
    /// `source` to `target`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the reference.
    #[cfg(test)]
    pub(crate) fn add_reference(&self, source: &Node, reference: u32, target: &Node) {
        // SAFETY: the server lives, and it copies each name.
        let status = Status(unsafe {
            ffi::test::shim_server_add_reference(
                self.raw(),
                source.into(),
                reference,
                target.into(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the reference");
    }

    /// Adds the view `node` with the browse and display name `name`, under `parent` by
    /// `Organizes`.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the node, or the key of `node` is the number 0.
    #[cfg(test)]
    pub(crate) fn add_view(&self, node: &Node, parent: &Node, name: &CStr) {
        // SAFETY: the server lives, and it copies each name.
        let status = Status(unsafe {
            ffi::test::shim_server_add_view(
                self.raw(),
                added(node),
                parent.into(),
                name.as_ptr(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the node");
    }

    /// Deletes the reference of the type of the number `reference` in namespace 0
    /// from `source` to `target`, in both directions.
    ///
    /// # Panics
    ///
    /// If the server has no such reference.
    #[cfg(test)]
    pub(crate) fn delete_reference(
        &self,
        source: &Node,
        reference: u32,
        target: &Node,
    ) {
        // SAFETY: the server lives, and it copies each name.
        let status = Status(unsafe {
            ffi::test::shim_server_delete_reference(
                self.raw(),
                source.into(),
                reference,
                target.into(),
            )
        });
        assert_eq!(status, Status::GOOD, "open62541 refused the delete");
    }

    /// Gives the server, for a call that reads or changes it.
    pub(crate) fn raw(&self) -> *mut ffi::test::Server {
        self.raw.as_ptr()
    }

    /// Starts a server on `manager` with an empty host and room for `sessions`
    /// sessions, as [`Server::start`] does, and gives it to `add`. Then drives
    /// `manager` until `until` completes, closes the server as [`Server::close`] does,
    /// drops `manager`, and gives the output of `until`.
    ///
    /// # Panics
    ///
    /// As [`Server::start`] and [`Server::close`] do. A panic of `add` closes the
    /// server and drops `manager` before it goes on. Another panic leaks the server and
    /// `manager`.
    pub(crate) async fn serve<T>(
        manager: Manager,
        sessions: u16,
        add: impl FnOnce(&mut Self),
        until: impl Future<Output = T>,
    ) -> T {
        let manager = ManuallyDrop::new(manager);
        // SAFETY: each close below deletes the server before the manager drops. A
        // panic out of `start` leaks the manager, and the server if open62541 made
        // it. A panic out of the drive, `until`, or a close, and a drop of this future
        // while it waits, leak both.
        let mut server = unsafe { Self::start(&manager, c"", sessions) };
        if let Err(panic) = panic::catch_unwind(AssertUnwindSafe(|| add(&mut server))) {
            server.close(&manager).await;
            drop(ManuallyDrop::into_inner(manager));
            panic::resume_unwind(panic);
        }
        let out = {
            let mut drive = pin!(manager.drive(|_| Poll::<()>::Pending));
            let mut until = pin!(until);
            poll_fn(|cx| {
                assert!(drive.as_mut().poll(cx).is_pending(), "a drive never ends");
                until.as_mut().poll(cx)
            })
            .await
        };
        server.close(&manager).await;
        drop(ManuallyDrop::into_inner(manager));
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

/// The limits of the channels of a [`Server`], each 0 for the default of open62541:
/// 65536 bytes, 512 MiB, and 16384 chunks.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Limits {
    /// The send and receive buffer size.
    pub(crate) buffer: u32,
    /// The `MaxMessageSize`.
    pub(crate) message: u32,
    /// The `MaxChunkCount`.
    pub(crate) chunks: u32,
}

/// The count of the Browse requests of a [`Server`], from [`Server::count_browses`].
#[cfg(test)]
pub(crate) struct Browses<'s> {
    /// Lives until the delete of the server.
    count: NonNull<usize>,
    server: std::marker::PhantomData<&'s Server>,
}

#[cfg(test)]
impl Browses<'_> {
    /// Gives the count.
    pub(crate) fn get(&self) -> usize {
        // SAFETY: the server lives, and runs on this thread.
        unsafe { self.count.read() }
    }
}

/// `node` for an add.
///
/// # Panics
///
/// If its key is the number 0, since open62541 then adds the node at a free number.
fn added(node: &Node) -> ffi::Node<'_> {
    assert!(
        node.key != Key::Number(0),
        "{node}: open62541 adds a node of the number 0 at a free number"
    );
    node.into()
}

#[cfg(test)]
mod tests;
