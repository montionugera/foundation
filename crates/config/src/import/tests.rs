use std::collections::BTreeMap;

use ::connector::browse::Facts;
use ::connector::kind::Table;
use document::diagnostic::{Code, Diagnostic};
use document::{Block, Document};
use proptest::prelude::*;
use spec::channel::{Data, Kind};
use spec::data_type::DataType;
use spec::unit::Unit;
use types::name::Name;
use types::sample::{Scalar, Sides, Type};

use super::{channel, index};
use crate::{Definition, Entry};

fn name(text: &str) -> Name {
    text.parse().unwrap()
}

fn facts(data_type: Option<Type>, unit: Option<&str>) -> Facts {
    Facts {
        data_type,
        unit: unit.map(Into::into),
        ..Facts::default()
    }
}

/// What `config::check` gives for an index channel `edge.time` and the channel that
/// `channel` writes for `edge.temp` with `facts`.
fn check(facts: &Facts) -> Result<BTreeMap<Name, Entry>, Vec<Diagnostic>> {
    let time = name("edge.time");
    let blocks: Vec<Block> =
        vec![index(&time), channel(&name("edge.temp"), &time, facts)];
    let document = Document {
        blocks,
        ..Document::default()
    };
    crate::check(&[document], &Table::new())
}

/// The entries of `edge.time` and of `edge.temp` as `data`.
fn entries(data: Data<Name>) -> BTreeMap<Name, Entry> {
    let index = Kind::Index {
        error: None,
        control: None,
    };
    [("edge.time", index), ("edge.temp", Kind::Data(data))]
        .into_iter()
        .map(|(key, kind)| {
            let entry = Entry {
                definition: Definition::Channel(kind),
                label_span: None,
            };
            (name(key), entry)
        })
        .collect()
}

fn data(data_type: Type, unit: Option<&str>) -> Data<Name> {
    let unit = unit.map(|unit| Unit::new(unit).unwrap());
    Data::new(name("edge.time"), None, DataType::Sample(data_type), unit).unwrap()
}

fn unfilled(what: &str) -> Diagnostic {
    Diagnostic::new(
        Code::new("config.unfilled"),
        None,
        format!("the import found no value for {what}"),
        "Write the value in place of `fill`".into(),
    )
}

/// Each scalar, and each other shape of sample type.
fn types() -> Vec<Type> {
    let scalars = [
        Scalar::Bool,
        Scalar::I8,
        Scalar::I16,
        Scalar::I32,
        Scalar::I64,
        Scalar::U8,
        Scalar::U16,
        Scalar::U32,
        Scalar::U64,
        Scalar::F32,
        Scalar::F64,
        Scalar::Stamp,
        Scalar::Span,
        Scalar::Uuid,
    ];
    let shapes = [
        Type::Array {
            element: Scalar::F32,
            len: 4,
        },
        Type::List {
            element: Scalar::I16,
            max: 8,
        },
        Type::Matrix {
            element: Scalar::U8,
            sides: Sides {
                rows: 2,
                columns: 3,
            },
        },
        Type::Array {
            element: Scalar::Bool,
            len: 2,
        },
        Type::String,
        Type::Bytes,
    ];
    scalars
        .into_iter()
        .map(Type::Scalar)
        .chain(shapes)
        .collect()
}

#[test]
fn writes_a_channel_that_checks_for_each_sample_type() {
    for data_type in types() {
        assert_eq!(
            check(&facts(Some(data_type), None)),
            Ok(entries(data(data_type, None))),
            "{data_type}"
        );
    }
}

#[test]
fn writes_the_unit_on_each_sample_type_that_holds_a_number() {
    for data_type in types() {
        let expected = Data::new(
            name("edge.time"),
            None,
            DataType::Sample(data_type),
            Some(Unit::new("degC").unwrap()),
        )
        .unwrap_or_else(|_| data(data_type, None));
        assert_eq!(
            check(&facts(Some(data_type), Some("degC"))),
            Ok(entries(expected)),
            "{data_type}"
        );
    }
}

#[test]
fn leaves_out_a_unit_on_a_string() {
    assert_eq!(
        check(&facts(Some(Type::String), Some("degC"))),
        Ok(entries(data(Type::String, None)))
    );
}

#[test]
fn leaves_out_an_empty_unit() {
    let f64 = Type::Scalar(Scalar::F64);
    assert_eq!(
        check(&facts(Some(f64), Some(""))),
        Ok(entries(data(f64, None)))
    );
}

#[test]
fn fills_a_unit_that_does_not_read_with_the_text_of_the_device() {
    assert_eq!(
        check(&facts(Some(Type::Scalar(Scalar::F64)), Some("°K"))),
        Err(vec![unfilled("the unit, which the device gives as \"°K\"")])
    );
}

#[test]
fn writes_the_unit_that_spec_gives_for_the_text_and_the_code() {
    let f64 = Type::Scalar(Scalar::F64);
    let with_code = |unit: Option<&str>| Facts {
        unit_code: Some("CEL".into()),
        ..facts(Some(f64), unit)
    };
    for facts in [
        facts(Some(f64), Some("°C")),
        with_code(Some("°K")),
        with_code(None),
    ] {
        assert_eq!(
            check(&facts),
            Ok(entries(data(f64, Some("degC")))),
            "{facts:?}"
        );
    }
}

fn coded(unit: Option<&str>, code: &str) -> Facts {
    Facts {
        unit_code: Some(code.into()),
        ..facts(Some(Type::Scalar(Scalar::F64)), unit)
    }
}

#[test]
fn fills_a_code_that_does_not_read_with_no_text() {
    for unit in [None, Some("")] {
        assert_eq!(
            check(&coded(unit, "XYZ")),
            Err(vec![unfilled(
                "the unit, which the device gives as the code \"XYZ\""
            )]),
            "{unit:?}"
        );
    }
}

#[test]
fn fills_a_text_and_a_code_that_do_not_read_with_both() {
    assert_eq!(
        check(&coded(Some("°K"), "XYZ")),
        Err(vec![unfilled(
            "the unit, which the device gives as \"°K\" with the code \"XYZ\""
        )])
    );
}

#[test]
fn writes_a_text_that_reads_and_drops_a_code_that_does_not() {
    let f64 = Type::Scalar(Scalar::F64);
    assert_eq!(
        check(&coded(Some("°C"), "XYZ")),
        Ok(entries(data(f64, Some("degC"))))
    );
}

#[test]
fn takes_an_empty_code_as_none() {
    let f64 = Type::Scalar(Scalar::F64);
    assert_eq!(check(&coded(None, "")), Ok(entries(data(f64, None))));
    assert_eq!(
        check(&coded(Some("°K"), "")),
        Err(vec![unfilled("the unit, which the device gives as \"°K\"")])
    );
}

#[test]
fn fills_a_missing_sample_type_and_keeps_the_unit() {
    assert_eq!(
        check(&facts(None, None)),
        Err(vec![unfilled("the sample type")])
    );
    assert_eq!(
        check(&facts(None, Some("°K"))),
        Err(vec![
            unfilled("the sample type"),
            unfilled("the unit, which the device gives as \"°K\""),
        ])
    );
    let block = channel(
        &name("edge.temp"),
        &name("edge.time"),
        &facts(None, Some("degC")),
    );
    assert_eq!(
        block
            .body
            .attributes
            .get("unit")
            .map(|attribute| &attribute.value.kind),
        Some(&document::value::Kind::String("degC".into()))
    );
}

proptest! {
    #[test]
    fn writes_each_unit_that_reads_and_fills_each_other(unit in any::<String>()) {
        let f64 = Type::Scalar(Scalar::F64);
        let expected = if unit.is_empty() {
            Ok(entries(data(f64, None)))
        } else if Unit::new(&unit).is_ok() {
            Ok(entries(data(f64, Some(&unit))))
        } else if let Some(given) = Unit::given(Some(&unit), None) {
            Ok(entries(data(f64, Some(given.as_str()))))
        } else {
            Err(vec![unfilled(&format!("the unit, which the device gives as {unit:?}"))])
        };
        prop_assert_eq!(check(&facts(Some(f64), Some(&unit))), expected);
    }
}
