use proptest::prelude::*;

use super::{Error, Key, NAME, Node};

fn node() -> impl Strategy<Value = Node> {
    let key = prop_oneof![
        any::<u32>().prop_map(Key::Number),
        ".{1,64}".prop_map(Key::Name),
    ];
    (any::<u16>(), key).prop_map(|(namespace, key)| Node { namespace, key })
}

proptest! {
    #[test]
    fn a_node_reads_back_from_its_text(node in node()) {
        prop_assert_eq!(node.to_string().parse::<Node>(), Ok(node));
    }
}

#[test]
fn a_text_gives_its_node() {
    let cases = [
        ("ns=3;s=SpikeData", 3, Key::Name("SpikeData".to_owned())),
        ("ns=1;i=1000", 1, Key::Number(1000)),
        ("i=2258", 0, Key::Number(2258)),
        ("s=a;b=c", 0, Key::Name("a;b=c".to_owned())),
        ("ns=65535;i=4294967295", u16::MAX, Key::Number(u32::MAX)),
        ("ns=007;i=08", 7, Key::Number(8)),
    ];
    for (text, namespace, key) in cases {
        assert_eq!(text.parse(), Ok(Node { namespace, key }), "{text}");
    }
}

#[test]
fn a_namespace_by_uri_is_refused() {
    let read = "nsu=urn:plant;s=SpikeData".parse::<Node>();
    assert_eq!(read, Err(Error::Uri));
    assert_eq!(
        Error::Uri.to_string(),
        "a namespace by URI (nsu=) is not supported: give its index (ns=)"
    );
}

#[test]
fn a_guid_or_opaque_key_is_refused() {
    for text in [
        "ns=1;g=09087e75-8e5e-499b-954f-f2a9603db28a",
        "b=M/RbKBsRVkePCePcx24oRA",
    ] {
        assert_eq!(text.parse::<Node>(), Err(Error::Form), "{text}");
    }
    assert_eq!(
        Error::Form.to_string(),
        "a GUID (g=) or opaque (b=) key is not supported: give a number (i=) or a \
         name (s=)"
    );
}

#[test]
fn another_text_is_refused() {
    let texts = [
        "",
        "SpikeData",
        "ns=3",
        "ns=3;",
        "ns=;i=1",
        "ns=65536;i=1",
        "ns=+3;i=1",
        "ns=-1;i=1",
        "ns=3;i=",
        "ns=3;i=4294967296",
        "ns=3;i=+1",
        "ns=3;i=1x",
        "ns=3;s=",
        "ns=3;ns=4;i=1",
        "NS=3;i=1",
        " i=1",
    ];
    for text in texts {
        assert_eq!(text.parse::<Node>(), Err(Error::Syntax), "{text:?}");
    }
    assert_eq!(
        Error::Syntax.to_string(),
        "a node ID is ns=<index>;i=<number> or ns=<index>;s=<name>, with an index to \
         65535, a number to 4294967295, and a name of 1 to 4096 bytes"
    );
}

#[test]
fn a_name_is_at_most_4096_bytes() {
    let name = "ü".repeat(NAME / 2);
    let text = format!("ns=1;s={name}");
    assert_eq!(
        text.parse(),
        Ok(Node {
            namespace: 1,
            key: Key::Name(name)
        })
    );
    let text = format!("ns=1;s={}x", "ü".repeat(NAME / 2));
    assert_eq!(text.parse::<Node>(), Err(Error::Syntax));
}

#[test]
fn namespace_0_has_no_index_in_its_text() {
    let node = Node {
        namespace: 0,
        key: Key::Number(2258),
    };
    assert_eq!(node.to_string(), "i=2258");
    let node = Node {
        namespace: 3,
        key: Key::Name("SpikeData".to_owned()),
    };
    assert_eq!(node.to_string(), "ns=3;s=SpikeData");
}
