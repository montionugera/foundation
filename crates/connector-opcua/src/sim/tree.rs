//! The tree that [`super::serve`] serves, as the module doc of [`super`] gives it.

use std::ffi::CStr;

use crate::node::{Key, Name, Node};
use crate::server::{self, EXTENSION_OBJECT, Server};
use crate::value::Value;

/// The most references of each page of a Browse, so `Plant.Types` takes two pages.
pub(crate) const REFERENCES: u32 = 8;

/// The path of each node of the tree, its name in namespace 1.
pub(crate) const PLANT: &str = "Plant";
pub(crate) const TYPES: &str = "Plant.Types";
pub(crate) const LINE: &str = "Plant.Line";
pub(crate) const TEMPERATURE: &str = "Plant.Line.Temperature";
pub(crate) const UNIT: &str = "Plant.Line.Temperature.EngineeringUnits";
pub(crate) const SPEED: &str = "Plant.Line.Speed";
pub(crate) const MODE: &str = "Plant.Line.Mode";
pub(crate) const PROFILE: &str = "Plant.Line.Profile";
pub(crate) const LOCKED: &str = "Plant.Line.Locked";
pub(crate) const RECIPE: &str = "Plant.Line.Recipe";
pub(crate) const ODD: &str = "Plant.Odd";
pub(crate) const LEVEL: &str = "Plant.Odd.Level";
pub(crate) const TWIN: &str = "Plant.Odd.Twin";

/// The name of each data type of the tree in namespace 1.
pub(crate) const SPEED_TYPE: &str = "Speed";
pub(crate) const MODE_TYPE: &str = "Mode";
pub(crate) const RECIPE_TYPE: &str = "Recipe";

/// The number in namespace 0 of each type that the tree adds, since the namespace 0
/// of the build has none of them.
#[cfg(test)]
pub(crate) const NS0_TYPES: [u32; 3] = [DOUBLE, ENUMERATION, STRUCTURE];

const DOUBLE: u32 = 11;
const OBJECTS: u32 = 85;
const STRUCTURE: u32 = 22;
const BASE_DATA_TYPE: u32 = 24;
const ENUMERATION: u32 = 29;
const ORGANIZES: u32 = 35;
const HAS_COMPONENT: u32 = 47;
/// The `AccessLevel` bit `CurrentRead`.
const READ: u8 = 1;

/// Gives the node of `name` in namespace 1.
///
/// # Panics
///
/// If `name` is empty.
pub(crate) fn node(name: &str) -> Node {
    let name = Name::new(name).expect("invariant: a node of the tree has a name");
    Node {
        namespace: 1,
        key: Key::Name(name),
    }
}

/// Adds the tree to `server`.
///
/// # Panics
///
/// If open62541 refuses a node of the tree, such as one that the server has.
pub(crate) fn add(server: &Server) {
    server.add_folder(&node(PLANT), &ns0(OBJECTS), ORGANIZES, c"Plant", c"Plant");
    add_types(server);
    add_line(server);
    add_odd(server);
}

/// Adds `Plant.Types` and a variable of each type.
fn add_types(server: &Server) {
    server.add_folder(&node(TYPES), &node(PLANT), ORGANIZES, c"Types", c"Types");
    let numbers = [
        (c"Boolean", Value::Bool(true)),
        (c"SByte", Value::I8(-8)),
        (c"Byte", Value::U8(8)),
        (c"Int16", Value::I16(-16)),
        (c"UInt16", Value::U16(16)),
        (c"Int32", Value::I32(-32)),
        (c"UInt32", Value::U32(32)),
        (c"Int64", Value::I64(-64)),
        (c"UInt64", Value::U64(64)),
        (c"Float", Value::F32(0.5)),
        (c"Double", Value::F64(0.25)),
    ]
    .map(|(name, value)| (name, server::encoded(value)));
    let text = [&4_i32.to_le_bytes()[..], b"text"].concat();
    // The DateTime of the Unix epoch, in 100 ns since 1601.
    let epoch = 116_444_736_000_000_000_i64.to_le_bytes().to_vec();
    let guid = [
        &1_u32.to_le_bytes()[..],
        &[2, 0, 3, 0, 4, 0, 0, 0, 0, 0, 0, 0],
    ]
    .concat();
    let others = [
        (c"String", (server::STRING, text)),
        (c"DateTime", (server::DATE_TIME, epoch)),
        (c"Guid", (server::GUID, guid)),
        (c"ByteString", (server::BYTE_STRING, two_bytes())),
    ];
    for (name, (kind, encoded)) in numbers.into_iter().chain(others) {
        let path = format!("{TYPES}.{}", name.to_str().expect("an ASCII name"));
        let data_type = server::type_of(kind);
        add_value(server, &path, TYPES, name, &data_type, (kind, encoded));
    }
}

/// Adds the data types of the tree, `Plant.Line`, and its components.
fn add_line(server: &Server) {
    let base = ns0(BASE_DATA_TYPE);
    let (double, enumeration, structure) =
        (ns0(DOUBLE), ns0(ENUMERATION), ns0(STRUCTURE));
    server.add_data_type(&double, &base, c"Double");
    server.add_data_type(&enumeration, &base, c"Enumeration");
    server.add_data_type(&structure, &base, c"Structure");
    server.add_data_type(&node(SPEED_TYPE), &double, c"Speed");
    server.add_data_type(&node(MODE_TYPE), &enumeration, c"Mode");
    server.add_data_type(&node(RECIPE_TYPE), &structure, c"Recipe");
    let line = node(LINE);
    server.add_folder(&line, &node(PLANT), HAS_COMPONENT, c"Line", c"Line");
    server.add_reference(&line, ORGANIZES, &node(PLANT));
    let temperature = server::encoded(Value::F64(21.5));
    add_value(
        server,
        TEMPERATURE,
        LINE,
        c"Temperature",
        &double,
        temperature,
    );
    let cefact = c"http://www.opcfoundation.org/UA/units/un/cefact";
    server.add_unit(&node(UNIT), &node(TEMPERATURE), cefact, c"°C", 0x0043_454C);
    let speed = server::encoded(Value::F64(1.25));
    add_value(server, SPEED, LINE, c"Speed", &node(SPEED_TYPE), speed);
    let component = |path, name, data_type: &Node, rank, access| {
        server.add_variable(
            &node(path),
            &line,
            HAS_COMPONENT,
            name,
            data_type,
            rank,
            access,
        );
    };
    // open62541 refuses an Int32 value of a subtype of Enumeration.
    component(MODE, c"Mode", &node(MODE_TYPE), -1, READ);
    component(PROFILE, c"Profile", &double, 1, READ);
    component(LOCKED, c"Locked", &double, -1, 0);
    // An ExtensionObject of the type `Recipe`: its NodeId, then a ByteString body.
    let name = RECIPE_TYPE.as_bytes();
    let length = i32::try_from(name.len()).expect("a short name");
    let recipe = [
        &[3, 1, 0][..],
        &length.to_le_bytes(),
        name,
        &[1],
        &two_bytes(),
    ]
    .concat();
    let recipe = (EXTENSION_OBJECT, recipe);
    add_value(server, RECIPE, LINE, c"Recipe", &node(RECIPE_TYPE), recipe);
}

/// Adds `Plant.Odd` and its nodes.
fn add_odd(server: &Server) {
    let odd = node(ODD);
    server.add_folder(&odd, &node(PLANT), ORGANIZES, c"Odd", c"Odd");
    server.add_guid(&odd);
    let double = ns0(DOUBLE);
    add_value(
        server,
        LEVEL,
        ODD,
        c"Level",
        &double,
        server::encoded(Value::F64(2.5)),
    );
    server.add_reference(&odd, ORGANIZES, &node(LEVEL));
    add_value(
        server,
        TWIN,
        ODD,
        c"Level",
        &double,
        server::encoded(Value::F64(3.5)),
    );
}

/// Adds the variable `path` of `parent` by `HasComponent`, with the browse name `name`,
/// the `DataType` `data_type`, and a value of a kind and its encoding.
fn add_value(
    server: &Server,
    path: &str,
    parent: &str,
    name: &CStr,
    data_type: &Node,
    (kind, encoded): (u16, Vec<u8>),
) {
    let (node, parent) = (node(path), node(parent));
    server.add_value(
        &node,
        &parent,
        HAS_COMPONENT,
        name,
        data_type,
        kind,
        &encoded,
    );
}

/// The encoding of the `ByteString` 1, 2.
fn two_bytes() -> Vec<u8> {
    [&2_i32.to_le_bytes()[..], &[1, 2]].concat()
}

const fn ns0(number: u32) -> Node {
    Node {
        namespace: 0,
        key: Key::Number(number),
    }
}
