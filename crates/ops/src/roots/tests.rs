use proptest::collection::vec;
use proptest::prelude::*;
use serde_json::{Value, json};

use super::Failures;

/// A failure that holds `held` bytes with its `String`.
fn holding(held: usize) -> String {
    "x".repeat(held - size_of::<String>())
}

/// `roots` as `status` writes them.
fn written(roots: &Failures) -> Value {
    serde_json::to_value(roots).expect("roots convert to JSON")
}

#[test]
fn keep_the_failures_that_hold_at_most_a_mebibyte() {
    let fits = vec![holding(1 << 19), holding(1 << 19)];
    let roots = Failures::new(String::new(), fits.clone());
    let expected = json!({ "source": "", "failures": fits, "unlisted": 0 });
    assert_eq!(written(&roots), expected);
    let over = vec![holding(1 << 19), holding((1 << 19) + 1), String::new()];
    let roots = Failures::new(String::new(), over.clone());
    let expected = json!({ "source": "", "failures": [over[0]], "unlisted": 2 });
    assert_eq!(written(&roots), expected);
}

#[test]
fn the_source_counts_against_the_mebibyte() {
    let source = "s".repeat(1 << 19);
    let failed = vec![holding(1 << 19), String::new()];
    let roots = Failures::new(source.clone(), failed.clone());
    let expected = json!({ "source": source, "failures": [failed[0]], "unlisted": 1 });
    assert_eq!(written(&roots), expected);
    let source = "s".repeat((1 << 20) + 1);
    let roots = Failures::new(source.clone(), vec![String::new()]);
    let expected = json!({ "source": source, "failures": [], "unlisted": 1 });
    assert_eq!(written(&roots), expected);
}

#[test]
fn json_keeps_the_bound_and_adds_the_count() {
    let given =
        json!({ "source": "s", "failures": ["x".repeat(1 << 21)], "unlisted": 3 });
    let roots: Failures = serde_json::from_value(given).expect("roots from JSON");
    let expected = json!({ "source": "s", "failures": [], "unlisted": 4 });
    assert_eq!(written(&roots), expected);
}

proptest! {
    #[test]
    fn keep_a_prefix_of_at_most_a_mebibyte_and_count_the_rest(
        source in 0usize..(1 << 19),
        sizes in vec(0usize..(1 << 18), 0..24),
    ) {
        let failed: Vec<String> = sizes.iter().map(|&size| "x".repeat(size)).collect();
        let roots = written(&Failures::new("s".repeat(source), failed.clone()));
        let listed: Vec<String> =
            serde_json::from_value(roots["failures"].clone()).expect("the failures");
        let kept = listed.len();
        let held = |count: usize| {
            let failures = failed[..count].iter();
            let each = failures.map(|failure| size_of::<String>() + failure.len());
            source + each.sum::<usize>()
        };
        prop_assert_eq!(&listed[..], &failed[..kept]);
        prop_assert!(held(kept) <= 1 << 20);
        let unlisted = u64::try_from(failed.len() - kept).ok();
        prop_assert_eq!(roots["unlisted"].as_u64(), unlisted);
        prop_assert!(kept == failed.len() || held(kept + 1) > 1 << 20);
    }
}

#[test]
fn the_schema_documents_each_field() {
    let schema = schemars::schema_for!(Failures);
    let schema = schema.as_value();
    assert_eq!(schema["title"], "Failures");
    let properties = &schema["properties"];
    assert_eq!(
        properties["source"]["description"],
        "Where the read looked."
    );
    assert_eq!(
        properties["failures"]["description"],
        "The text of each failure, in the order of the read.",
    );
    assert_eq!(
        properties["unlisted"]["description"],
        "The count of the failures after `failures`, which the node does not keep.",
    );
}
