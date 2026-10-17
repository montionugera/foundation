//! The skips check. The lists of each OS in `ci.yaml` are the one record of what that
//! OS does not run (LINUX CI), so a test skips an OS by an attribute only in a case of
//! `docs/claude/testing.md`, "No `#[ignore]`", and its reason says so.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cfg_expr::targets::get_builtin_target_by_triple;
use cfg_expr::{Expression, Predicate};
use syn::ext::IdentExt;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, Expr, Ident, Lit, Meta, Token};

use crate::{field, files};

/// A runner of CI that runs the tests of the workspace: its target, and its OS as a
/// reason names it.
struct Runner {
    triple: &'static str,
    os: &'static str,
}

/// The runners of the lists of LINUX CI, one for each OS, in the order in which a
/// reason names their OSes.
const RUNNERS: [Runner; 3] = [
    Runner {
        triple: "x86_64-unknown-linux-gnu",
        os: "Linux",
    },
    Runner {
        triple: "aarch64-apple-darwin",
        os: "macOS",
    },
    Runner {
        triple: "x86_64-pc-windows-msvc",
        os: "Windows",
    },
];

/// A set of the runners of `RUNNERS`, one bit for each by its index.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Runners(u8);

impl Runners {
    const ALL: Self = Self((1 << RUNNERS.len()) - 1);
    const NONE: Self = Self(0);

    /// The runners on which `predicate`, the predicate of a `cfg`, holds in `cargo test
    /// --all-features`: `test`, `debug_assertions`, and each `feature` hold, and each
    /// other name that is not one of a target does not.
    fn holding(predicate: &Meta) -> Result<Self, String> {
        let expression =
            Expression::parse(&text(predicate)?).map_err(|e| e.to_string())?;
        let mut runners = Self::NONE;
        for (i, runner) in RUNNERS.iter().enumerate() {
            let target = get_builtin_target_by_triple(runner.triple)
                .expect("invariant: rustc knows the target of each runner");
            let holds = expression.eval(|predicate| match predicate {
                Predicate::Target(name) => Some(name.matches(target)),
                Predicate::Test
                | Predicate::DebugAssertions
                | Predicate::Feature(_) => Some(true),
                Predicate::ProcMacro
                | Predicate::Flag(_)
                | Predicate::KeyValue { .. } => Some(false),
                Predicate::TargetFeature(_) => None,
            });
            match holds {
                Some(true) => runners.0 |= 1 << i,
                Some(false) => {}
                None => {
                    return Err(format!(
                        "a `cfg` predicate with no known value on `{}`",
                        runner.triple
                    ));
                }
            }
        }
        Ok(runners)
    }

    fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The OSes of the runners, in the order of `RUNNERS`.
    fn oses(self) -> Vec<&'static str> {
        RUNNERS
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 >> i & 1 == 1)
            .map(|(_, runner)| runner.os)
            .collect()
    }
}

/// Checks each test function in the module tree of each target of the workspace at
/// `root`, other than in the body of a macro call. A test that its `cfg` builds on an
/// OS of the lists, but that an `ignore` skips on the runner of that OS, is refused,
/// unless the reason of each such `ignore` is `only <OS>` or starts with `only <OS>:`,
/// where `<OS>` names the OSes that run the test. It also refuses a file that does not
/// parse, a `mod` with no file, a `path` in a `cfg_attr`, a `cfg` predicate that is not
/// one of Rust or whose value on a runner is not known, and an `ignore` reason that is
/// not a string literal.
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
    let mut modules = Vec::new();
    for package in field::list(&metadata, "packages")? {
        for target in field::list(package, "targets")? {
            let file = files::normalize(Path::new(field::text(target, "src_path")?));
            let dir = file
                .parent()
                .ok_or_else(|| format!("{} has no directory", file.display()))?
                .to_path_buf();
            modules.push(Module {
                file,
                dir,
                on: Runners::ALL,
            });
        }
    }
    let mut seen = BTreeSet::new();
    let mut problems = BTreeSet::new();
    while let Some(module) = modules.pop() {
        if !seen.insert((module.file.clone(), module.on)) {
            continue;
        }
        let shown = module
            .file
            .strip_prefix(root)
            .unwrap_or(&module.file)
            .display()
            .to_string();
        let text = std::fs::read_to_string(&module.file)
            .map_err(|e| format!("{}: {e}", module.file.display()))?;
        let file = match syn::parse_file(&text) {
            Ok(file) => file,
            Err(e) => {
                problems.insert(format!("`{shown}`: {e}"));
                continue;
            }
        };
        let mut items = Items {
            shown: &shown,
            file: &module.file,
            dir: module.dir,
            inline: 0,
            item: Vec::new(),
            on: module.on,
            modules: &mut modules,
            problems: &mut problems,
        };
        items.enter(&file.attrs, None, |items, _| items.visit_file(&file));
    }
    Ok(problems.into_iter().collect())
}

/// A file of a module, the directory of the modules that it declares, and the runners
/// on which it builds.
struct Module {
    file: PathBuf,
    dir: PathBuf,
    on: Runners,
}

/// What the attributes of an item do: the runners on which it builds, and each
/// `ignore`, with the runners on which it applies and its reason.
struct Effect {
    builds: Runners,
    ignores: Vec<Ignore>,
}

struct Ignore {
    on: Runners,
    reason: Option<String>,
}

/// Walks the items of one file.
struct Items<'a> {
    shown: &'a str,
    file: &'a Path,
    /// The directory of the modules that the item declares, with each inline module.
    dir: PathBuf,
    /// How many inline modules hold the item.
    inline: usize,
    item: Vec<String>,
    /// The runners on which the item builds.
    on: Runners,
    modules: &'a mut Vec<Module>,
    problems: &'a mut BTreeSet<String>,
}

impl Items<'_> {
    /// Visits an item with `attributes`, named `name`, on the runners on which it
    /// builds, if any.
    fn enter(
        &mut self,
        attributes: &[Attribute],
        name: Option<&Ident>,
        visit: impl FnOnce(&mut Self, &[Ignore]),
    ) {
        self.item.extend(name.map(ToString::to_string));
        match effect(attributes) {
            Ok(effect) => {
                let on = self.on.and(effect.builds);
                if on != Runners::NONE {
                    let outer = std::mem::replace(&mut self.on, on);
                    visit(self, &effect.ignores);
                    self.on = outer;
                }
            }
            Err(e) => self.problem(&e),
        }
        if name.is_some() {
            self.item.pop();
        }
    }

    fn problem(&mut self, problem: &str) {
        let item = self.item.join("::");
        self.problems
            .insert(format!("`{}`: `{item}`: {problem}", self.shown));
    }

    /// Refuses each `ignore` of a test that skips an OS outside a case. A test that
    /// its `ignore` attributes skip on each OS on which it builds has no OS for a
    /// reason to name, unless a plain `#[ignore]` skips it on every OS.
    fn test(&mut self, ignores: &[Ignore]) {
        if ignores.iter().any(|ignore| ignore.on == Runners::ALL) {
            return;
        }
        let skipped = ignores
            .iter()
            .fold(Runners::NONE, |skipped, ignore| skipped.or(ignore.on));
        let runs = self.on.without(skipped);
        for ignore in ignores {
            let given = runs != Runners::NONE
                && ignore
                    .reason
                    .as_deref()
                    .is_some_and(|r| gives(r, &runs.oses()));
            if ignore.on.and(self.on) != Runners::NONE && !given {
                self.problems
                    .insert(refused(self.shown, &self.item.join("::")));
            }
        }
    }

    /// The directory from which a `#[path]` of the item names its file.
    fn base(&self) -> &Path {
        match self.inline {
            0 => self.file.parent().unwrap_or(&self.dir),
            _ => &self.dir,
        }
    }

    /// The file of the module `module` declares with no body, and the directory of the
    /// modules of that file.
    fn declared(&self, module: &syn::ItemMod) -> Result<(PathBuf, PathBuf), String> {
        let name = module.ident.unraw().to_string();
        if let Some(path) = path(&module.attrs)? {
            let file = files::normalize(&self.base().join(path));
            let dir = file.parent().unwrap_or(self.base()).to_path_buf();
            return Ok((file, dir));
        }
        let dir = self.dir.join(&name);
        let flat = self.dir.join(format!("{name}.rs"));
        if flat.is_file() {
            Ok((flat, dir))
        } else if dir.join("mod.rs").is_file() {
            Ok((dir.join("mod.rs"), dir))
        } else {
            Err(format!("no file for the module `{name}`"))
        }
    }
}

impl<'ast> Visit<'ast> for Items<'_> {
    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.enter(&i.attrs, Some(&i.ident), |s, _| {
            if i.content.is_some() {
                let dir = match path(&i.attrs) {
                    Ok(Some(path)) => files::normalize(&s.base().join(path)),
                    Ok(None) => s.dir.join(i.ident.unraw().to_string()),
                    Err(e) => return s.problem(&e),
                };
                let outer = std::mem::replace(&mut s.dir, dir);
                s.inline += 1;
                syn::visit::visit_item_mod(s, i);
                s.inline -= 1;
                s.dir = outer;
                return;
            }
            match s.declared(i) {
                Ok((file, dir)) => s.modules.push(Module {
                    file,
                    dir,
                    on: s.on,
                }),
                Err(e) => s.problem(&e),
            }
        });
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        self.enter(&i.attrs, None, |s, _| syn::visit::visit_item_impl(s, i));
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.enter(&i.attrs, None, |s, _| syn::visit::visit_item_trait(s, i));
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.enter(&i.attrs, Some(&i.sig.ident), |s, ignores| {
            s.test(ignores);
            syn::visit::visit_item_fn(s, i);
        });
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.enter(&i.attrs, Some(&i.sig.ident), |s, ignores| {
            s.test(ignores);
            syn::visit::visit_impl_item_fn(s, i);
        });
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.enter(&i.attrs, Some(&i.sig.ident), |s, ignores| {
            s.test(ignores);
            syn::visit::visit_trait_item_fn(s, i);
        });
    }
}

/// What `attributes` do. It reads into each `cfg_attr`.
fn effect(attributes: &[Attribute]) -> Result<Effect, String> {
    let mut effect = Effect {
        builds: Runners::ALL,
        ignores: Vec::new(),
    };
    apply(
        attributes.iter().map(|a| &a.meta),
        Runners::ALL,
        &mut effect,
    )?;
    Ok(effect)
}

/// Adds to `effect` what each of `attributes` does on the runners of `on`.
fn apply<'a>(
    attributes: impl IntoIterator<Item = &'a Meta>,
    on: Runners,
    effect: &mut Effect,
) -> Result<(), String> {
    for meta in attributes {
        let path = meta.path();
        if path.is_ident("cfg_attr") {
            let (predicate, inner) = meta
                .require_list()
                .and_then(|list| list.parse_args_with(cfg_attr))
                .map_err(|e| e.to_string())?;
            if inner.iter().any(|meta| meta.path().is_ident("path")) {
                return Err("a `path` in a `cfg_attr`".to_string());
            }
            apply(&inner, on.and(Runners::holding(&predicate)?), effect)?;
        } else if path.is_ident("cfg") {
            let predicate = meta
                .require_list()
                .and_then(syn::MetaList::parse_args::<Meta>)
                .map_err(|e| e.to_string())?;
            let fails = on.without(Runners::holding(&predicate)?);
            effect.builds = effect.builds.without(fails);
        } else if path.is_ident("ignore") {
            let reason = match meta {
                Meta::Path(_) => None,
                Meta::NameValue(pair) => Some(string(&pair.value)?),
                Meta::List(_) => {
                    return Err("`ignore` takes no list".to_string());
                }
            };
            effect.ignores.push(Ignore { on, reason });
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

/// The path of a `#[path]` among `attributes`.
fn path(attributes: &[Attribute]) -> Result<Option<String>, String> {
    attributes
        .iter()
        .find(|a| a.path().is_ident("path"))
        .map(|a| match &a.meta {
            Meta::NameValue(pair) => string(&pair.value),
            _ => Err("a `path` that is no string".to_string()),
        })
        .transpose()
}

/// The text of a `cfg` predicate, as `cfg-expr` reads it.
fn text(predicate: &Meta) -> Result<String, String> {
    let name = predicate
        .path()
        .get_ident()
        .ok_or("a `cfg` name that is no identifier")?
        .to_string();
    Ok(match predicate {
        Meta::Path(_) => name,
        Meta::NameValue(pair) => format!("{name} = \"{}\"", string(&pair.value)?),
        Meta::List(list) => {
            let nested = list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|e| e.to_string())?;
            let nested: Vec<_> = nested.iter().map(text).collect::<Result<_, _>>()?;
            format!("{name}({})", nested.join(", "))
        }
    })
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

/// Reports whether `reason` is `only <OS>` or starts with `only <OS>:`, where `<OS>`
/// names `oses` and no more: their names joined by `and`, or Unix for Linux and macOS.
fn gives(reason: &str, oses: &[&str]) -> bool {
    let joined = oses.join(" and ");
    let unix = (oses == ["Linux", "macOS"]).then_some("Unix");
    [Some(joined.as_str()), unix]
        .into_iter()
        .flatten()
        .any(|name| {
            reason
                .strip_prefix("only ")
                .and_then(|r| r.strip_prefix(name))
                .is_some_and(|r| r.is_empty() || r.starts_with(':'))
        })
}

fn refused(shown: &str, item: &str) -> String {
    format!(
        "`{shown}`: `{item}` ignores a test on an OS outside the lists of LINUX CI. \
         Fix: remove the `ignore` and add `--skip <path> # #<issue>` to the `tests` \
         list of each OS in `ci.yaml` where the test fails, or, in a case of \
         `docs/claude/testing.md`, \"No `#[ignore]`\", make its reason `only <OS>` \
         or start it with `only <OS>:`, where <OS> names each OS that runs the test: \
         Linux, macOS, or Windows, two of them joined by `and` in that order, or Unix \
         for Linux and macOS."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_each_ignore_on_an_os_outside_its_case_and_names_what_it_cannot_read() {
        let skips = "crates/a/src";
        let mut expected = vec![
            refused("bench/b/src/lib.rs", "b"),
            format!(
                "`{skips}/arity.rs`: `a`: not(unix, windows)\n^^^^^^^^^^^^^^^^^^ not() \
                 takes 1 predicate, found 2"
            ),
            refused(&format!("{skips}/everywhere.rs"), "a"),
            refused(&format!("{skips}/inline_dir/inner.rs"), "a"),
            refused("crates/a/src/lib.rs", "a"),
            "`crates/a/src/lib.rs`: `chosen`: a `path` in a `cfg_attr`".into(),
            "`crates/a/src/lib.rs`: `missing`: no file for the module `missing`".into(),
            format!("`{skips}/list.rs`: `b::a`: `ignore` takes no list"),
            format!("`{skips}/literal.rs`: `a`: a value that is no string literal"),
            format!("`{skips}/parse.rs`: cannot parse string into token stream"),
            refused(&format!("{skips}/pathed_file.rs"), "a"),
        ];
        let reasons_rs = format!("{skips}/reasons.rs");
        expected.extend(["comma", "or", "slash"].map(|i| refused(&reasons_rs, i)));
        let items = [
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n::o",
            "n::p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
        ];
        let refused_rs = format!("{skips}/refused.rs");
        expected.extend(items.map(|i| refused(&refused_rs, i)));
        expected.extend([
            format!(
                "`{skips}/target.rs`: `a`: a `cfg` predicate with no known value on \
                 `x86_64-pc-windows-msvc`"
            ),
            refused(&format!("{skips}/type.rs"), "a"),
            refused(&format!("{skips}/unix.rs"), "b"),
            format!(
                "`{skips}/unknown.rs`: `a`: nany(unix)\n    ^ expected one of `=`, \
                 `,`, `)` here"
            ),
            refused(&format!("{skips}/windows.rs"), "a"),
            refused("oracles/a/common.rs", "ready_on_each_os"),
            refused("outside.rs", "on_each_os"),
        ]);
        assert_eq!(check(&crate::fixture().join("skips")), Err(expected));
    }

    #[test]
    fn names_the_oses_that_run_a_test() {
        assert!(gives(
            "only Linux and Windows: a case",
            &["Linux", "Windows"]
        ));
        assert!(gives("only Unix", &["Linux", "macOS"]));
        assert!(gives("only Linux and macOS: a case", &["Linux", "macOS"]));
        assert!(!gives("only Linux and Windows: a case", &["Linux"]));
        assert!(!gives("only Linux: a case", &["Linux", "Windows"]));
        assert!(!gives("only Linuxes", &["Linux"]));
    }
}
