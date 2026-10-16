use proptest::prelude::*;

use super::*;

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

fn path(text: &str) -> Path {
    text.parse().expect("a valid path")
}

/// The path of `names` below `start`, each name escaped.
fn below(start: &str, names: &[String]) -> Path {
    let escaped = names.iter().map(|name| {
        name.chars().fold(String::new(), |mut text, c| {
            if matches!(c, '/' | '*' | '\\') {
                text.push('\\');
            }
            text.push(c);
            text
        })
    });
    let all: Vec<_> = (!start.is_empty())
        .then(|| start.to_owned())
        .into_iter()
        .chain(escaped)
        .collect();
    path(&all.join("/"))
}

/// The names of `picks` with no given name, below the root, and no name taken.
fn defaults(connector: &str, picks: &[&str]) -> Vec<Option<Name>> {
    let paths: Vec<_> = picks.iter().map(|text| path(text)).collect();
    let picks: Vec<_> = paths.iter().map(|path| (path, None)).collect();
    names(&name(connector), &Path::root(), &picks, [])
}

fn some(texts: &[&str]) -> Vec<Option<Name>> {
    texts.iter().map(|text| Some(name(text))).collect()
}

#[test]
fn puts_an_underscore_in_place_of_each_character_that_a_name_does_not_allow() {
    let names = defaults("plc", &["Kühler", "a.b/@c/\\*d e/x-1_Y"]);
    assert_eq!(names, some(&["plc.K_hler", "plc.a_b._c._d_e.x-1_Y"]));
}

#[test]
fn gives_a_suffix_to_a_default_that_differs_only_in_case() {
    let names = defaults("plc", &["temp", "Temp", "TEMP"]);
    assert_eq!(names, some(&["plc.temp", "plc.Temp_2", "plc.TEMP_3"]));
}

#[test]
fn keeps_the_name_of_a_connector_with_many_segments() {
    let names = defaults("site_a.plc_7", &["Line1/Temp"]);
    assert_eq!(names, some(&["site_a.plc_7.Line1.Temp"]));
}

#[test]
fn takes_the_names_below_the_start() {
    let (start, point) = (path("Objects/Plant"), path("Objects/Plant/Line1/Temp"));
    let names = names(&name("plc"), &start, &[(&point, None)], []);
    assert_eq!(names, some(&["plc.Line1.Temp"]));
}

#[test]
fn gives_no_default_the_name_that_a_later_pick_gives() {
    let (x, y) = (path("x"), path("y"));
    let given = name("PLC.X");
    let names = names(
        &name("plc"),
        &Path::root(),
        &[(&x, None), (&y, Some(&given))],
        [],
    );
    assert_eq!(names, some(&["plc.x_2", "PLC.X"]));
}

#[test]
fn keeps_a_given_name_that_is_taken() {
    let x = path("x");
    let given = name("plc.status");
    let taken = name("plc.status");
    let names = names(&name("plc"), &Path::root(), &[(&x, Some(&given))], [&taken]);
    assert_eq!(names, some(&["plc.status"]));
}

#[test]
fn gives_a_suffix_to_a_default_that_is_taken() {
    let (status, x, x_2) = (path("status"), path("x"), path("x_2"));
    let taken = [name("plc.Status"), name("plc.x")];
    let picks = [(&status, None), (&x, None), (&x_2, None)];
    let names = names(&name("plc"), &Path::root(), &picks, &taken);
    assert_eq!(names, some(&["plc.status_2", "plc.x_2", "plc.x_2_2"]));
}

#[test]
fn leaves_out_a_default_longer_than_the_most_bytes_of_a_name() {
    let longest = "a".repeat(Name::MAX_BYTES - "plc.".len());
    let names = defaults("plc", &[&longest, &format!("{longest}b"), "c"]);
    assert_eq!(
        names,
        vec![
            Some(name(&format!("plc.{longest}"))),
            None,
            Some(name("plc.c"))
        ]
    );
}

#[test]
fn leaves_out_a_default_whose_suffix_makes_it_too_long() {
    let longest = "a".repeat(Name::MAX_BYTES - "plc.".len());
    let names = defaults("plc", &[&longest, &longest.to_uppercase()]);
    assert_eq!(names, vec![Some(name(&format!("plc.{longest}"))), None]);
}

#[test]
#[should_panic(expected = r#"the path "Other/x" is not below the start "Objects""#)]
fn panics_on_a_path_that_is_not_below_the_start() {
    let (start, point) = (path("Objects"), path("Other/x"));
    drop(names(&name("plc"), &start, &[(&point, None)], []));
}

#[test]
#[should_panic(expected = r#"the path "Objects" is not below the start "Objects""#)]
fn panics_on_a_path_that_is_the_start() {
    let start = path("Objects");
    drop(names(&name("plc"), &start, &[(&start, None)], []));
}

#[test]
fn fills_a_fact_with_a_call_of_one_string() {
    let what = Value {
        kind: Kind::String("the sample type".into()),
        span: None,
    };
    let expected = Kind::Call(Call {
        function: "fill".into(),
        function_span: None,
        arguments: vec![what],
    });
    assert_eq!(fill("the sample type").kind, expected);
}

/// A name of a device, with characters that a name allows and does not allow.
fn device_name() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop::sample::select(vec!['a', 'A', '_', '2', '.', 'ü', '*']),
        1..4,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

/// A name under `plc` that a default of [`device_name`] can be.
fn channel_name() -> impl Strategy<Value = Name> {
    proptest::collection::vec(
        prop::sample::select(vec!["a", "A", "_", "2", "a_2", "A_3"]),
        1..3,
    )
    .prop_map(|parts| name(&format!("plc.{}", parts.concat())))
}

/// The default of `names` below the start, with no suffix.
fn base(names: &[String]) -> String {
    let segments = names.iter().map(|name| {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    });
    std::iter::once("plc".to_owned())
        .chain(segments)
        .collect::<Vec<_>>()
        .join(".")
}

proptest! {
    #[test]
    fn gives_valid_unique_names_and_keeps_each_given_name(
        start in prop::sample::select(vec!["", "Objects"]),
        picks in proptest::collection::vec(
            (
                proptest::collection::vec(device_name(), 1..3),
                proptest::option::weighted(0.3, channel_name()),
            ),
            0..12,
        ),
        taken in proptest::collection::vec(channel_name(), 0..6),
    ) {
        let paths: Vec<_> =
            picks.iter().map(|(names, _)| below(start, names)).collect();
        let given: Vec<_> = picks.iter().map(|(_, given)| given.as_ref()).collect();
        let pairs: Vec<_> = paths.iter().zip(given.iter().copied()).collect();
        let start = if start.is_empty() { Path::root() } else { path(start) };
        let result = names(&name("plc"), &start, &pairs, &taken);
        prop_assert_eq!(result.len(), picks.len());

        let mut used: Set<String> = taken
            .iter()
            .chain(given.iter().flatten().copied())
            .map(folded)
            .collect();
        for ((names, given), result) in picks.iter().zip(&result) {
            if let Some(given) = given {
                prop_assert_eq!(result.as_ref(), Some(given));
                continue;
            }
            let base = base(names);
            let free = (1..=used.len() + 1)
                .map(|n| if n == 1 { base.clone() } else { format!("{base}_{n}") })
                .find(|text| !used.contains(&text.to_ascii_lowercase()))
                .expect("a free name");
            match result {
                None => {
                    prop_assert!(free.len() > Name::MAX_BYTES, "{free:?} was left out");
                }
                Some(default) => {
                    prop_assert_eq!(default.as_str(), free.as_str());
                    let parsed = default.as_str().parse::<Name>();
                    prop_assert_eq!(parsed, Ok(default.clone()));
                    prop_assert!(!default.reserved());
                    prop_assert!(used.insert(folded(default)), "{default} is in use");
                }
            }
        }
    }
}
