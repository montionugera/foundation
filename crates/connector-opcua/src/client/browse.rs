//! The browse of a server: the references of a node, and what a server says of each
//! variable.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::num::{NonZeroU32, NonZeroUsize};

use types::sample::Scalar;

use super::service::{self, Answer, Item, Request, Sizes};
use super::{Client, id_bytes, limit};
use crate::ffi::{self, Service, Status};
use crate::node::ns0::{ENUMERATION, HAS_SUBTYPE};
use crate::node::{Key, Node};
use crate::value::{SCALARS, Value};

/// `HierarchicalReferences`.
const HIERARCHICAL: u32 = 33;

/// The mask of the node classes Object (1), Variable (2), and View (128).
const CLASSES: u32 = 0b1000_0011;

/// The mask of the node class `DataType`.
const DATA_TYPE: u32 = 64;

/// The attributes `DataType`, `ValueRank`, and `UserAccessLevel`.
const FACTS: [u32; 3] = [14, 15, 18];

/// The variables `MaxNodesPerBrowse`, `MaxNodesPerTranslateBrowsePathsToNodeIds`, and
/// `MaxBrowseContinuationPoints`.
const BOUNDS: [u32; 3] = [11710, 11712, 2735];

/// The `namespaceUri` of an `EUInformation` whose `unitId` is a code of UN/CEFACT
/// Recommendation 20.
const CEFACT: &[u8] = b"http://www.opcfoundation.org/UA/units/un/cefact";

/// The most steps of a walk up the `HasSubtype` references.
const STEPS: usize = 16;

/// The id of `Int32`.
const INT32: u32 = 6;

/// The most supertypes that a client keeps past a call of [`Client::describe`]: a call
/// that ends with more, or drops in its walk, clears them.
const SUPERTYPES: usize = 1024;

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
    /// The node, or its text when [`Node`] cannot hold it.
    pub(crate) node: Result<Node, Foreign>,
    /// The text of its browse name.
    pub(crate) name: String,
    /// The text of its display name.
    pub(crate) display: String,
    pub(crate) class: Class,
    /// Its reference type, such as `Organizes` (`i=35`), or `None` when the id of the
    /// type is not a number or a name.
    pub(crate) via: Option<Node>,
}

/// A node of a [`Reference`] that [`Node`] cannot hold.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Foreign {
    /// The bytes of its text in the form of OPC 10000-6, 5.3.1.10, such as
    /// `ns=1;g=<guid>`, with the bytes of a name as the server gave them.
    pub(crate) text: Vec<u8>,
    pub(crate) cause: Cause,
}

/// Why [`Node`] cannot hold a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// Its key is a GUID or opaque.
    Key,
    /// Its key is a name that is empty or not UTF-8.
    Name,
    /// It is a node of another server, or of a namespace by its URI.
    Remote,
}

/// The node class of a [`Reference`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Object,
    Variable,
    View,
    /// A class that the request did not ask for.
    Other,
}

/// What a server says of a variable.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Description {
    /// The built-in type of its data type, as [`base`] gives it. `None` also when a
    /// type on the walk has an id that is not a number or a name.
    pub(crate) base: Option<Base>,
    /// Its `ValueRank`: -1 for a scalar.
    pub(crate) rank: Option<i32>,
    /// Its `UserAccessLevel`.
    pub(crate) access: Option<u8>,
    /// Its `EngineeringUnits`.
    pub(crate) unit: Option<Unit>,
}

/// The built-in type of the data type of a variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Base {
    /// A type from Boolean to Double, which a read decodes, as its sample type.
    Scalar(Scalar),
    /// Another built-in type, by its name, such as `String`.
    Other(&'static str),
}

impl Base {
    /// Gives the base of the built-in type of the id `id` in namespace 0.
    ///
    /// # Panics
    ///
    /// If `id` is not of a built-in type, 1 to 25.
    pub(crate) fn of(id: u32) -> Self {
        let index = usize::try_from(id - 1).expect("invariant: a u32 fits a usize");
        // The index of a built-in type in `UA_TYPES` is its id less 1, as `shim.c`
        // asserts, and `SCALARS` is at the same index.
        match SCALARS.get(index) {
            Some(&scalar) => Self::Scalar(scalar),
            None => Self::Other(ffi::types()[index].name()),
        }
    }
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

/// The bounds of a server on the nodes of one request of a service, other than Read.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    /// Its `MaxNodesPerBrowse`.
    browse: Option<NonZeroUsize>,
    /// Its `MaxNodesPerTranslateBrowsePathsToNodeIds`.
    translate: Option<NonZeroUsize>,
    /// Its `MaxBrowseContinuationPoints`.
    points: Option<NonZeroUsize>,
}

impl Client {
    /// Browses the forward hierarchical references of `node` and their subtypes, to the
    /// node classes Object, Variable, and View, and gives the first page of at most
    /// `most` references, in server order, with [`Class::Other`] for a reference of
    /// another class from a server that does not filter by class. It asks for fewer
    /// when an answer of `most` references, each with its texts at 256 bytes, would be
    /// past one message that the client takes, but for at least 1. `None` when the
    /// server does not have `node`.
    ///
    /// # Errors
    ///
    /// `BadRequestTooLarge`, with nothing sent, when the request does not fit one
    /// message that the client sends, `BadTimeout` when the answer does not come in
    /// the request timeout of the client (5 s), `BadTooManyOperations` when the client
    /// cannot send in that time, as [`Client::read`] says, else the status of
    /// open62541 when it refuses the request, the service result, or the status of
    /// the result of `node` when it is not Good and not `BadNodeIdUnknown`, such as
    /// `BadNoContinuationPoints`.
    pub(crate) async fn browse(
        &mut self,
        node: &Node,
        most: NonZeroU32,
    ) -> Result<Option<Page>, Status> {
        let build = |raw: &[ffi::Node<'_>], most| {
            Request::browse(raw, HIERARCHICAL, false, CLASSES, most)
        };
        let fixed = service::answer_bytes(Service::BROWSE) + Item::Result.bytes();
        let fits = self.takes.saturating_sub(fixed) / Item::Reference.bytes();
        let fits = NonZeroU32::new(u32::try_from(fits).unwrap_or(u32::MAX))
            .unwrap_or(NonZeroU32::MIN);
        let answer = self.serve(&build(&[node.into()], most.min(fits))).await?;
        match page(&answer) {
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
        let answer = self
            .serve(&Request::browse_next(&[&point.0], false))
            .await?;
        page(&answer)
    }

    /// Releases each of `points`, points of this client's session, with `BrowseNext`
    /// requests of at most the `MaxNodesPerBrowse` of the server. A point whose
    /// request alone does not fit one message stays until the session ends.
    ///
    /// # Errors
    ///
    /// As [`Client::browse`], for each request that it sends.
    pub(crate) async fn release(&mut self, points: &[Point]) -> Result<(), Status> {
        let most = self.bounds().await?.browse;
        let sizes = Sizes::points(self.token);
        let split =
            self.split(points.len(), most, &sizes, |index| points[index].0.len());
        for range in split.ranges {
            let points: Vec<&[u8]> =
                points[range].iter().map(|point| &point.0[..]).collect();
            self.serve(&Request::browse_next(&points, true)).await?;
        }
        Ok(())
    }

    /// Gives the most continuation points that the server holds for a session, its
    /// `MaxBrowseContinuationPoints`, or `None` when it gives 0 or no `UInt16`. It
    /// reads the bounds of the server once per client, as [`Client::describe`] does.
    ///
    /// # Errors
    ///
    /// As [`Client::read`].
    pub(crate) async fn points(&mut self) -> Result<Option<NonZeroUsize>, Status> {
        Ok(self.bounds().await?.points)
    }

    /// Gives the description of each variable of `nodes`, in order. A fact that the
    /// server does not give, or gives with a status that is not Good, is `None`. It
    /// splits each request by the limit of the server for its service
    /// (`MaxNodesPerRead`, `MaxNodesPerBrowse`, or
    /// `MaxNodesPerTranslateBrowsePathsToNodeIds`) and so that it and its answer, with
    /// each text at 256 bytes, fit one message of the session, and sends one at a
    /// time. A fact whose request alone, or its answer, does not fit is `None`. At its
    /// first call, it reads the limits of a Browse and a Translate. It browses each
    /// data type once per client, with one Browse request for each step of the walk of
    /// the types of a call, and keeps at most 1024 supertypes past a call. A call that
    /// ends with more, or drops in its walk, clears them.
    ///
    /// # Errors
    ///
    /// As [`Client::browse`], for each request that it sends, else
    /// `BadUnexpectedError` when an answer has a result for more or fewer items than
    /// its request.
    pub(crate) async fn describe(
        &mut self,
        nodes: &[Node],
    ) -> Result<Vec<Description>, Status> {
        let mut descriptions: Vec<Description> =
            nodes.iter().map(|_| Description::default()).collect();
        let bounds = self.bounds().await?;
        let types = self.facts(nodes, &mut descriptions).await?;
        self.units(nodes, bounds.translate, &mut descriptions)
            .await?;
        // A drop of the call in the walk drops them, so they never pass the cap.
        let mut supertypes = mem::take(&mut self.supertypes);
        let bases = self.walk(&types, bounds.browse, &mut supertypes).await;
        if supertypes.len() <= SUPERTYPES {
            self.supertypes = supertypes;
        }
        let bases = bases?;
        for (description, base) in descriptions.iter_mut().zip(bases) {
            description.base = base;
        }
        Ok(descriptions)
    }

    /// Reads the [`Bounds`] of the server at the first call, as [`limit`] gives each,
    /// and the points as a `UInt16` above 0.
    async fn bounds(&mut self) -> Result<Bounds, Status> {
        if let Some(bounds) = self.bounds {
            return Ok(bounds);
        }
        let nodes = BOUNDS.map(Node::ns0);
        let nodes = self.numbered(&nodes);
        let values = self.read(&nodes).await?;
        let [browse, translate, points] = values else {
            panic!("invariant: a read gives one value for each node");
        };
        let points = match points {
            Ok(Value::U16(most)) => NonZeroUsize::new(usize::from(*most)),
            _ => None,
        };
        let bounds = Bounds {
            browse: limit(browse),
            translate: limit(translate),
            points,
        };
        self.bounds = Some(bounds);
        Ok(bounds)
    }

    /// Writes the `ValueRank` and the `UserAccessLevel` of each node of `nodes` to its
    /// description, and gives the data type of each, with Reads of the [`FACTS`] of
    /// each node.
    async fn facts(
        &mut self,
        nodes: &[Node],
        descriptions: &mut [Description],
    ) -> Result<Vec<Option<Node>>, Status> {
        let per = FACTS.len();
        let mut types = vec![None; nodes.len()];
        // Each fact is sized as the largest, the data type.
        let sizes = self.sizes(Request::values, Item::Fact.bytes());
        let split = self.split(nodes.len() * per, self.per_read, &sizes, |index| {
            id_bytes(&nodes[index / per])
        });
        for range in split.ranges {
            let raw: Vec<ffi::Node<'_>> = range
                .clone()
                .map(|index| (&nodes[index / per]).into())
                .collect();
            let attributes: Vec<u32> =
                range.clone().map(|index| FACTS[index % per]).collect();
            let answer = self.serve(&Request::attributes(&raw, &attributes)).await?;
            answer.has(range.len())?;
            for (offset, index) in range.enumerate() {
                let (node, fact) = (index / per, answer.value(offset));
                let description = &mut descriptions[node];
                match index % per {
                    0 => types[node] = node_of(fact),
                    1 => {
                        // SAFETY: the value lives with the answer.
                        if let Ok(Value::I32(rank)) = unsafe { Value::of(fact) } {
                            description.rank = Some(rank);
                        }
                    }
                    _ => {
                        // SAFETY: the value lives with the answer.
                        if let Ok(Value::U8(access)) = unsafe { Value::of(fact) } {
                            description.access = Some(access);
                        }
                    }
                }
            }
        }
        Ok(types)
    }

    /// Writes the unit of each node of `nodes` to its description, with Translate
    /// requests of at most `most` nodes.
    async fn units(
        &mut self,
        nodes: &[Node],
        most: Option<NonZeroUsize>,
        descriptions: &mut [Description],
    ) -> Result<(), Status> {
        let sizes = self.sizes(Request::units, Item::Path.bytes());
        let split =
            self.split(nodes.len(), most, &sizes, |index| id_bytes(&nodes[index]));
        let (mut found, mut units) = (Vec::new(), Vec::new());
        for range in split.ranges {
            let answer = self
                .serve(&Request::units(&borrowed(&nodes[range.clone()])))
                .await?;
            answer.has(range.len())?;
            for (offset, index) in range.enumerate() {
                if let Some(unit) = answer.target(offset) {
                    found.push(index);
                    units.push(unit);
                }
            }
        }
        let sizes = self.sizes(Request::values, Item::Unit.bytes());
        let split = self.split(units.len(), self.per_read, &sizes, |index| {
            id_bytes(&units[index])
        });
        for range in split.ranges {
            let answer = self
                .serve(&Request::values(&borrowed(&units[range.clone()])))
                .await?;
            answer.has(range.len())?;
            for (offset, index) in range.enumerate() {
                descriptions[found[index]].unit = unit_of(answer.value(offset));
            }
        }
        Ok(())
    }

    /// Browses the supertype of each data type that a walk from `types` needs and the
    /// client has not browsed, until [`base`] knows each of `types`, with Browse
    /// requests of at most `most` nodes, and gives the base of each. It adds to
    /// `supertypes` the first supertype of each Good result, and releases the
    /// continuation point of a type with more. A result that is not Good, or a type
    /// whose request or answer does not fit one message, gives no base in this call,
    /// and the next call browses that type again.
    async fn walk(
        &mut self,
        types: &[Option<Node>],
        most: Option<NonZeroUsize>,
        supertypes: &mut BTreeMap<Node, Option<Node>>,
    ) -> Result<Vec<Option<Base>>, Status> {
        let build = |raw: &[ffi::Node<'_>]| {
            Request::browse(raw, HAS_SUBTYPE, true, DATA_TYPE, NonZeroU32::MIN)
        };
        let sizes = self.sizes(build, Item::Result.bytes() + Item::Reference.bytes());
        let mut failed = BTreeSet::new();
        loop {
            let mut missing = BTreeSet::new();
            let mut bases = Vec::with_capacity(types.len());
            for data_type in types {
                match data_type
                    .as_ref()
                    .map(|data_type| base(supertypes, &failed, data_type))
                {
                    None => bases.push(None),
                    Some(Ok(base)) => bases.push(base),
                    Some(Err(step)) => {
                        missing.insert(step);
                    }
                }
            }
            if missing.is_empty() {
                return Ok(bases);
            }
            let missing: Vec<Node> = missing.into_iter().collect();
            let split = self.split(missing.len(), most, &sizes, |index| {
                id_bytes(&missing[index])
            });
            failed.extend(split.large.iter().map(|large| missing[large.index].clone()));
            let mut points = Vec::new();
            for range in split.ranges {
                let answer = self
                    .serve(&build(&borrowed(&missing[range.clone()])))
                    .await?;
                answer.has(range.len())?;
                for (offset, index) in range.enumerate() {
                    let mut first = None;
                    let browsed = answer.browsed(offset, |found| {
                        first = first.take().or(found.node.ok());
                    });
                    if let Ok(point) = browsed {
                        supertypes.insert(missing[index].clone(), first);
                        points.extend(point.map(Point));
                    } else {
                        failed.insert(missing[index].clone());
                    }
                }
            }
            self.release(&points).await?;
        }
    }
}

/// Gives the page of the one result of `answer`, a Browse or `BrowseNext` answer, with
/// each reference, and [`Class::Other`] for a class out of [`CLASSES`].
///
/// # Errors
///
/// The status of the result when it is not Good, or `BadUnexpectedError` when the
/// answer has no result or more than one.
fn page(answer: &Answer) -> Result<Page, Status> {
    answer.has(1)?;
    let mut references = Vec::new();
    let next = answer.browsed(0, |found| {
        let class = match found.class {
            1 => Class::Object,
            2 => Class::Variable,
            128 => Class::View,
            _ => Class::Other,
        };
        references.push(Reference {
            node: found.node,
            name: found.name,
            display: found.display,
            class,
            via: found.via,
        });
    })?;
    Ok(Page {
        references,
        next: next.map(Point),
    })
}

/// The nodes of `nodes`, which borrow their names.
fn borrowed(nodes: &[Node]) -> Vec<ffi::Node<'_>> {
    nodes.iter().map(Into::into).collect()
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
fn base(
    supertypes: &BTreeMap<Node, Option<Node>>,
    failed: &BTreeSet<Node>,
    data_type: &Node,
) -> Result<Option<Base>, Node> {
    let mut at = data_type;
    for _ in 0..=STEPS {
        if let (0, Key::Number(id)) = (at.namespace, &at.key) {
            match *id {
                1..=21 | 23 | 25 => return Ok(Some(Base::of(*id))),
                ENUMERATION => return Ok(Some(Base::of(INT32))),
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
fn code_of(id: i32) -> Option<String> {
    let bytes = id.to_be_bytes();
    let first = bytes.iter().position(|&byte| byte != 0)?;
    let code = &bytes[first..];
    let read = (2..=3).contains(&code.len())
        && code
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    read.then(|| code.iter().map(|&byte| char::from(byte)).collect())
}
