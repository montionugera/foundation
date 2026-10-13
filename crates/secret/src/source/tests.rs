use document::value::Call;
use document::{Position, Span};
use proptest::prelude::*;

use super::*;

/// A one-byte span at `offset`.
fn at(offset: u32) -> Option<Span> {
    let position = |offset| Position {
        offset,
        line: 0,
        column: offset,
    };
    Span::new(document::Source(0), position(offset), position(offset + 1))
}

fn string(text: &str) -> Value {
    Value {
        kind: Kind::String(text.into()),
        span: at(9),
    }
}

/// `function(arguments)` at offset 3.
fn call(function: &str, arguments: Vec<Value>) -> Value {
    Value {
        kind: Kind::Call(Call {
            function: function.into(),
            function_span: at(3),
            arguments,
        }),
        span: at(3),
    }
}

/// The diagnostic that `read` gives for `value`, at offset 3.
fn refused(value: &Value, message: &str, fix: &str) -> Diagnostic {
    let diagnostic = Source::read(value).expect_err("a bad source");
    assert_eq!(
        diagnostic,
        Diagnostic::new(
            Code::new("secret.source"),
            value.span,
            message.into(),
            fix.into()
        )
    );
    diagnostic
}

const WRITE: &str =
    "Write `env(\"NAME\")`, `file(\"/absolute/path\")`, or `secret(\"name\")`";

#[test]
fn reads_each_source() {
    let name: Name = "plant.token".parse().expect("a name");
    for (value, source) in [
        (
            call("env", vec![string("INFLUX_TOKEN")]),
            Source::Env("INFLUX_TOKEN".into()),
        ),
        (
            call("file", vec![string("/run/secrets/influx token")]),
            Source::File("/run/secrets/influx token".into()),
        ),
        (
            call("secret", vec![string("plant.token")]),
            Source::Secret(name.clone()),
        ),
        (
            call(
                "secret",
                vec![Value {
                    kind: Kind::Reference(name.clone()),
                    span: at(10),
                }],
            ),
            Source::Secret(name),
        ),
    ] {
        assert_eq!(Source::read(&value), Ok(source));
    }
}

#[test]
fn refuses_a_value_in_place_of_a_source() {
    let value = Value {
        kind: Kind::String("abc123".into()),
        span: at(3),
    };
    refused(
        &value,
        "a secret takes where its value comes from, never the value",
        WRITE,
    );
}

#[test]
fn refuses_another_function() {
    refused(
        &call("vault", vec![string("a")]),
        "a secret comes from `env`, `file`, or `secret`, not `vault`",
        WRITE,
    );
}

#[test]
fn refuses_a_count_of_arguments_that_is_not_one() {
    refused(
        &call("env", vec![]),
        "`env` takes one argument, not 0",
        WRITE,
    );
    refused(
        &call("file", vec![string("/a"), string("/b")]),
        "`file` takes one argument, not 2",
        WRITE,
    );
}

#[test]
fn refuses_an_argument_that_is_not_a_string() {
    let reference = Value {
        kind: Kind::Reference("INFLUX_TOKEN".parse().expect("a name")),
        span: at(9),
    };
    refused(
        &call("env", vec![reference]),
        "the argument of `env` is a reference, not a string",
        WRITE,
    );
    let integer = Value {
        kind: Kind::Integer(7),
        span: at(9),
    };
    refused(
        &call("secret", vec![integer]),
        "the argument of `secret` is an integer, not a string",
        WRITE,
    );
}

#[test]
fn refuses_a_bad_variable_name() {
    for name in ["", "A=B", "A\0B"] {
        refused(
            &call("env", vec![string(name)]),
            "the argument of `env` is not a variable name",
            "Write a name with no `=` and no NUL, such as `env(\"INFLUX_TOKEN\")`",
        );
    }
}

#[test]
fn refuses_a_relative_path_or_one_with_a_parent_segment() {
    for path in [
        "",
        "token",
        "./token",
        "secrets/token",
        "/run/../etc/shadow",
        "/a/..",
    ] {
        refused(
            &call("file", vec![string(path)]),
            "the argument of `file` is not an absolute path with no `..`",
            "Write the whole path from the root, such as \
             `file(\"/run/secrets/influx_token\")`",
        );
    }
}

#[test]
fn refuses_a_bad_secret_name() {
    refused(
        &call("secret", vec![string("plant..token")]),
        "the argument of `secret` is not a valid name",
        types::name::Error::Segment {
            input: String::new(),
            segment: String::new(),
        }
        .fix(),
    );
    refused(
        &call("secret", vec![string("")]),
        "the argument of `secret` is not a valid name",
        types::name::Error::Empty.fix(),
    );
}

#[test]
fn shows_each_source_as_written() {
    for (source, shown) in [
        (Source::Env("INFLUX_TOKEN".into()), r#"env("INFLUX_TOKEN")"#),
        (
            Source::File("/run/secrets/token".into()),
            r#"file("/run/secrets/token")"#,
        ),
        (
            Source::Secret("plant.token".parse().expect("a name")),
            r#"secret("plant.token")"#,
        ),
        (Source::Env("A\"B".into()), r#"env("A\"B")"#),
    ] {
        assert_eq!(source.to_string(), shown);
    }
}

proptest! {
    #[test]
    fn no_text_quotes_what_the_config_wrote(
        function in prop::sample::select(vec!["env", "file", "secret", "vault"]),
        text in "[a-z]{0,4}[=/.\0]{0,2}[A-Za-z0-9]{12,24}",
        count in 0..3_usize,
    ) {
        let bare = Value { kind: Kind::String(text.as_str().into()), span: at(3) };
        let arguments = vec![string(&text); count];
        for value in [bare, call(function, arguments)] {
            if let Err(diagnostic) = Source::read(&value) {
                prop_assert!(!diagnostic.message.contains(&text));
                prop_assert!(!diagnostic.fix.contains(&text));
            }
        }
    }
}
