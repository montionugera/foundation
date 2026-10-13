//! Checks the quickstart page against the binary.

use crate::{Rig, start, text};

const PAGE: &str = include_str!("../../../../docs/quickstart.md");

/// The text of each fenced block on the page whose language is `language`, as `blocks`
/// of `crates/ops/src/common.rs` gives it.
fn blocks(language: &str) -> impl Iterator<Item = &'static str> + '_ {
    PAGE.split("```")
        .skip(1)
        .step_by(2)
        .filter_map(move |block| block.strip_prefix(language)?.strip_prefix('\n'))
}

/// The config of the page, its one `hcl` block.
pub(crate) fn plant() -> &'static str {
    let hcl: Vec<_> = blocks("hcl").collect();
    let [hcl] = hcl[..] else {
        panic!("the page has {} `hcl` blocks, not one", hcl.len());
    };
    hcl
}

/// The output that the page shows, its one `text` block that starts with `start`.
fn shown(start: &str) -> &'static str {
    let shown: Vec<_> = blocks("text")
        .filter(|block| block.starts_with(start))
        .collect();
    let [text] = shown[..] else {
        panic!(
            "the page has {} `text` blocks that start with {start:?}, not one",
            shown.len()
        );
    };
    text
}

#[test]
fn the_page_shows_what_the_first_start_prints() {
    assert_eq!(shown("node "), start::RUNS);
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1744"]
fn the_binary_plans_and_applies_the_config_of_the_page_as_the_page_shows() {
    let mut rig = Rig::new();
    rig.config(plant());
    rig.start();
    let plan = rig.run(&["plan", "plant.hcl", "--out", "plant.plan"], b"");
    let plan = (plan.status.code(), text(&plan.stdout), text(&plan.stderr));
    assert_eq!(plan, (Some(0), shown("+ channel"), ""));
    let apply = rig.run(&["apply", "plant.plan"], b"");
    let apply = (
        apply.status.code(),
        text(&apply.stdout),
        text(&apply.stderr),
    );
    assert_eq!(apply, (Some(0), shown("Applied"), ""));
}

#[test]
fn the_help_names_each_command_and_flag_on_the_quickstart_page() {
    let commands: Vec<Vec<&str>> = blocks("sh")
        .flat_map(str::lines)
        .filter_map(|line| line.strip_prefix("foundation "))
        .map(|words| words.split_whitespace().collect())
        .collect();
    assert!(!commands.is_empty(), "the page has no `foundation` command");
    for words in &commands {
        let output = Rig::new().run(&[words[0], "--help"], b"");
        let help = text(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`foundation {} --help` failed: {}",
            words[0],
            text(&output.stderr)
        );
        for flag in words.iter().filter(|word| word.starts_with("--")) {
            assert!(
                help.split_whitespace()
                    .any(|word| word.trim_end_matches(',') == *flag),
                "`foundation {} --help` does not name `{flag}`:\n{help}",
                words[0]
            );
        }
    }
}
