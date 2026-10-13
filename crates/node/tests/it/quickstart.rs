//! Checks the quickstart page against the binary.

use crate::{Rig, text};

#[path = "../../../ops/tests/common/quickstart.rs"]
mod page;

use page::blocks;
pub(crate) use page::{plant, shown};

#[test]
fn the_binary_plans_and_applies_the_config_of_the_page_as_the_page_shows() {
    let mut rig = Rig::new();
    rig.config(plant());
    rig.start();
    let [plan, apply] = rig.apply();
    assert_eq!((&*plan, &*apply), (shown("+ channel"), shown("Applied")));
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
