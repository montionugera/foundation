//! The skips check. The lists of each OS in `ci.yaml` are the one record of what that
//! OS does not run (LINUX CI), so a test skips an OS by an attribute or a `cfg` only in
//! a case of `docs/claude/testing.md`, "No `#[ignore]`", and its reason says so.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cfg_expr::targets::get_builtin_target_by_triple;
use cfg_expr::{Expression, Predicate};
use syn::ext::IdentExt;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, Expr, Ident, Item, Lit, Meta, Token};

use crate::{field, files};

/// A runner of CI that runs the tests of the workspace: its target, its OS as a reason
/// names it, and the name of its entry in the matrix of the job `check`.
struct Runner {
    triple: &'static str,
    os: &'static str,
    entry: &'static str,
}

/// The runners of the lists of LINUX CI, one for each OS, in the order in which a
/// reason names their OSes.
const RUNNERS: [Runner; 3] = [
    Runner {
        triple: "x86_64-unknown-linux-gnu",
        os: "Linux",
        entry: "check",
    },
    Runner {
        triple: "aarch64-apple-darwin",
        os: "macOS",
        entry: "macos",
    },
    Runner {
        triple: "x86_64-pc-windows-msvc",
        os: "Windows",
        entry: "windows",
    },
];

/// A set of the runners of `RUNNERS`, one bit for each by its index.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Runners(u8);

impl Runners {
    const ALL: Self = Self((1 << RUNNERS.len()) - 1);
    const NONE: Self = Self(0);

    /// The runners on which `predicate`, the predicate of a `cfg`, holds in `cargo test
    /// --all-features`, and whether it names a target: `test`, `debug_assertions`, and
    /// each `feature` hold, and each other name that is not one of a target does not.
    fn holding(predicate: &Meta) -> Result<(Self, bool), String> {
        let expression =
            Expression::parse(&text(predicate)?).map_err(|e| e.to_string())?;
        let keyed = expression
            .predicates()
            .any(|predicate| matches!(predicate, Predicate::Target(_)));
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
        Ok((runners, keyed))
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
/// `root`. A test that its `cfg` builds on an OS of the lists, but that an `ignore`
/// skips on the runner of that OS, is refused, unless the reason of each such `ignore`
/// is `only <OS>` or starts with `only <OS>:`, where `<OS>` names the OSes that run the
/// test. An item whose `cfg` names a target and leaves out of an OS a test that no item
/// of the same path builds there is a gate: it is refused, unless the first line of its
/// doc gives the OSes on which it builds in the same way, with `Only` for `only`, or
/// the `gates` list of that OS in the job `check` of `.github/workflows/ci.yaml` names
/// its file and its path in the file. An item of a `gates` list that names no such gate
/// is refused too. In a target that `cargo test` runs with no harness, each function is
/// a test; elsewhere, a function with a `test` attribute, in the body of a macro call
/// too, where a `cfg` of the call is a gate of each test. The `ignore` attributes in
/// the body of a macro call are not read.
///
/// It also refuses a file that does not parse, a `mod` with no file, a `path` in a
/// `cfg_attr`, a `cfg` predicate that is not one of Rust or whose value on a runner is
/// not known, an `ignore` reason that is not a string literal, and a `ci.yaml` with no
/// `gates` list for an entry of the job `check`.
pub(crate) fn check(root: &Path) -> Result<(), Vec<String>> {
    let problems = problems(root).map_err(|e| vec![e])?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn problems(root: &Path) -> Result<Vec<String>, String> {
    let mut lists = lists(root)?;
    let mut modules = roots(root)?;
    let mut seen = BTreeSet::new();
    let mut problems = BTreeSet::new();
    let mut found = Found::default();
    while let Some(module) = modules.pop() {
        if !seen.insert((module.file.clone(), module.on, module.target)) {
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
        let tests = match module.tests {
            Tests::Root if file.items.iter().any(main) => Tests::Each,
            Tests::Root => Tests::Marked,
            tests => tests,
        };
        let mut items = Items {
            shown: &shown,
            file: &module.file,
            dir: module.dir,
            inline: 0,
            item: Vec::new(),
            on: module.on,
            target: module.target,
            module: module.path,
            gates: module.gates,
            tests,
            modules: &mut modules,
            found: &mut found,
            problems: &mut problems,
        };
        items.enter(&file.attrs, None, |items, _| items.visit_file(&file));
    }
    found.refuse(&mut lists, &mut problems);
    for (runner, list) in RUNNERS.iter().zip(lists) {
        for item in list {
            problems.insert(unlisted(&item, runner.os));
        }
    }
    Ok(problems.into_iter().collect())
}

/// The root module of each target of the workspace at `root`.
fn roots(root: &Path) -> Result<Vec<Module>, String> {
    let metadata = crate::metadata(root)?;
    let mut modules = Vec::new();
    for (package, value) in field::list(&metadata, "packages")?.iter().enumerate() {
        for (target, value) in field::list(value, "targets")?.iter().enumerate() {
            let file = files::normalize(Path::new(field::text(value, "src_path")?));
            let dir = file
                .parent()
                .ok_or_else(|| format!("{} has no directory", file.display()))?
                .to_path_buf();
            let kinds = field::list(value, "kind")?;
            let tests = if !field::flag(value, "test")? {
                Tests::None
            } else if kinds.iter().any(|k| k == "test" || k == "bench") {
                Tests::Root
            } else {
                Tests::Marked
            };
            modules.push(Module {
                file,
                dir,
                on: Runners::ALL,
                target: (package, target),
                path: Vec::new(),
                gates: Vec::new(),
                tests,
            });
        }
    }
    Ok(modules)
}

/// Reports whether `item`, an item of the root file of a target, makes the target a
/// program with no test harness.
fn main(item: &Item) -> bool {
    match item {
        Item::Fn(function) => function.sig.ident == "main",
        Item::Macro(call) => call
            .mac
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "criterion_main"),
        _ => false,
    }
}

/// Which functions of a module tree are tests.
#[derive(Clone, Copy)]
enum Tests {
    /// None: `cargo test` does not run the target.
    None,
    /// Those with a `test` attribute.
    Marked,
    /// Each one: the target has no test harness.
    Each,
    /// `Marked` or `Each`, by the root file of a test or bench target.
    Root,
}

/// A target of the workspace: the index of its package, and its index in the package.
type Target = (usize, usize);

/// A file of a module, the directory of the modules that it declares, the runners on
/// which it builds, its target, its path in the target, the gates around it, and which
/// of its functions are tests.
struct Module {
    file: PathBuf,
    dir: PathBuf,
    on: Runners,
    target: Target,
    path: Vec<String>,
    gates: Vec<usize>,
    tests: Tests,
}

/// An item whose `cfg` names a target and leaves out runners on which its parent
/// builds.
struct Gate {
    /// The item as a `gates` list names it: its file, and its path in the file.
    named: String,
    /// The runners that its `cfg` leaves out.
    out: Runners,
    /// The runners on which it builds.
    on: Runners,
    /// The first line of its doc.
    doc: Option<String>,
}

/// The gates and the tests of the workspace.
#[derive(Default)]
struct Found {
    gates: Vec<Gate>,
    /// Each item of each path of a test in its target.
    tests: BTreeMap<(Target, Vec<String>), Vec<Built>>,
}

/// An item of the path of a test: the runners on which it builds, and the gates around
/// it.
struct Built {
    on: Runners,
    gates: Vec<usize>,
}

impl Found {
    /// Refuses each gate that leaves a test out of an OS on which no item of its path
    /// builds, with no item of the `gates` list of that OS and no doc that gives the
    /// case. It takes each item that it finds out of `lists`.
    fn refuse(&self, lists: &mut [BTreeSet<String>], problems: &mut BTreeSet<String>) {
        let mut out = BTreeSet::new();
        for items in self.tests.values() {
            let on = items.iter().fold(Runners::NONE, |on, item| on.or(item.on));
            for item in items {
                for &gate in &item.gates {
                    let left = self.gates[gate].out.without(on);
                    out.extend(
                        (0..RUNNERS.len())
                            .filter(|i| left.0 >> i & 1 == 1)
                            .map(|i| (gate, i)),
                    );
                }
            }
        }
        for (gate, i) in out {
            let gate = &self.gates[gate];
            let given = gate
                .doc
                .as_deref()
                .and_then(|d| d.strip_prefix("Only "))
                .is_some_and(|d| gives(&format!("only {d}"), &gate.on.oses()));
            if !lists[i].remove(&gate.named) && !given {
                problems.insert(gated(&gate.named, RUNNERS[i].os));
            }
        }
    }
}

/// What the attributes of an item do: the runners on which it builds, those on which
/// each `cfg` that names a target builds it, and each `ignore`, with the runners on
/// which it applies and its reason.
struct Effect {
    builds: Runners,
    keyed: Runners,
    ignores: Vec<Ignore>,
}

struct Ignore {
    on: Runners,
    /// A `cfg_attr` around it names a target.
    keyed: bool,
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
    /// The path of the item in the file.
    item: Vec<String>,
    /// The runners on which the item builds.
    on: Runners,
    target: Target,
    /// The path of the file in its target.
    module: Vec<String>,
    /// The gates around the item.
    gates: Vec<usize>,
    tests: Tests,
    modules: &'a mut Vec<Module>,
    found: &'a mut Found,
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
        self.item.extend(name.map(|name| name.unraw().to_string()));
        match effect(attributes) {
            Ok(effect) => {
                let on = self.on.and(effect.builds);
                let out = self.on.without(effect.keyed);
                if out != Runners::NONE {
                    let item = self.item.join("::");
                    self.gates.push(self.found.gates.len());
                    self.found.gates.push(Gate {
                        named: [self.shown, &item].join(" ").trim_end().to_string(),
                        out,
                        on,
                        doc: doc(attributes),
                    });
                }
                if on != Runners::NONE {
                    let outer = std::mem::replace(&mut self.on, on);
                    visit(self, &effect.ignores);
                    self.on = outer;
                }
                if out != Runners::NONE {
                    self.gates.pop();
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

    /// Visits the function with `attributes`, named `name`: it records the function
    /// when it is a test, and refuses each of its `ignore` attributes that skips an OS
    /// outside a case. A test that its `ignore` attributes skip on each OS on which it
    /// builds has no OS for a reason to name, unless a plain `#[ignore]` skips it on
    /// every OS.
    fn function(
        &mut self,
        attributes: &[Attribute],
        name: &Ident,
        visit: impl FnOnce(&mut Self),
    ) {
        self.enter(attributes, Some(name), |s, ignores| {
            s.record(attributes);
            s.test(ignores);
            visit(s);
        });
    }

    /// Records the item as a test, when `attributes` are those of a test function.
    fn record(&mut self, attributes: &[Attribute]) {
        let test = match self.tests {
            Tests::None => false,
            Tests::Root => unreachable!(
                "invariant: the root file of a target sets `Each` or `Marked`"
            ),
            Tests::Marked => attributes
                .iter()
                .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "test")),
            Tests::Each => true,
        };
        if test {
            let path = [self.module.as_slice(), &self.item].concat();
            self.found
                .tests
                .entry((self.target, path))
                .or_default()
                .push(Built {
                    on: self.on,
                    gates: self.gates.clone(),
                });
        }
    }

    fn test(&mut self, ignores: &[Ignore]) {
        if ignores
            .iter()
            .any(|ignore| !ignore.keyed && ignore.on == Runners::ALL)
        {
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
                    target: s.target,
                    path: [s.module.as_slice(), &s.item].concat(),
                    gates: s.gates.clone(),
                    tests: s.tests,
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

    /// A `cfg` of the call is a gate of each function in its body, by its name.
    fn visit_item_macro(&mut self, i: &'ast syn::ItemMacro) {
        match i.mac.parse_body_with(functions) {
            Ok(functions) => {
                for (attributes, name) in functions {
                    let attributes = [i.attrs.as_slice(), &attributes].concat();
                    self.enter(&attributes, Some(&name), |s, _| s.record(&attributes));
                }
            }
            Err(e) => self.problem(&e.to_string()),
        }
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.function(&i.attrs, &i.sig.ident, |s| syn::visit::visit_item_fn(s, i));
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.function(&i.attrs, &i.sig.ident, |s| {
            syn::visit::visit_impl_item_fn(s, i);
        });
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.function(&i.attrs, &i.sig.ident, |s| {
            syn::visit::visit_trait_item_fn(s, i);
        });
    }
}

/// The functions with an attribute at the top level of the body of a macro call, as
/// `proptest!` takes them: their attributes and their names. Each other token is
/// passed.
fn functions(input: ParseStream<'_>) -> syn::Result<Vec<(Vec<Attribute>, Ident)>> {
    let mut functions = Vec::new();
    while !input.is_empty() {
        if input.peek(Token![#]) && input.peek2(syn::token::Bracket) {
            let attributes = input.call(Attribute::parse_outer)?;
            if input.peek(Token![fn]) {
                input.parse::<Token![fn]>()?;
                functions.push((attributes, input.call(Ident::parse_any)?));
            }
            continue;
        }
        input.step(|cursor| {
            cursor
                .token_tree()
                .map(|(_, next)| ((), next))
                .ok_or_else(|| cursor.error("the end of the body"))
        })?;
    }
    Ok(functions)
}

/// The first line of the doc in `attributes`.
fn doc(attributes: &[Attribute]) -> Option<String> {
    attributes
        .iter()
        .find(|a| a.path().is_ident("doc"))
        .and_then(|a| match &a.meta {
            Meta::NameValue(pair) => string(&pair.value).ok(),
            _ => None,
        })
        .map(|doc| doc.lines().next().unwrap_or_default().trim().to_string())
}

/// The items of the `gates` list of each entry of the matrix of the job `check` in
/// `.github/workflows/ci.yaml`, in the order of `RUNNERS`, with no comment.
fn lists(root: &Path) -> Result<Vec<BTreeSet<String>>, String> {
    let path = root.join(".github/workflows/ci.yaml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let indent = |line: &str| line.len() - line.trim_start().len();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let job = lines
        .iter()
        .position(|line| *line == "  check:")
        .ok_or("`ci.yaml` has no job `check`")?;
    let mut lists: Vec<Option<BTreeSet<String>>> = vec![None; RUNNERS.len()];
    let (mut entry, mut list) = (None, None);
    for line in lines[job + 1..].iter().take_while(|line| indent(line) > 2) {
        let text = line.trim();
        if let Some(name) = text.strip_prefix("- name: ") {
            entry = RUNNERS.iter().position(|runner| runner.entry == name);
            list = None;
        } else if list.is_some_and(|key| indent(line) > key) {
            if let (Some(i), Some(item)) = (entry, text.strip_prefix("- ")) {
                let item = item.split_once(" #").map_or(item, |(item, _)| item);
                lists[i]
                    .get_or_insert_default()
                    .insert(item.trim().to_string());
            }
        } else if let (Some(i), true) = (entry, text.starts_with("gates:")) {
            lists[i].get_or_insert_default();
            list = Some(indent(line));
        } else {
            list = None;
        }
    }
    RUNNERS
        .iter()
        .zip(lists)
        .map(|(runner, list)| {
            list.ok_or_else(|| {
                format!(
                    "`ci.yaml` has no `gates` list in the entry `{}` of the job \
                     `check`",
                    runner.entry
                )
            })
        })
        .collect()
}

/// What `attributes` do. It reads into each `cfg_attr`.
fn effect(attributes: &[Attribute]) -> Result<Effect, String> {
    let mut effect = Effect {
        builds: Runners::ALL,
        keyed: Runners::ALL,
        ignores: Vec::new(),
    };
    apply(
        attributes.iter().map(|a| &a.meta),
        Runners::ALL,
        false,
        &mut effect,
    )?;
    Ok(effect)
}

/// Adds to `effect` what each of `attributes` does on the runners of `on`, inside a
/// `cfg_attr` that names a target when `keyed`.
fn apply<'a>(
    attributes: impl IntoIterator<Item = &'a Meta>,
    on: Runners,
    keyed: bool,
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
            let (holds, named) = Runners::holding(&predicate)?;
            apply(&inner, on.and(holds), keyed || named, effect)?;
        } else if path.is_ident("cfg") {
            let predicate = meta
                .require_list()
                .and_then(syn::MetaList::parse_args::<Meta>)
                .map_err(|e| e.to_string())?;
            let (holds, named) = Runners::holding(&predicate)?;
            let fails = on.without(holds);
            effect.builds = effect.builds.without(fails);
            if keyed || named {
                effect.keyed = effect.keyed.without(fails);
            }
        } else if path.is_ident("ignore") {
            let reason = match meta {
                Meta::Path(_) => None,
                Meta::NameValue(pair) => Some(string(&pair.value)?),
                Meta::List(_) => {
                    return Err("`ignore` takes no list".to_string());
                }
            };
            effect.ignores.push(Ignore { on, keyed, reason });
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

fn gated(named: &str, os: &str) -> String {
    format!(
        "`{named}`: a `cfg` leaves a test out of {os} outside the lists of LINUX CI. \
         Fix: add `- {named} # #<issue>` to the `gates` list of {os} in `ci.yaml`, \
         or, in a case of `docs/claude/testing.md`, \"No `#[ignore]`\", make the \
         first line of the doc of the item `Only <OS>` or start it with `Only <OS>:`, \
         where <OS> names each OS on which the item builds, as the reason of an \
         `ignore` does."
    )
}

fn unlisted(item: &str, os: &str) -> String {
    format!(
        "`ci.yaml`: the item `{item}` of the `gates` list of {os} names no `cfg` that \
         leaves a test out of {os}. Fix: remove it."
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
            refused(&format!("{skips}/everywhere.rs"), "b"),
            refused(&format!("{skips}/everywhere.rs"), "c"),
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
    fn refuses_each_gate_that_no_list_or_doc_gives_and_each_unused_list_item() {
        let lib = "crates/g/src/lib.rs";
        let mut expected = vec![
            gated(&format!("{lib} refused"), "Windows"),
            gated(&format!("{lib} misnamed"), "Windows"),
            gated(&format!("{lib} generated"), "Linux"),
            gated(&format!("{lib} generated"), "Windows"),
            gated("crates/g/src/module.rs inner", "Linux"),
            gated("crates/g/src/module.rs inner", "macOS"),
            gated("crates/g/tests/free.rs case", "Windows"),
            gated("crates/g/tests/filed.rs", "Windows"),
            gated("crates/g/benches/timed.rs case", "Windows"),
            "`ci.yaml`: the item `crates/g/src/lib.rs stale` of the `gates` list of \
             Windows names no `cfg` that leaves a test out of Windows. Fix: remove it."
                .into(),
        ];
        expected.sort();
        assert_eq!(check(&crate::fixture().join("gates")), Err(expected));
    }

    #[test]
    fn refuses_a_ci_yaml_with_no_gates_list_for_an_entry() {
        assert_eq!(
            lists(&crate::fixture().join("gates/missing")),
            Err(
                "`ci.yaml` has no `gates` list in the entry `windows` of the job \
                 `check`"
                    .into()
            )
        );
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
