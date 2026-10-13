//! The blocks of the quickstart page, which the `ops` and `node` tests read.

const PAGE: &str = include_str!("../../../../docs/quickstart.md");

/// The text of each fenced block of the page whose language is `language`.
pub(crate) fn blocks(language: &str) -> impl Iterator<Item = &'static str> + '_ {
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
pub(crate) fn shown(start: &str) -> &'static str {
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
