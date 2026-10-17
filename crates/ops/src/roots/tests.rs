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
    let roots = Failures::new("s".into(), fits.clone());
    let expected = json!({ "source": "s", "failures": fits, "unlisted": 0 });
    assert_eq!(written(&roots), expected);
    let over = vec![holding(1 << 19), holding((1 << 19) + 1), String::new()];
    let roots = Failures::new("s".into(), over.clone());
    let expected = json!({ "source": "s", "failures": [over[0]], "unlisted": 2 });
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
        sizes in vec(0usize..(1 << 18), 0..24),
    ) {
        let failed: Vec<String> = sizes.iter().map(|&size| "x".repeat(size)).collect();
        let roots = Failures::new(String::new(), failed.clone());
        let kept = roots.listed.len();
        let held = |count: usize| {
            let failures = failed[..count].iter();
            failures.map(|failure| size_of::<String>() + failure.len()).sum::<usize>()
        };
        prop_assert_eq!(&roots.listed[..], &failed[..kept]);
        prop_assert!(held(kept) <= 1 << 20);
        let counted = super::count(kept) + roots.unlisted;
        prop_assert_eq!(counted, super::count(failed.len()));
        prop_assert!(kept == failed.len() || held(kept + 1) > 1 << 20);
    }
}
