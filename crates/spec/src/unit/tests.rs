use proptest::prelude::*;

use super::*;

fn unit(text: &str) -> Unit {
    Unit::new(text).unwrap()
}

#[test]
fn gives_the_fix_of_each_error() {
    for (error, fix) in [
        (Error::Empty, "Write a unit such as kPa, or remove the unit"),
        (Error::Long { len: 33 }, "Use a shorter unit, such as kPa"),
        (
            Error::Character { at: 1, found: ' ' },
            "Use only printable ASCII characters with no space, such as m/s2",
        ),
    ] {
        assert_eq!(error.fix(), fix, "{error:?}");
    }
}

#[test]
fn refuses_an_empty_unit() {
    assert_eq!(Unit::new(""), Err(Error::Empty));
    assert_eq!(Error::Empty.to_string(), "a unit is empty");
}

#[test]
fn refuses_a_unit_longer_than_the_limit() {
    let longest = "a".repeat(32);
    assert_eq!(unit(&longest).as_str(), longest);
    assert_eq!(Unit::new(&"a".repeat(33)), Err(Error::Long { len: 33 }));
    assert_eq!(
        Error::Long { len: 33 }.to_string(),
        "a unit has 33 bytes, more than 32"
    );
}

#[test]
fn refuses_a_character_that_is_not_printable_ascii() {
    for (text, at, found) in [
        ("k Pa", 1, ' '),
        ("kPa\n", 3, '\n'),
        ("kPa\u{200B}", 3, '\u{200B}'),
        ("k\u{200D}Pa", 1, '\u{200D}'),
        ("k\u{AD}Pa", 1, '\u{AD}'),
        ("k\u{202E}aP", 1, '\u{202E}'),
        ("\u{FEFF}kPa", 0, '\u{FEFF}'),
        ("°C", 0, '°'),
    ] {
        assert_eq!(
            Unit::new(text),
            Err(Error::Character { at, found }),
            "{text}"
        );
    }
    assert_eq!(
        Error::Character { at: 1, found: ' ' }.to_string(),
        "a unit has the character ' ' at byte 1, which is not printable ASCII"
    );
}

#[test]
fn gives_the_code_of_a_unit_in_the_table() {
    assert_eq!(unit("kPa").code(), Some("KPA"));
    assert_eq!(unit("degC").code(), Some("CEL"));
    assert_eq!(unit("mPa").code(), None);
    assert_eq!(unit("kpa").code(), None);
    assert_eq!(unit("furlong").code(), None);
    assert_eq!(unit("furlong").as_str(), "furlong");
}

#[test]
fn holds_a_sorted_table_of_units_that_read() {
    assert!(TABLE.is_sorted_by(|a, b| a.0 < b.0));
    for (text, code) in TABLE {
        assert_eq!(unit(text).code(), Some(*code), "{text}");
    }
}

/// The lookup by code needs each code once. This states that invariant of the table in
/// one place, apart from what each code gives.
#[test]
fn holds_each_code_once_in_the_table() {
    let mut codes: Vec<_> = TABLE.iter().map(|(_, code)| code).collect();
    codes.sort_unstable();
    let len = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), len);
}

#[test]
fn spells_each_common_spelling_as_a_unit_in_the_table() {
    for (spelling, _) in SPELLINGS {
        let code = Unit::given(Some(spelling), None).and_then(|unit| unit.code());
        assert!(code.is_some(), "{spelling}");
    }
}

fn given(text: Option<&str>, code: Option<&str>) -> Option<String> {
    Unit::given(text, code).map(|unit| unit.as_str().to_owned())
}

#[test]
fn gives_the_unit_of_a_code_in_the_table_whatever_the_text() {
    assert_eq!(given(Some("°K"), Some("CEL")).as_deref(), Some("degC"));
    assert_eq!(given(Some("kPa"), Some("BAR")).as_deref(), Some("bar"));
    assert_eq!(given(None, Some("KPA")).as_deref(), Some("kPa"));
    for (text, code) in TABLE {
        assert_eq!(
            given(Some(""), Some(code)).as_deref(),
            Some(*text),
            "{code}"
        );
    }
}

#[test]
fn reads_the_text_when_the_table_has_no_code() {
    assert_eq!(given(Some("rpm"), Some("XYZ")).as_deref(), Some("rpm"));
    assert_eq!(given(Some("rpm"), Some("cel")).as_deref(), Some("rpm"));
    assert_eq!(given(Some("rpm"), None).as_deref(), Some("rpm"));
}

#[test]
fn gives_the_ascii_form_of_a_common_spelling() {
    for (text, expected) in [
        ("°C", "degC"),
        ("°F", "degF"),
        ("\u{3A9}", "ohm"),
        ("\u{2126}", "ohm"),
        ("m³", "m3"),
        ("m/s²", "m/s2"),
        ("\u{B5}A", "uA"),
        ("\u{3BC}s", "us"),
        ("\u{B5}\u{3A9}", "uohm"),
        ("\u{3BC}°C", "udegC"),
    ] {
        assert_eq!(given(Some(text), None).as_deref(), Some(expected), "{text}");
    }
}

#[test]
fn gives_no_unit_for_a_text_that_no_step_reads() {
    for text in [
        "",
        "k Pa",
        "°K",
        "k\u{3A9}",
        "°C ",
        "\u{B5}",
        "\u{B5}\u{B5}A",
        "\u{B5}k Pa",
        "A\u{B5}",
        "\u{2126}m",
    ] {
        assert_eq!(given(Some(text), None), None, "{text:?}");
    }
    assert_eq!(given(None, None), None);
    assert_eq!(given(None, Some("XYZ")), None);
    let longest = format!("\u{B5}{}", "a".repeat(31));
    assert_eq!(
        given(Some(&longest), None),
        Some(format!("u{}", "a".repeat(31)))
    );
    let long = format!("\u{B5}{}", "a".repeat(32));
    assert_eq!(given(Some(&long), None), None);
}

proptest! {
    #[test]
    fn gives_each_text_that_reads_as_it_is(text in "[!-~]{1,32}") {
        prop_assert_eq!(given(Some(&text), None), Some(text));
    }

    #[test]
    fn reads_exactly_the_short_printable_ascii_texts(
        text in "[\\x00-\\x7f°\u{200B}]{0,40}",
    ) {
        let valid = (1..=32).contains(&text.len())
            && text.bytes().all(|b| (0x21..=0x7e).contains(&b));
        prop_assert_eq!(Unit::new(&text).is_ok(), valid);
        if valid {
            prop_assert_eq!(unit(&text).as_str().to_owned(), text);
        }
    }
}
