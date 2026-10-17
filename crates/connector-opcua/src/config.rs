//! The config of one `opcua` connector, read from its document.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::ffi::CString;

use connector::kind;
use document::diagnostic::{Code, Diagnostic, Note};
use document::value::Value;
use document::{Attribute, Block, Document, Label, Map, read};
use types::name::Name;
use types::time::Rate;

use crate::client;
use crate::node::Node;

#[cfg(test)]
mod tests;

const BAD_ADDRESS: Code = Code::new("opcua.bad-address");
const BAD_NODE_ID: Code = Code::new("opcua.bad-node-id");
const NO_READ: Code = Code::new("opcua.no-read");
const REPEATED_CHANNEL: Code = Code::new("opcua.repeated-channel");

/// The scheme of each address.
const SCHEME: &str = "opc.tcp://";

/// One opcua connector's config: the endpoint of the server, the rate, and the node of
/// each channel. Only [`Kind::parse`](connector::kind::Kind::parse) of
/// [`Kind`](crate::Kind) makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The endpoint URL, which open62541 reads as a host and an optional port and
    /// path.
    pub(crate) address: CString,
    pub(crate) rate: Rate,
    /// One or more, each of another channel.
    pub(crate) reads: Vec<Read>,
}

/// The node of one channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Read {
    pub(crate) channel: Name,
    pub(crate) node: Node,
}

/// Reads `address`, `rate`, and one or more `read` blocks, and no other key or block.
pub(crate) fn parse(config: &Document) -> Result<Config, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let address = read::required(
        config,
        kind::NOUN,
        None,
        "address",
        address,
        "Add an `address` attribute with the endpoint of the server, such as \
         \"opc.tcp://plc:4840\""
            .into(),
    );
    let rate = read::required(
        config,
        kind::NOUN,
        None,
        "rate",
        read::rate,
        "Add a `rate` attribute with the reads per second, such as \"10Hz\"".into(),
    );
    let address = address.map_err(|error| diagnostics.push(error));
    let rate = rate.map_err(|error| diagnostics.push(error));
    let reads = reads(config, &mut diagnostics);
    diagnostics.extend(read::unknown(
        config,
        kind::NOUN,
        &["address", "rate"],
        &["read"],
    ));
    match (address, rate) {
        (Ok(address), Ok(rate)) if diagnostics.is_empty() => Ok(Config {
            address,
            rate,
            reads,
        }),
        _ => Err(diagnostics),
    }
}

/// Reads an endpoint URL: `opc.tcp://`, a host, and an optional port and path, as
/// open62541 reads it at the connect, with no NUL.
fn address(value: &Value) -> Result<CString, Diagnostic> {
    let refused = |message| {
        Diagnostic::new(
            BAD_ADDRESS,
            value.span,
            message,
            "Write an address such as \"opc.tcp://plc:4840\"".into(),
        )
    };
    let document::value::Kind::String(text) = &value.kind else {
        return Err(refused(format!(
            "an address is a string, not {}",
            value.kind.noun()
        )));
    };
    if !text.starts_with(SCHEME) {
        return Err(refused(format!(
            "the address {text:?} does not start with {SCHEME}"
        )));
    }
    if !client::endpoint(text) {
        return Err(refused(format!(
            "the address {text:?} has no host, or a port that is not 0 to 65535"
        )));
    }
    CString::new(text.as_bytes())
        .map_err(|_nul| refused("the address holds a NUL".into()))
}

/// Reads the `read` blocks of `config`, and adds each problem to `diagnostics`.
fn reads(config: &Document, diagnostics: &mut Vec<Diagnostic>) -> Vec<Read> {
    const OF: &str = "the `read` block";
    let blocks = config.blocks.iter();
    let blocks: Vec<&Block> =
        blocks.filter(|block| &*block.keyword == "read").collect();
    if blocks.is_empty() {
        diagnostics.push(Diagnostic::new(
            NO_READ,
            None,
            "the connector has no `read` block".into(),
            "Add a `read` block for each channel, such as `read \"plant.spike\" { \
             node_id = \"ns=3;s=SpikeData\" }`"
                .into(),
        ));
    }
    let mut channels: BTreeMap<Name, &Block> = BTreeMap::new();
    let mut reads = Vec::with_capacity(blocks.len());
    for block in blocks {
        let fix = "Name the channel of the block, such as `read \"plant.spike\"`";
        let channel = read::labels::<1>(block, fix.into())
            .and_then(|[label]| read::label(label))
            .map_err(|error| diagnostics.push(error));
        let node = read::required(
            &block.body,
            OF,
            block.keyword_span,
            "node_id",
            node,
            "Add a `node_id` attribute with the node of the channel, such as \
             \"ns=3;s=SpikeData\""
                .into(),
        );
        let node = node.map_err(|error| diagnostics.push(error));
        diagnostics.extend(read::unknown(&block.body, OF, &["node_id"], &[]));
        let Ok(channel) = channel else { continue };
        match channels.entry(channel) {
            Entry::Occupied(first) => {
                diagnostics.push(repeated(first.key(), block, first.get()));
            }
            Entry::Vacant(slot) => {
                if let Ok(node) = node {
                    reads.push(Read {
                        channel: slot.key().clone(),
                        node,
                    });
                }
                slot.insert(block);
            }
        }
    }
    reads
}

/// The `read` block of the node `tag` as the channel `channel`, as
/// [`Kind::block`](connector::kind::Kind::block) asks: `read "<channel>" { node_id =
/// "<tag>" }`.
pub(crate) fn block(tag: &str, channel: &Name) -> Result<Vec<Block>, Vec<Diagnostic>> {
    let value = Value {
        kind: document::value::Kind::String(tag.into()),
        span: None,
    };
    node(&value).map_err(|mut error| {
        error.message = format!("the tag {tag:?}: {}", error.message);
        vec![error]
    })?;
    Ok(vec![read_block(channel, value)])
}

/// `read "<channel>" { node_id = <node_id> }`.
fn read_block(channel: &Name, node_id: Value) -> Block {
    let node_id = Attribute {
        key: "node_id".into(),
        key_span: None,
        value: node_id,
    };
    Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![Label {
            text: channel.as_str().into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![node_id]).expect("invariant: one key"),
            blocks: Vec::new(),
        },
        span: None,
    }
}

/// Reads a node ID, such as `"ns=3;s=SpikeData"`.
fn node(value: &Value) -> Result<Node, Diagnostic> {
    let refused = |message| {
        Diagnostic::new(
            BAD_NODE_ID,
            value.span,
            message,
            "Write a node ID such as \"ns=3;s=SpikeData\" or \"ns=3;i=1001\"".into(),
        )
    };
    let document::value::Kind::String(text) = &value.kind else {
        return Err(refused(format!(
            "a node ID is a string, not {}",
            value.kind.noun()
        )));
    };
    text.parse()
        .map_err(|error: crate::node::Error| refused(error.to_string()))
}

/// The diagnostic of a `read` block of `channel` after the block `first` of it.
fn repeated(channel: &Name, block: &Block, first: &Block) -> Diagnostic {
    let label = |block: &Block| block.labels.first().and_then(|label| label.span);
    let mut diagnostic = Diagnostic::new(
        REPEATED_CHANNEL,
        label(block),
        format!("the connector reads `{channel}` in another `read` block"),
        "Remove one of the two `read` blocks".into(),
    );
    diagnostic.notes.extend(label(first).map(|span| Note {
        span,
        text: format!("the first `read` block of `{channel}`"),
    }));
    diagnostic
}
