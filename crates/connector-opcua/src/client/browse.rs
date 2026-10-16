//! The browse of a server: the references of a node, and what a server says of each
//! variable.

use std::ffi::c_void;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::ptr::{self, NonNull};
use std::task::Poll;

use types::hash::{Map, Set};

use super::{Client, READS, Reads, limit};
use crate::alloc;
use crate::ffi::{self, Service, Status};
use crate::node::{Key, Node};
use crate::value::Value;

/// `HierarchicalReferences`.
const HIERARCHICAL: u32 = 33;

/// `HasSubtype`.
const HAS_SUBTYPE: u32 = 45;

/// The mask of the node classes Object (1), Variable (2), and View (128).
const CLASSES: u32 = 0b1000_0011;

/// The mask of the node class `DataType`.
const DATA_TYPE: u32 = 64;

/// The attributes `DataType`, `ValueRank`, and `UserAccessLevel`.
const FACTS: [u32; 3] = [14, 15, 18];

/// The attribute `Value`.
const VALUE: [u32; 1] = [13];

/// The variables `MaxNodesPerBrowse` and `MaxNodesPerTranslateBrowsePathsToNodeIds`.
const LIMITS: [u32; 2] = [11710, 11712];

/// The `namespaceUri` of an `EUInformation` whose `unitId` is a code of UN/CEFACT
/// Recommendation 20.
const CEFACT: &[u8] = b"http://www.opcfoundation.org/UA/units/un/cefact";

/// The most steps of a walk up the `HasSubtype` references.
const STEPS: usize = 16;

/// The id of `Enumeration`.
const ENUMERATION: u32 = 29;

/// The id of `Int32`.
const INT32: u32 = 6;

/// One page of the references of a node.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Page {
    /// The references, in server order.
    pub(crate) references: Vec<Reference>,
    /// Where the next page starts, or `None` after the last.
    pub(crate) next: Option<Point>,
}

/// A continuation point of a browse, which lives in the session that gave it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Point(Vec<u8>);

/// A reference to a node.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Reference {
    /// The node, or `None` when its id is not a number or a name of this server (a
    /// GUID, an opaque key, or a node of another server).
    pub(crate) node: Option<Node>,
    /// The text of its browse name.
    pub(crate) name: String,
    /// The text of its display name.
    pub(crate) display: String,
    pub(crate) class: Class,
    /// Its reference type, such as `Organizes` (`i=35`), or `None` when the id of the
    /// type is not a number or a name.
    pub(crate) kind: Option<Node>,
}

/// The node class of a [`Reference`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Object,
    Variable,
    View,
}

/// What a server says of a variable.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Description {
    /// The id in namespace 0 of the built-in type of its data type, such as 11 for
    /// Double, as [`base`] gives it. `None` also when a type on the walk has an id
    /// that is not a number or a name.
    pub(crate) base: Option<u32>,
    /// Its `ValueRank`: -1 for a scalar.
    pub(crate) rank: Option<i32>,
    /// Its `UserAccessLevel`.
    pub(crate) access: Option<u8>,
    /// Its `EngineeringUnits`.
    pub(crate) unit: Option<Unit>,
}

/// The `EUInformation` of a variable.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Unit {
    /// The text of its display name, or `None` when empty.
    pub(crate) text: Option<String>,
    /// Its UN/CEFACT Recommendation 20 code from `unitId`, such as `CEL`, as
    /// [`code_of`] gives it, when its `namespaceUri` is that of Recommendation 20,
    /// else `None`.
    pub(crate) code: Option<String>,
}

/// The limits of a server on the nodes of one request of a service, other than Read.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    /// Its `MaxNodesPerBrowse`.
    browse: Option<NonZeroUsize>,
    /// Its `MaxNodesPerTranslateBrowsePathsToNodeIds`.
    translate: Option<NonZeroUsize>,
}

impl Client {
    /// Browses the forward hierarchical references of `node` and their subtypes, to
    /// the node classes Object, Variable, and View, and gives the first page of at
    /// most `most` references, in server order, 0 for no limit. `None` when the server
    /// does not have `node`.
    ///
    /// # Errors
    ///
    /// `BadTimeout` when the answer does not come in the request timeout of the
    /// client (5 s), `BadTooManyOperations` when the client cannot send in that time,
    /// as [`Client::read`] says, else the status of open62541 when it refuses the
    /// request, the service result, or the status of the result of `node` when it is
    /// not Good and not `BadNodeIdUnknown`, such as `BadNoContinuationPoints`.
    pub(crate) async fn browse(
        &mut self,
        node: &Node,
        most: u32,
    ) -> Result<Option<Page>, Status> {
        let raw = [ffi::Node::from(node)];
        // SAFETY: `raw` holds one node, and C copies its name.
        let request = Request::new(
            unsafe {
                ffi::shim_browse_new(
                    raw.as_ptr(),
                    1,
                    HIERARCHICAL,
                    false,
                    CLASSES,
                    most,
                )
            },
            Service::BROWSE,
        );
        let answer = self.serve(&request).await?;
        match answer.page() {
            Err(Status::BAD_NODE_ID_UNKNOWN) => Ok(None),
            page => page.map(Some),
        }
    }

    /// Gives the page after the page of `point`, a point of this client's session.
    ///
    /// # Errors
    ///
    /// As [`Client::browse`], and `BadContinuationPointInvalid` when the server no
    /// longer holds `point`.
    pub(crate) async fn browse_next(&mut self, point: Point) -> Result<Page, Status> {
        // SAFETY: `point` holds its bytes, and C copies them.
        let request = Request::new(
            unsafe { ffi::shim_browse_next_new(point.0.as_ptr(), point.0.len()) },
            Service::BROWSE_NEXT,
        );
        self.serve(&request).await?.page()
    }

    /// Gives the description of each variable of `nodes`, in order. A fact that the
    /// server does not give, or gives with a status that is not Good, is `None`. It
    /// splits each request by the limit of the server for its service
    /// (`MaxNodesPerRead`, `MaxNodesPerBrowse`, or
    /// `MaxNodesPerTranslateBrowsePathsToNodeIds`) and by the size of a message that
    /// the client sends, and sends one at a time. At its first call, it reads the
    /// limits of a Browse and a Translate. It browses each data type once per client,
    /// with one Browse request for each step of the walk of the types of a call.
    ///
    /// # Errors
    ///
    /// As [`Client::browse`], for each request that it sends, else
    /// `BadRequestTooLarge` when the request of one node does not fit one message, and
    /// `BadUnexpectedError` when an answer has a result for more or fewer nodes than
    /// its request.
    pub(crate) async fn describe(
        &mut self,
        nodes: &[Node],
    ) -> Result<Vec<Description>, Status> {
        let mut descriptions: Vec<Description> =
            nodes.iter().map(|_| Description::default()).collect();
        let mut types = vec![None; nodes.len()];
        let limits = self.limits().await?;
        let facts =
            self.split(nodes, self.per_read, FACTS.len(), Service::READ, |raw| {
                // SAFETY: `raw` and `FACTS` hold their counts, and C copies each name.
                unsafe {
                    ffi::shim_read_attributes_new(
                        raw.as_ptr(),
                        raw.len(),
                        FACTS.as_ptr(),
                        FACTS.len(),
                    )
                }
            })?;
        for (range, request) in facts {
            let answer = self.serve(&request).await?;
            answer.has(range.len() * FACTS.len())?;
            for (offset, index) in range.enumerate() {
                let fact = |attribute| answer.value(offset * FACTS.len() + attribute);
                types[index] = node_of(fact(0));
                // SAFETY: the value lives with the answer.
                if let Ok(Value::I32(rank)) = unsafe { Value::of(fact(1)) } {
                    descriptions[index].rank = Some(rank);
                }
                // SAFETY: the value lives with the answer.
                if let Ok(Value::U8(access)) = unsafe { Value::of(fact(2)) } {
                    descriptions[index].access = Some(access);
                }
            }
        }
        self.units(nodes, limits.translate, &mut descriptions)
            .await?;
        let bases = self.walk(&types, limits.browse).await?;
        for (description, base) in descriptions.iter_mut().zip(bases) {
            description.base = base;
        }
        Ok(descriptions)
    }

    /// Reads the [`Limits`] of the server at the first call, as [`limit`] gives each.
    async fn limits(&mut self) -> Result<Limits, Status> {
        if let Some(limits) = self.limits {
            return Ok(limits);
        }
        let nodes = LIMITS.map(|number| Node {
            namespace: 0,
            key: Key::Number(number),
        });
        let nodes = self
            .nodes(&nodes)
            .expect("invariant: a read of two nodes by number fits one message");
        let values = self.read(&nodes).await?;
        let [browse, translate] = values else {
            panic!("invariant: a read gives one value for each node");
        };
        let limits = Limits {
            browse: limit(browse),
            translate: limit(translate),
        };
        self.limits = Some(limits);
        Ok(limits)
    }

    /// Writes the unit of each node of `nodes` to its description, with Translate
    /// requests of at most `most` nodes.
    async fn units(
        &mut self,
        nodes: &[Node],
        most: Option<NonZeroUsize>,
        descriptions: &mut [Description],
    ) -> Result<(), Status> {
        let paths = self.split(nodes, most, 1, Service::TRANSLATE, |raw| {
            // SAFETY: `raw` holds its count, and C copies each name.
            unsafe { ffi::shim_units_new(raw.as_ptr(), raw.len()) }
        })?;
        let (mut found, mut units) = (Vec::new(), Vec::new());
        for (range, request) in paths {
            let answer = self.serve(&request).await?;
            answer.has(range.len())?;
            for (offset, index) in range.enumerate() {
                if let Some(unit) = answer.target(offset) {
                    found.push(index);
                    units.push(unit);
                }
            }
        }
        let reads = self.split(&units, self.per_read, 1, Service::READ, |raw| {
            // SAFETY: `raw` and `VALUE` hold their counts, and C copies each name.
            unsafe {
                ffi::shim_read_attributes_new(
                    raw.as_ptr(),
                    raw.len(),
                    VALUE.as_ptr(),
                    1,
                )
            }
        })?;
        for (range, request) in reads {
            let answer = self.serve(&request).await?;
            answer.has(range.len())?;
            for (offset, index) in range.enumerate() {
                descriptions[found[index]].unit = unit_of(answer.value(offset));
            }
        }
        Ok(())
    }

    /// Browses the supertype of each data type that a walk from `types` needs and the
    /// client has not browsed, until [`base`] knows each of `types`, with Browse
    /// requests of at most `most` nodes, and gives the base of each. The client keeps
    /// each supertype of a Good result. A result that is not Good gives no base in
    /// this call, and the next call browses that type again.
    async fn walk(
        &mut self,
        types: &[Option<Node>],
        most: Option<NonZeroUsize>,
    ) -> Result<Vec<Option<u32>>, Status> {
        let mut failed = Set::default();
        loop {
            let mut missing: Vec<Node> = Vec::new();
            let mut bases = Vec::with_capacity(types.len());
            for data_type in types {
                let Some(data_type) = data_type else {
                    bases.push(None);
                    continue;
                };
                match base(&self.supertypes, &failed, data_type) {
                    Ok(base) => bases.push(base),
                    Err(step) => {
                        if !missing.contains(&step) {
                            missing.push(step);
                        }
                    }
                }
            }
            if missing.is_empty() {
                return Ok(bases);
            }
            let browses = self.split(&missing, most, 1, Service::BROWSE, |raw| {
                // SAFETY: `raw` holds its count, and C copies each name.
                unsafe {
                    ffi::shim_browse_new(
                        raw.as_ptr(),
                        raw.len(),
                        HAS_SUBTYPE,
                        true,
                        DATA_TYPE,
                        0,
                    )
                }
            })?;
            for (range, request) in browses {
                let answer = self.serve(&request).await?;
                answer.has(range.len())?;
                for (offset, index) in range.enumerate() {
                    let mut first = None;
                    // A type has one supertype.
                    let result = answer
                        .browsed(offset, |found| first = first.take().or(found.node));
                    if result.is_ok() {
                        self.supertypes.insert(missing[index].clone(), first);
                    } else {
                        failed.insert(missing[index].clone());
                    }
                }
            }
        }
    }

    /// Builds the requests of `service` that `build` gives for `nodes`, in order, each
    /// for at most `most` over `items` nodes, at least 1, and in one message that the
    /// client sends.
    ///
    /// # Errors
    ///
    /// `BadRequestTooLarge` when the request of one node does not fit one message.
    ///
    /// # Aborts
    ///
    /// If open62541 cannot allocate a request, as [`alloc::exhausted`] says.
    fn split(
        &self,
        nodes: &[Node],
        most: Option<NonZeroUsize>,
        items: usize,
        service: Service,
        build: impl Fn(&[ffi::Node<'_>]) -> *mut c_void,
    ) -> Result<Vec<(Range<usize>, Request)>, Status> {
        let raw: Vec<ffi::Node<'_>> = nodes.iter().map(Into::into).collect();
        let most = most.map_or(nodes.len(), |most| (most.get() / items).max(1));
        let mut requests = Vec::new();
        let mut first = 0;
        while first < nodes.len() {
            let mut count = most.min(nodes.len() - first);
            loop {
                let range = first..first + count;
                let request = Request::new(build(&raw[range.clone()]), service);
                if request.bytes(self.token) <= self.sends {
                    requests.push((range, request));
                    break;
                }
                if count == 1 {
                    return Err(Status::BAD_REQUEST_TOO_LARGE);
                }
                count /= 2;
            }
            first += count;
        }
        Ok(requests)
    }

    /// Sends `request` and drives the client until its answer, as [`Client::read`]
    /// does with one request.
    ///
    /// # Errors
    ///
    /// As [`Client::browse`], but for the status of a result.
    async fn serve(&mut self, request: &Request) -> Result<Answer, Status> {
        self.asked = self.clock.now();
        let deadline = self.asked + self.timeout;
        self.reads.awaited.set(None);
        drop(self.reads.answer.take());
        let mut sent = false;
        self.ask(request, &mut sent)?;
        let served = self
            .connections
            .drive_until(Some(deadline), |_| {
                if let Err(status) = self.ask(request, &mut sent) {
                    return Poll::Ready(Err(status));
                }
                self.reads
                    .answer
                    .take()
                    .map_or(Poll::Pending, |answer| Poll::Ready(Ok(answer)))
            })
            .await;
        let answer = match served {
            Some(answer) => answer?,
            None if !sent => return Err(Status::BAD_TOO_MANY_OPERATIONS),
            None => return Err(Status::BAD_TIMEOUT),
        };
        // SAFETY: the answer lives.
        let status = Status(unsafe { ffi::shim_response_result(answer.raw.as_ptr()) });
        if status != Status::GOOD {
            return Err(status);
        }
        Ok(answer)
    }

    /// Sends `request` unless `sent`, or [`READS`] are in flight, and then sets
    /// `sent`.
    fn ask(&self, request: &Request, sent: &mut bool) -> Result<(), Status> {
        if *sent || self.reads.pending.get() == READS {
            return Ok(());
        }
        let mut key = 0;
        // SAFETY: the client and the request live, and `reads` outlives the client.
        let status = Status(unsafe {
            ffi::shim_client_service(
                self.raw.as_ptr(),
                request.raw.as_ptr(),
                request.service,
                served,
                ptr::from_ref::<Reads>(&self.reads).cast_mut().cast(),
                &raw mut key,
            )
        });
        if status != Status::GOOD {
            return Err(status);
        }
        self.reads.pending.set(self.reads.pending.get() + 1);
        self.reads.awaited.set(Some((key, request.service)));
        *sent = true;
        Ok(())
    }
}

/// Gives the base of `data_type` from the supertype of each data type that a client
/// browsed: the first type up the `HasSubtype` references that is a built-in type of
/// namespace 0, Int32 for a subtype of `Enumeration`, and none for `Structure`,
/// `BaseDataType`, `Number`, `Integer`, `UInteger`, a type with no supertype or one
/// whose id is not a number or a name, a type of `failed`, and a walk of more than
/// [`STEPS`] steps, such as a loop.
///
/// # Errors
///
/// The first type of the walk that the client has not browsed and that is not of
/// `failed`.
pub(super) fn base(
    supertypes: &Map<Node, Option<Node>>,
    failed: &Set<Node>,
    data_type: &Node,
) -> Result<Option<u32>, Node> {
    let mut at = data_type;
    for _ in 0..=STEPS {
        if let (0, Key::Number(id)) = (at.namespace, &at.key) {
            match *id {
                1..=21 | 23 | 25 => return Ok(Some(*id)),
                ENUMERATION => return Ok(Some(INT32)),
                22 | 24 | 26..=28 => return Ok(None),
                _ => {}
            }
        }
        if failed.contains(at) {
            return Ok(None);
        }
        match supertypes.get(at) {
            None => return Err(at.clone()),
            Some(None) => return Ok(None),
            Some(Some(supertype)) => at = supertype,
        }
    }
    Ok(None)
}

/// Gives the node of `value`, an answer of a read of `DataType`.
fn node_of(value: *const ffi::DataValue) -> Option<Node> {
    let mut node = ffi::Node::default();
    // SAFETY: the value lives, as the caller ensures, and `node` is a place to write.
    let status = Status(unsafe { ffi::shim_value_node(value, &raw mut node) });
    // SAFETY: on Good, the name of `node` borrows from the value.
    (status == Status::GOOD).then(|| unsafe { node.owned() })?
}

/// Gives the unit of `value`, an answer of a read of `EngineeringUnits`.
fn unit_of(value: *const ffi::DataValue) -> Option<Unit> {
    let (mut uri, mut code, mut text) =
        (ffi::Bytes::default(), 0, ffi::Bytes::default());
    // SAFETY: the value lives, as the caller ensures, and the outputs are places to
    // write.
    let status = Status(unsafe {
        ffi::shim_value_unit(value, &raw mut uri, &raw mut code, &raw mut text)
    });
    if status != Status::GOOD {
        return None;
    }
    // SAFETY: on Good, `uri` borrows from the value.
    let cefact = unsafe { uri.to_vec() } == CEFACT;
    // SAFETY: on Good, `text` borrows from the value.
    let text = unsafe { text.lossy() };
    Some(Unit {
        text: (!text.is_empty()).then_some(text),
        code: code_of(code).filter(|_| cefact),
    })
}

/// Gives the UN/CEFACT Recommendation 20 code of the `unitId` `id`: its bytes from
/// the most significant, after the zero bytes, when they are 2 or 3 ASCII capital
/// letters or digits.
pub(super) fn code_of(id: i32) -> Option<String> {
    let bytes = id.to_be_bytes();
    let first = bytes.iter().position(|&byte| byte != 0)?;
    let code = &bytes[first..];
    let read = (2..=3).contains(&code.len())
        && code
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    read.then(|| code.iter().map(|&byte| char::from(byte)).collect())
}

/// A request of a service, which C owns until the drop.
struct Request {
    raw: NonNull<c_void>,
    service: Service,
}

impl Request {
    /// Takes `raw`, a request of `service` from C, or NULL when C could not allocate
    /// it.
    ///
    /// # Aborts
    ///
    /// On NULL, as [`alloc::exhausted`] says.
    fn new(raw: *mut c_void, service: Service) -> Self {
        let Some(raw) = NonNull::new(raw) else {
            alloc::exhausted()
        };
        Self { raw, service }
    }

    /// Gives the bytes of the body of its message with a session token of `token`
    /// encoded bytes.
    fn bytes(&self, token: usize) -> usize {
        // SAFETY: the request lives.
        unsafe { ffi::shim_request_bytes(self.raw.as_ptr(), self.service, token) }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        // SAFETY: the request lives, and nothing else holds it.
        unsafe { ffi::shim_request_free(self.raw.as_ptr(), self.service) };
    }
}

/// The answer of a service, moved out of open62541.
pub(super) struct Answer {
    raw: NonNull<c_void>,
    service: Service,
}

/// A reference of a browse, copied out of its answer.
struct Found {
    node: Option<Node>,
    kind: Option<Node>,
    name: String,
    display: String,
    class: u32,
}

impl Answer {
    /// Checks that the answer has `count` results.
    ///
    /// # Errors
    ///
    /// `BadUnexpectedError` when it has more or fewer.
    fn has(&self, count: usize) -> Result<(), Status> {
        // SAFETY: the answer lives.
        if unsafe { ffi::shim_answer_count(self.raw.as_ptr()) } == count {
            Ok(())
        } else {
            Err(Status::BAD_UNEXPECTED_ERROR)
        }
    }

    /// Gives the node of the unit of result `index` of a Translate answer with more
    /// results than `index`, or `None` when the result has none in this server by a
    /// number or a name.
    fn target(&self, index: usize) -> Option<Node> {
        let mut node = ffi::Node::default();
        // SAFETY: the answer lives and has more results than `index`, and `node` is a
        // place to write.
        let found =
            unsafe { ffi::shim_units_target(self.raw.as_ptr(), index, &raw mut node) };
        // SAFETY: when found, the name of `node` borrows from the answer.
        found.then(|| unsafe { node.owned() })?
    }

    /// Gives the value of result `index` of a Read answer with more results than
    /// `index`, which lives with the answer.
    fn value(&self, index: usize) -> *const ffi::DataValue {
        // SAFETY: the answer lives, and has more results than `index`.
        unsafe { ffi::shim_read_result(self.raw.as_ptr().cast(), index) }
    }

    /// Gives the page of the one result of a Browse or `BrowseNext` answer, with each
    /// reference of the classes of [`CLASSES`].
    ///
    /// # Errors
    ///
    /// The status of the result when it is not Good, or `BadUnexpectedError` when
    /// the answer has no result or more than one.
    fn page(&self) -> Result<Page, Status> {
        self.has(1)?;
        let mut references = Vec::new();
        let next = self.browsed(0, |found| {
            let class = match found.class {
                1 => Class::Object,
                2 => Class::Variable,
                128 => Class::View,
                // A server that does not filter by the class of the request.
                _ => return,
            };
            references.push(Reference {
                node: found.node,
                name: found.name,
                display: found.display,
                class,
                kind: found.kind,
            });
        })?;
        Ok(Page { references, next })
    }

    /// Gives each reference of result `index` of a Browse or `BrowseNext` answer with
    /// more results than `index` to `each`, in order, and gives its continuation
    /// point.
    ///
    /// # Errors
    ///
    /// The status of the result when it is not Good.
    fn browsed(
        &self,
        index: usize,
        mut each: impl FnMut(Found),
    ) -> Result<Option<Point>, Status> {
        let raw = self.raw.as_ptr();
        let (mut count, mut point) = (0, ffi::Bytes::default());
        // SAFETY: the answer lives and has more results than `index`.
        let status = Status(unsafe {
            ffi::shim_browse_result(raw, index, &raw mut count, &raw mut point)
        });
        if status != Status::GOOD {
            return Err(status);
        }
        for reference in 0..count {
            let mut out = ffi::Reference::default();
            // SAFETY: the result has `count` references, and `out` is a place to
            // write.
            unsafe { ffi::shim_browse_reference(raw, index, reference, &raw mut out) };
            // SAFETY: each text and the name of the node borrow from the answer, which
            // lives through the copies.
            let node = unsafe { out.node.owned() }.filter(|_| out.local);
            // SAFETY: as above.
            let kind = unsafe { out.kind.owned() }.filter(|_| out.typed);
            // SAFETY: as above.
            let name = unsafe { out.name.lossy() };
            // SAFETY: as above.
            let display = unsafe { out.display.lossy() };
            each(Found {
                node,
                kind,
                name,
                display,
                class: out.class,
            });
        }
        // SAFETY: the point borrows from the answer.
        let point = unsafe { point.to_vec() };
        Ok((!point.is_empty()).then_some(Point(point)))
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        // SAFETY: the answer lives, and nothing else holds it.
        unsafe { ffi::shim_answer_free(self.raw.as_ptr(), self.service) };
    }
}

/// Ends one request of the [`Reads`] at `data`. When it is the request that the
/// client waits for, moves its answer there.
///
/// # Safety
///
/// `data` points at live [`Reads`], and `response` at the answer.
unsafe extern "C" fn served(
    _: *mut ffi::Client,
    data: *mut c_void,
    request: u32,
    response: *mut c_void,
) {
    // SAFETY: the client that holds the reads lives.
    let reads = unsafe { &*data.cast_const().cast::<Reads>() };
    reads.pending.set(reads.pending.get() - 1);
    let Some((key, service)) = reads.awaited.get() else {
        return;
    };
    if key != request {
        return;
    }
    reads.awaited.set(None);
    // SAFETY: open62541 gives a live answer of the service of the request.
    let raw = unsafe { ffi::shim_answer_move(response, service) };
    let Some(raw) = NonNull::new(raw) else {
        alloc::exhausted()
    };
    reads.answer.set(Some(Answer { raw, service }));
}
