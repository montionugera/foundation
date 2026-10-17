//! The skips check. The lists of each OS in `ci.yaml` are the one record of what that
//! OS does not run (LINUX CI), so a test skips an OS by an attribute only in a case of
//! `docs/claude/testing.md`, "No `#[ignore]`", and its reason says so.

use std::collections::BTreeSet;
use std::path::Path;

use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, Expr, Lit, Meta, Token};

use crate::{field, files};

/// The values that a `cfg` predicate reads on an OS of the lists.
struct Os {
    name: &'static str,
    family: &'static str,
    env: &'static str,
    vendor: &'static str,
}

/// The OSes of the lists, in the order of each `[bool; 3]` of this module.
const OSES: [Os; 3] = [
    Os {
        name: "linux",
        family: "unix",
        env: "gnu",
        vendor: "unknown",
    },
    Os {
        name: "macos",
        family: "unix",
        env: "",
        vendor: "apple",
    },
    Os {
        name: "windows",
        family: "windows",
        env: "msvc",
        vendor: "pc",
    },
];

/// Each name that a reason may give after `only `, with the OSes that it names.
const NAMES: [(&str, [bool; 3]); 4] = [
    ("Linux", [true, false, false]),
    ("macOS", [false, true, false]),
    ("Windows", [false, false, true]),
    ("Unix", [true, true, false]),
];

/// Checks that no `.rs` file under the directory of a target of the workspace at
/// `root` ignores a test on some OSes of the lists, unless its reason starts with
/// `only <OS>`, where `<OS>` names the OSes that run the test. It does not read the
/// body of a macro call. It also refuses a file that does not parse, a `cfg` predicate
/// that is not one of Rust, and an `ignore` reason that is not a string literal.
pub(crate) fn check(root: &Path) -> Result<(), Vec<String>> {
    let problems = problems(root).map_err(|e| vec![e])?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn problems(root: &Path) -> Result<Vec<String>, String> {
    let metadata = crate::metadata(root)?;
    let mut sources = BTreeSet::new();
    for package in field::list(&metadata, "packages")? {
        for target in field::list(package, "targets")? {
            let path = files::normalize(Path::new(field::text(target, "src_path")?));
            let dir = path
                .parent()
                .ok_or_else(|| format!("{} has no directory", path.display()))?;
            sources.extend(files::rust(dir)?);
        }
    }
    let mut problems = Vec::new();
    for file in sources {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let shown = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .display()
            .to_string();
        problems.extend(refusals(&shown, &text));
    }
    Ok(problems)
}

/// The problems of `text`, the source of the file shown as `shown`: each refusal, or
/// source that does not parse, a `cfg` predicate that is not one of Rust, or an `ignore`
/// reason that is not a string literal.
fn refusals(shown: &str, text: &str) -> Vec<String> {
    let file = match syn::parse_file(text) {
        Ok(file) => file,
        Err(e) => return vec![format!("`{shown}`: {e}")],
    };
    let mut skips = Skips {
        shown,
        item: Vec::new(),
        problems: Vec::new(),
    };
    skips.visit_file(&file);
    skips.problems
}

/// Walks a file, with the path of the item it is in.
struct Skips<'a> {
    shown: &'a str,
    item: Vec<String>,
    problems: Vec<String>,
}

impl Skips<'_> {
    fn within(&mut self, name: &syn::Ident, visit: impl FnOnce(&mut Self)) {
        self.item.push(name.to_string());
        visit(self);
        self.item.pop();
    }
}

impl<'ast> Visit<'ast> for Skips<'_> {
    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.within(&i.ident, |s| syn::visit::visit_item_mod(s, i));
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.within(&i.sig.ident, |s| syn::visit::visit_item_fn(s, i));
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.within(&i.sig.ident, |s| syn::visit::visit_impl_item_fn(s, i));
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.within(&i.sig.ident, |s| syn::visit::visit_trait_item_fn(s, i));
    }

    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        let mut found = Vec::new();
        let item = self.item.join("::");
        if let Err(e) = ignores([&attribute.meta], [true; 3], &mut found) {
            self.problems
                .push(format!("`{}`: `{item}`: {e}", self.shown));
        }
        for (skipped, reason) in found {
            let runs = skipped.map(|s| !s);
            if skipped == [false; 3]
                || runs == [false; 3]
                || reason.is_some_and(|r| gives(&r, runs))
            {
                continue;
            }
            self.problems.push(refused(self.shown, &item));
        }
    }
}

/// Adds to `found` each `ignore` of `attributes` that applies on the OSes of `on`, with
/// the OSes on which it applies and its reason. It reads into each `cfg_attr`.
fn ignores<'a>(
    attributes: impl IntoIterator<Item = &'a Meta>,
    on: [bool; 3],
    found: &mut Vec<([bool; 3], Option<String>)>,
) -> Result<(), String> {
    for meta in attributes {
        let path = meta.path();
        if path.is_ident("cfg_attr") {
            let (predicate, inner) = meta
                .require_list()
                .and_then(|list| list.parse_args_with(cfg_attr))
                .map_err(|e| e.to_string())?;
            let mut held = on;
            for (held, os) in held.iter_mut().zip(&OSES) {
                *held = *held && holds(&predicate, os)?;
            }
            ignores(&inner, held, found)?;
        } else if path.is_ident("ignore") {
            let reason = match meta {
                Meta::Path(_) => None,
                Meta::NameValue(pair) => Some(string(&pair.value)?),
                Meta::List(_) => {
                    return Err("`ignore` takes no list".to_string());
                }
            };
            found.push((on, reason));
        }
    }
    Ok(())
}

/// Parses the arguments of a `cfg_attr`: its predicate and its attributes.
fn cfg_attr(input: ParseStream<'_>) -> syn::Result<(Meta, Vec<Meta>)> {
    let predicate = input.parse()?;
    input.parse::<Token![,]>()?;
    let attributes = Punctuated::<Meta, Token![,]>::parse_terminated(input)?;
    Ok((predicate, attributes.into_iter().collect()))
}

/// Reports whether the `cfg` predicate holds on `os` in `cargo test --all-features`:
/// `test`, `debug_assertions`, and each `feature` hold, and no other name of no OS.
fn holds(predicate: &Meta, os: &Os) -> Result<bool, String> {
    let name = predicate.path().get_ident().map(ToString::to_string);
    let name = name.as_deref().unwrap_or_default();
    match predicate {
        Meta::Path(_) => Ok(match name {
            "unix" | "windows" => os.family == name,
            "test" | "debug_assertions" => true,
            _ => false,
        }),
        Meta::NameValue(pair) => {
            let value = string(&pair.value)?;
            Ok(match name {
                "target_os" => os.name == value,
                "target_family" => os.family == value,
                "target_env" => os.env == value,
                "target_vendor" => os.vendor == value,
                "feature" => true,
                _ => false,
            })
        }
        Meta::List(list) => {
            let nested = list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|e| e.to_string())?;
            let mut values = Vec::new();
            for meta in &nested {
                values.push(holds(meta, os)?);
            }
            match (name, &values[..]) {
                ("any", _) => Ok(values.contains(&true)),
                ("all", _) => Ok(!values.contains(&false)),
                ("not", [value]) => Ok(!value),
                _ => Err("an unknown `cfg` predicate".to_string()),
            }
        }
    }
}

/// The value of a string literal.
fn string(expr: &Expr) -> Result<String, String> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: Lit::Str(text),
            ..
        }) => Ok(text.value()),
        _ => Err("a value that is no string literal".to_string()),
    }
}

/// Reports whether `reason` starts with `only <OS>`, where `<OS>` names the OSes of
/// `runs` and no more.
fn gives(reason: &str, runs: [bool; 3]) -> bool {
    NAMES.iter().any(|(name, named)| {
        *named == runs
            && reason
                .strip_prefix("only ")
                .and_then(|r| r.strip_prefix(name))
                .is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric()))
    })
}

fn refused(shown: &str, item: &str) -> String {
    format!(
        "`{shown}`: `{item}` ignores a test on an OS outside the lists of LINUX CI. Fix: \
         remove the `ignore` and add `--skip <path> # #<issue>` to the `tests` list of \
         each OS in `ci.yaml` where the test fails, or, in a case of \
         `docs/claude/testing.md`, \"No `#[ignore]`\", start its reason with `only \
         <OS>`, where <OS> is Linux, macOS, Windows, or Unix and names each OS that \
         runs the test."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_each_ignore_on_an_os_outside_its_case_and_names_what_it_cannot_read() {
        let skips = "crates/a/src/skips";
        let mut expected = vec![
            "`bench/b/src/skips.rs`: `b` ignores a test on an OS outside the lists of \
             LINUX CI. Fix: remove the `ignore` and add `--skip <path> # #<issue>` to \
             the `tests` list of each OS in `ci.yaml` where the test fails, or, in a \
             case of `docs/claude/testing.md`, \"No `#[ignore]`\", start its reason \
             with `only <OS>`, where <OS> is Linux, macOS, Windows, or Unix and names \
             each OS that runs the test."
                .to_string(),
            format!("`{skips}/arity.rs`: `a`: an unknown `cfg` predicate"),
            format!("`{skips}/list.rs`: `b::a`: `ignore` takes no list"),
            format!("`{skips}/literal.rs`: `a`: a value that is no string literal"),
            format!("`{skips}/parse.rs`: cannot parse string into token stream"),
        ];
        let items = [
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "q", "r",
            "n::o", "n::p",
        ];
        let refused_rs = format!("{skips}/refused.rs");
        expected.extend(items.map(|i| refused(&refused_rs, i)));
        expected.extend([
            format!("`{skips}/unknown.rs`: `a`: an unknown `cfg` predicate"),
            refused("crates/a/src/skips.rs", "a"),
            refused("oracles/a/common.rs", "ready_on_each_os"),
        ]);
        assert_eq!(check(&crate::fixture()), Err(expected));
    }
}
