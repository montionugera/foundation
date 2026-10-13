#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use proptest::prelude::*;

use super::*;

/// One parsed line: measurement, tags, fields, and time.
type Parsed = (String, Vec<(String, String)>, Vec<(String, Value)>, i64);

/// Splits `text` at each `split` that no backslash escapes, and keeps the escapes.
fn split(text: &str, split: char) -> Vec<&str> {
    let (mut parts, mut from, mut escaped) = (Vec::new(), 0, false);
    for (at, c) in text.char_indices() {
        if !escaped && c == split {
            parts.push(text.get(from..at).unwrap());
            from = at + 1;
        }
        escaped = !escaped && c == '\\';
    }
    parts.push(text.get(from..).unwrap());
    parts
}

/// Drops the backslash of each escape.
fn unescape(text: &str) -> String {
    let (mut out, mut chars) = (String::new(), text.chars());
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next().unwrap() } else { c });
    }
    out
}

/// Splits `pair` at its first `=` that no backslash escapes.
fn pair(pair: &str) -> (String, &str) {
    let parts = split(pair, '=');
    let key = parts.first().unwrap();
    (unescape(key), pair.get(key.len() + 1..).unwrap())
}

fn value(text: &str) -> Value {
    match text {
        "t" => Value::Boolean(true),
        "f" => Value::Boolean(false),
        _ if text.ends_with('i') => {
            Value::Integer(text.trim_end_matches('i').parse().unwrap())
        }
        _ => Value::Float(float(text.parse().unwrap())),
    }
}

/// Parses one line of line protocol, as InfluxDB reads it.
fn parse(line: &str) -> Parsed {
    let line = line.strip_suffix('\n').expect("a line ends with a newline");
    let line = line.trim_start_matches([' ', '\t', '\0']);
    assert!(!line.starts_with('#'), "InfluxDB drops a comment: {line:?}");
    let [head, fields, time] = split(line, ' ')[..] else {
        panic!("not three parts: {line:?}");
    };
    let head = split(head, ',');
    let tags = head.iter().skip(1).map(|tag| {
        let (key, value) = pair(tag);
        (key, unescape(value))
    });
    let fields = fields.trim_start_matches([' ', '\t', '\0']);
    let fields = split(fields, ',').into_iter().map(|field| {
        let (key, text) = pair(field);
        (key, value(text))
    });
    (
        unescape(head.first().unwrap()),
        tags.collect(),
        fields.collect(),
        time.parse().unwrap(),
    )
}

fn float(value: f64) -> Float {
    Float::new(value).unwrap()
}

fn text(out: &[u8]) -> &str {
    std::str::from_utf8(out).unwrap()
}

/// A measurement and the field of each of its keys.
type Built = (Measurement, Vec<Field>);

/// The measurement `name` with `tags`, and the field of each key in `fields`.
fn build(name: &str, tags: &[(&str, &str)], fields: &[&str]) -> Result<Built, Error> {
    let measurement = Measurement::new(name, tags)?;
    let fields = fields.iter().map(|key| measurement.field(key));
    let fields = fields.collect::<Result<_, _>>()?;
    Ok((measurement, fields))
}

/// Appends the line of `built` at `time`, with each field that has a value in
/// `values`.
fn write(
    (measurement, fields): &Built,
    out: &mut Vec<u8>,
    values: &[Option<Value>],
    time: Stamp,
) {
    assert_eq!(values.len(), fields.len(), "one value for each field");
    let values = fields.iter().zip(values);
    let values = values.filter_map(|(field, value)| Some((field, (*value)?)));
    measurement.line(out, values, time);
}

#[test]
fn writes_one_known_line() {
    let measurement = build(
        "plant",
        &[("site", "west"), ("line", "a b")],
        &["temp", "n", "open"],
    )
    .unwrap();
    let mut out = b"before\n".to_vec();
    let values = [
        Some(Value::Float(float(21.5))),
        Some(Value::Integer(-3)),
        Some(Value::Boolean(true)),
    ];
    write(&measurement, &mut out, &values, Stamp::from_nanos(1_000));
    assert_eq!(
        text(&out),
        "before\nplant,line=a\\ b,site=west temp=2.15e1,n=-3i,open=t 1000\n"
    );
}

#[test]
fn escapes_as_the_specification_example() {
    let measurement =
        build("my Measurement", &[("tag Key1", "tag Value1")], &["f"]).unwrap();
    let mut out = Vec::new();
    write(
        &measurement,
        &mut out,
        &[Some(Value::Integer(1))],
        Stamp::EPOCH,
    );
    assert_eq!(
        text(&out),
        "my\\ Measurement,tag\\ Key1=tag\\ Value1 f=1i 0\n"
    );
}

#[test]
fn escapes_what_line_protocol_reads_as_syntax() {
    let measurement = build("a,b c=d", &[("k,=  ", "v,= ")], &["f,= "]).unwrap();
    let mut out = Vec::new();
    write(
        &measurement,
        &mut out,
        &[Some(Value::Boolean(false))],
        Stamp::EPOCH,
    );
    assert_eq!(
        text(&out),
        "a\\,b\\ c=d,k\\,\\=\\ \\ =v\\,\\=\\  f\\,\\=\\ =f 0\n"
    );
}

#[test]
fn leaves_out_a_field_with_no_value() {
    let measurement = build("m", &[], &["a", "b"]).unwrap();
    let mut out = b"x".to_vec();
    write(&measurement, &mut out, &[None, None], Stamp::EPOCH);
    assert_eq!(out, b"x", "no value, so nothing is written");
    write(
        &measurement,
        &mut out,
        &[None, Some(Value::Integer(1))],
        Stamp::EPOCH,
    );
    assert_eq!(text(&out), "xm b=1i 0\n");
}

#[test]
fn writes_each_field_in_the_order_given_and_twice() {
    let measurement = Measurement::new("m", &[]).unwrap();
    let a = measurement.field("a").unwrap();
    let c = measurement.field("c").unwrap();
    let mut out = b"x".to_vec();
    measurement.line(&mut out, [], Stamp::EPOCH);
    assert_eq!(out, b"x", "no value, so nothing is written");
    let values = [
        (&c, Value::Integer(3)),
        (&a, Value::Integer(1)),
        (&c, Value::Integer(4)),
    ];
    measurement.line(&mut out, values, Stamp::from_nanos(5));
    assert_eq!(text(&out), "xm c=3i,a=1i,c=4i 5\n");
}

#[test]
fn refuses_a_field_key_as_new_refuses_a_key() {
    let measurement = Measurement::new("m", &[("k", "v")]).unwrap();
    let character = Error::Character {
        part: Part::FieldKey,
        text: "a\tb".into(),
        character: '\t',
    };
    let cases = [
        ("", Error::Empty(Part::FieldKey)),
        ("_f", Error::Reserved("_f".into())),
        ("time", Error::Reserved("time".into())),
        ("a\tb", character),
        ("k", Error::Duplicate("k".into())),
    ];
    for (key, error) in cases {
        assert_eq!(measurement.field(key), Err(error));
    }
    let field = measurement.field("b c").unwrap();
    let mut out = Vec::new();
    measurement.line(&mut out, [(&field, Value::Integer(1))], Stamp::EPOCH);
    assert_eq!(text(&out), "m,k=v b\\ c=1i 0\n");
}

#[test]
fn refuses_a_float_that_is_not_finite() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(Float::new(bad), None);
    }
    assert_eq!(Float::new(-0.5).map(Float::get), Some(-0.5));
}

fn refuses(cases: &[(Result<Built, Error>, Error, &str)]) {
    for (got, error, message) in cases {
        assert_eq!(got, &Err(error.clone()));
        assert_eq!(error.to_string(), *message);
    }
}

#[test]
fn refuses_a_measurement_line_protocol_cannot_carry() {
    refuses(&[
        (
            build("", &[], &["f"]),
            Error::Empty(Part::Measurement),
            "the measurement name is empty",
        ),
        (
            build("m", &[("", "v")], &["f"]),
            Error::Empty(Part::TagKey),
            "a tag key is empty",
        ),
        (
            build("m", &[("k", "")], &["f"]),
            Error::Empty(Part::TagValue("k".into())),
            "the value of the tag \"k\" is empty",
        ),
        (
            build("m", &[], &[""]),
            Error::Empty(Part::FieldKey),
            "a field key is empty",
        ),
        (
            build("a\\b", &[], &["f"]),
            Error::Character {
                part: Part::Measurement,
                text: "a\\b".into(),
                character: '\\',
            },
            "the measurement name \"a\\\\b\" holds '\\\\', which a line cannot hold",
        ),
        (
            build("m", &[("k", "a\nb")], &["f"]),
            Error::Character {
                part: Part::TagValue("k".into()),
                text: "a\nb".into(),
                character: '\n',
            },
            "the value \"a\\nb\" of the tag \"k\" holds '\\n', \
             which a line cannot hold",
        ),
        (
            build("m", &[], &["f\r"]),
            Error::Character {
                part: Part::FieldKey,
                text: "f\r".into(),
                character: '\r',
            },
            "the field key \"f\\r\" holds '\\r', which a line cannot hold",
        ),
    ]);
}

#[test]
fn refuses_a_tab_or_nul_in_any_part() {
    refuses(&[
        (
            build("\t#m", &[], &["f"]),
            Error::Character {
                part: Part::Measurement,
                text: "\t#m".into(),
                character: '\t',
            },
            "the measurement name \"\\t#m\" holds '\\t', which a line cannot hold",
        ),
        (
            build("m\0x", &[], &["f"]),
            Error::Character {
                part: Part::Measurement,
                text: "m\0x".into(),
                character: '\0',
            },
            "the measurement name \"m\\0x\" holds '\\0', which a line cannot hold",
        ),
        (
            build("m", &[("\tk", "v")], &["f"]),
            Error::Character {
                part: Part::TagKey,
                text: "\tk".into(),
                character: '\t',
            },
            "the tag key \"\\tk\" holds '\\t', which a line cannot hold",
        ),
        (
            build("m", &[("k", "a\tb")], &["f"]),
            Error::Character {
                part: Part::TagValue("k".into()),
                text: "a\tb".into(),
                character: '\t',
            },
            "the value \"a\\tb\" of the tag \"k\" holds '\\t', \
             which a line cannot hold",
        ),
        (
            build("m", &[("k2", "a\tb"), ("k1", "a\tb")], &["f"]),
            Error::Character {
                part: Part::TagValue("k1".into()),
                text: "a\tb".into(),
                character: '\t',
            },
            "the value \"a\\tb\" of the tag \"k1\" holds '\\t', \
             which a line cannot hold",
        ),
        (
            build("m", &[], &["f", "\tf"]),
            Error::Character {
                part: Part::FieldKey,
                text: "\tf".into(),
                character: '\t',
            },
            "the field key \"\\tf\" holds '\\t', which a line cannot hold",
        ),
        (
            build("m", &[], &["f\0g"]),
            Error::Character {
                part: Part::FieldKey,
                text: "f\0g".into(),
                character: '\0',
            },
            "the field key \"f\\0g\" holds '\\0', which a line cannot hold",
        ),
    ]);
}

/// Characters that InfluxDB 1 and 2 with `validate-keys` drop: each is outside the
/// general categories L, M, N, P, and S, or is U+FFFD.
const DROPPED: [char; 14] = [
    '\u{1}',      // Cc
    '\u{7f}',     // Cc
    '\u{85}',     // Cc
    '\u{a0}',     // Zs
    '\u{3000}',   // Zs
    '\u{2028}',   // Zl
    '\u{2029}',   // Zp
    '\u{200b}',   // Cf
    '\u{e000}',   // Co
    '\u{378}',    // Cn
    '\u{e0001}',  // Cf
    '\u{f0000}',  // Co
    '\u{10ffff}', // Cn
    '\u{fffd}',   // So, which InfluxDB refuses by name
];

#[test]
fn refuses_a_character_influxdb_drops() {
    refuses(&[
        (
            build("m\u{1}x", &[], &["f"]),
            Error::Character {
                part: Part::Measurement,
                text: "m\u{1}x".into(),
                character: '\u{1}',
            },
            "the measurement name \"m\\u{1}x\" holds '\\u{1}', \
             which a line cannot hold",
        ),
        (
            build("m", &[("k\u{a0}", "v")], &["f"]),
            Error::Character {
                part: Part::TagKey,
                text: "k\u{a0}".into(),
                character: '\u{a0}',
            },
            "the tag key \"k\\u{a0}\" holds '\\u{a0}', which a line cannot hold",
        ),
        (
            build("m", &[("k", "a\u{fffd}b")], &["f"]),
            Error::Character {
                part: Part::TagValue("k".into()),
                text: "a\u{fffd}b".into(),
                character: '\u{fffd}',
            },
            "the value \"a\u{fffd}b\" of the tag \"k\" holds '\u{fffd}', \
             which a line cannot hold",
        ),
        (
            build("m", &[("k", "a\u{e0001}")], &["f"]),
            Error::Character {
                part: Part::TagValue("k".into()),
                text: "a\u{e0001}".into(),
                character: '\u{e0001}',
            },
            "the value \"a\\u{e0001}\" of the tag \"k\" holds '\\u{e0001}', \
             which a line cannot hold",
        ),
        (
            build("m", &[], &["f\u{378}"]),
            Error::Character {
                part: Part::FieldKey,
                text: "f\u{378}".into(),
                character: '\u{378}',
            },
            "the field key \"f\\u{378}\" holds '\\u{378}', which a line cannot hold",
        ),
    ]);
}

#[test]
fn accepts_each_general_category_influxdb_prints() {
    let printed = [
        'Z', 'é', 'ǅ', 'ʰ', '中', // Lu, Ll, Lt, Lm, Lo
        '\u{301}', '\u{903}', '\u{20dd}', // Mn, Mc, Me
        '7', 'Ⅻ', '½', // Nd, Nl, No
        '_', '-', '(', ')', '«', '»', '!', ',', '=', // Pc, Pd, Ps, Pe, Pi, Pf, Po
        '+', '$', '^', '°', '😀', // Sm, Sc, Sk, So, So outside the BMP
        ' ',  // U+0020, the one Zs that InfluxDB prints
    ];
    for c in printed {
        let text = format!("a{c}");
        let t = text.as_str();
        for got in [
            build(t, &[], &["f"]),
            build("m", &[(t, "v")], &["f"]),
            build("m", &[("k", t)], &["f"]),
            build("m", &[], &[t]),
        ] {
            assert!(got.is_ok(), "{c:?}: {got:?}");
        }
    }
}

#[test]
fn reads_general_categories_of_unicode_17() {
    assert_eq!(unicode_properties::UNICODE_VERSION, (17, 0, 0));
}

#[test]
fn names_the_first_refused_character() {
    refuses(&[(
        build("m", &[], &["a\0b\tc"]),
        Error::Character {
            part: Part::FieldKey,
            text: "a\0b\tc".into(),
            character: '\0',
        },
        "the field key \"a\\0b\\tc\" holds '\\0', which a line cannot hold",
    )]);
}

#[test]
fn refuses_a_name_influxdb_keeps_or_repeats() {
    refuses(&[
        (
            build("_m", &[], &["f"]),
            Error::Reserved("_m".into()),
            "InfluxDB keeps the name \"_m\" for itself",
        ),
        (
            build("m", &[("_k", "v")], &["f"]),
            Error::Reserved("_k".into()),
            "InfluxDB keeps the name \"_k\" for itself",
        ),
        (
            build("m", &[], &["_f"]),
            Error::Reserved("_f".into()),
            "InfluxDB keeps the name \"_f\" for itself",
        ),
        (
            build("m", &[("time", "v")], &["f"]),
            Error::Reserved("time".into()),
            "InfluxDB keeps the name \"time\" for itself",
        ),
        (
            build("m", &[], &["time"]),
            Error::Reserved("time".into()),
            "InfluxDB keeps the name \"time\" for itself",
        ),
        (
            build("#m", &[], &["f"]),
            Error::Comment("#m".into()),
            "the name \"#m\" starts with '#', which makes each line a comment",
        ),
        (
            build("m", &[("k", "a"), ("k", "b")], &["f"]),
            Error::Duplicate("k".into()),
            "the key \"k\" comes more than once",
        ),
        (
            build("m", &[("k", "a")], &["f", "k"]),
            Error::Duplicate("k".into()),
            "the key \"k\" comes more than once",
        ),
    ]);
}

#[test]
fn allows_what_is_reserved_only_elsewhere() {
    let measurement = build("time", &[("k", "_v"), ("t", "#")], &["#f"]).unwrap();
    let mut out = Vec::new();
    write(
        &measurement,
        &mut out,
        &[Some(Value::Integer(0))],
        Stamp::EPOCH,
    );
    assert_eq!(text(&out), "time,k=_v,t=# #f=0i 0\n");
}

/// A name, key, or tag value that a line accepts in each part.
pub(crate) fn name() -> impl Strategy<Value = String> {
    concat!(
        r"[[\pL\pM\pN\pP\pS ]--[_#\\\x{fffd}]]",
        r"[[\pL\pM\pN\pP\pS ]--[\\\x{fffd}]]{0,8}",
    )
    .prop_filter("not time", |name| name != "time")
}

fn field_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<f64>()
            .prop_filter_map("finite", Float::new)
            .prop_map(Value::Float),
        any::<i64>().prop_map(Value::Integer),
        any::<bool>().prop_map(Value::Boolean),
    ]
}

proptest! {
    #[test]
    fn each_line_parses_back(
        measurement in name(),
        keys in proptest::collection::btree_map(
            name(),
            (any::<bool>(), name(), field_value()),
            1..8,
        ),
        time in any::<i64>(),
    ) {
        let mut tags = Vec::new();
        let mut fields = Vec::new();
        let mut keys_of_fields = Vec::new();
        for (key, (tag, value, field)) in &keys {
            if *tag {
                tags.push((key.clone(), value.clone()));
            } else {
                keys_of_fields.push(key.as_str());
                fields.push((key.clone(), *field));
            }
        }
        prop_assume!(!fields.is_empty());
        let borrowed: Vec<(&str, &str)> =
            tags.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let line = build(&measurement, &borrowed, &keys_of_fields).unwrap();
        let values: Vec<Option<Value>> = fields.iter().map(|&(_, v)| Some(v)).collect();
        let mut out = Vec::new();
        write(&line, &mut out, &values, Stamp::from_nanos(time));
        prop_assert_eq!(parse(text(&out)), (measurement, tags, fields, time));
    }

    #[test]
    fn refuses_a_refused_character_anywhere(
        name in name(),
        at in any::<proptest::sample::Index>(),
        character in proptest::sample::select(
            [['\\', '\n', '\r', '\t', '\0'].as_slice(), &DROPPED].concat()
        ),
        part in 0..4_usize,
    ) {
        let mut chars: Vec<char> = name.chars().collect();
        chars.insert(at.index(chars.len() + 1), character);
        let text: String = chars.into_iter().collect();
        let n = text.as_str();
        let (got, part) = match part {
            0 => (build(n, &[("k", "v")], &["f"]), Part::Measurement),
            1 => (build("m", &[(n, "v")], &["f"]), Part::TagKey),
            2 => (
                build("m", &[("k", n)], &["f"]),
                Part::TagValue("k".into()),
            ),
            _ => (build("m", &[("k", "v")], &["f", n]), Part::FieldKey),
        };
        prop_assert_eq!(got, Err(Error::Character { part, text, character }));
    }
}
