//! Runs Miri on the pinned nightly.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use serde_json::Value;

use crate::libtest::{self, Run};
use crate::{field, select};

/// The `MIRIFLAGS` of each Miri pass.
const PASSES: [&str; 2] = [
    "-Zmiri-strict-provenance",
    "-Zmiri-strict-provenance -Zmiri-tree-borrows",
];

/// Crates whose `unsafe` code only calls the OS or C, which Miri cannot run. Tests on
/// the real OS check `os` (BLOCK MEMORY), and `cargo xtask sanitizers` checks
/// `connector-opcua`.
pub(crate) const SKIPPED: [&str; 2] = ["os", crate::sanitizers::CRATE];

/// The workspace crates whose source names `unsafe_code`, the lint that each `unsafe`
/// use must expect, except those in [`SKIPPED`].
pub(crate) fn packages(metadata: &Value) -> Result<Vec<select::Package>, String> {
    let mut packages =
        select::packages(metadata, |s| select::has_word(s, "unsafe_code"))?;
    packages.retain(|package| !SKIPPED.contains(&package.name.as_str()));
    Ok(packages)
}
/// A part of the Miri tests for one CI job: of the units in order, each `count`th one
/// from `index`.
/// `count`th one from `index`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shard {
    index: usize,
    count: usize,
}

impl Shard {
    /// Each test.
    pub(crate) const ALL: Self = Self { index: 0, count: 1 };

    /// Reads `<index>/<count>`, the form of `cargo mutants --shard`.
    ///
    /// # Errors
    ///
    /// When `text` has another form, or `index` is not less than `count`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let shard = text.split_once('/').and_then(|(index, count)| {
            Some(Self {
                index: index.parse().ok()?,
                count: count.parse().ok()?,
            })
        });
        shard
            .filter(|shard| shard.index < shard.count)
            .ok_or_else(|| {
                format!("`{text}` is not a shard `<index>/<count>` with index < count")
            })
    }

    /// The items of this shard, in order: each `count`th one from `index`.
    fn pick<T>(self, items: impl IntoIterator<Item = T>) -> Vec<T> {
        (items.into_iter().skip(self.index))
            .step_by(self.count)
            .collect()
    }
}

/// The names of tests.
type Tests = BTreeSet<String>;

/// What one unit of a crate's tests runs.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    /// A test of the lib target, by its name.
    Test(String),
    /// The doc tests of the lib target.
    Doc,
    /// Each test of a target of `kind` (`test`, `bin`, `example`, or `bench`), by its
    /// name. A run of it takes no filter: a target with no harness ignores the filter.
    Target { kind: String, name: String },
}

impl Part {
    /// The arguments of `cargo miri test` that run this part.
    fn args(&self) -> Vec<String> {
        match self {
            Self::Test(test) => {
                ["--lib", "--", "--exact", test].map(String::from).into()
            }
            Self::Doc => vec!["--doc".to_string()],
            Self::Target { kind, name } => vec![format!("--{kind}"), name.clone()],
        }
    }

    /// How the run of this part ends for its problem: a target with no harness gives
    /// no count of the tests that it ran, so an empty run of a target passes.
    fn outcome(&self, run: Run) -> Run {
        match (self, run) {
            (Self::Target { .. }, Run::Empty) => Run::Passed,
            (_, run) => run,
        }
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Test(test) => write!(f, "the test `{test}`"),
            Self::Doc => f.write_str("the doc tests"),
            Self::Target { kind, name } => write!(f, "the {kind} target `{name}`"),
        }
    }
}

/// A part of one pass: its `MIRIFLAGS`, its crate, and the part.
type Unit<'a> = (&'a str, &'a select::Package, &'a Part);

/// A crate of [`packages`], with the targets of its tests.
#[derive(Debug, PartialEq)]
struct Crate {
    package: select::Package,
    /// Whether its lib target has tests, and whether it has doc tests.
    lib: bool,
    doc: bool,
    /// A [`Part::Target`] for each other target that `cargo test` runs.
    targets: Vec<Part>,
}

impl Crate {
    /// Reads a package object of `cargo metadata`.
    ///
    /// # Errors
    ///
    /// A missing field.
    fn read(package: &Value) -> Result<Self, String> {
        let mut read = Self {
            package: select::Package::read(package)?,
            lib: false,
            doc: false,
            targets: Vec::new(),
        };
        for target in field::list(package, "targets")? {
            let kind = field::list(target, "kind")?.first().and_then(Value::as_str);
            let tested = field::flag(target, "test")?;
            match kind {
                Some(kind @ ("test" | "bin" | "example" | "bench")) => {
                    if tested {
                        read.targets.push(Part::Target {
                            kind: kind.to_string(),
                            name: field::text(target, "name")?.to_string(),
                        });
                    }
                }
                Some("custom-build") => {}
                _ => {
                    read.lib = tested;
                    read.doc = field::flag(target, "doctest")?;
                }
            }
        }
        Ok(read)
    }

    /// The parts of its tests that Miri runs, where `list` gives the tests of the
    /// target that its argument selects (`--lib` or `--doc`) that Miri runs.
    ///
    /// # Errors
    ///
    /// An error of `list`.
    fn parts(
        self,
        mut list: impl FnMut(&select::Package, &str) -> Result<Tests, String>,
    ) -> Result<(select::Package, Vec<Part>), String> {
        let mut parts = Vec::new();
        if self.lib {
            parts.extend(list(&self.package, "--lib")?.into_iter().map(Part::Test));
        }
        if self.doc && !list(&self.package, "--doc")?.is_empty() {
            parts.push(Part::Doc);
        }
        parts.extend(self.targets);
        Ok((self.package, parts))
    }
}

/// Each crate of [`packages`], in the order of `metadata`.
fn crates(metadata: &Value) -> Result<Vec<Crate>, String> {
    let picked = packages(metadata)?;
    (field::list(metadata, "packages")?.iter())
        .filter(|package| picked.iter().any(|p| package["id"] == p.id.as_str()))
        .map(Crate::read)
        .collect()
}

/// The units of `shard`, and a problem for each crate of `crates` with no part. Each
/// pass of a part comes after the one before it, so the passes of a slow test fall in
/// different shards.
fn plan(
    crates: &[(select::Package, Vec<Part>)],
    shard: Shard,
) -> (Vec<Unit<'_>>, Vec<String>) {
    let problems = (crates.iter())
        .filter(|(_, parts)| parts.is_empty())
        .map(|(package, _)| {
            format!(
                "`{}` names `unsafe_code` but runs no tests under Miri. Add tests that \
                 reach its unsafe code.",
                package.name
            )
        })
        .collect();
    let all = crates.iter().flat_map(|(package, parts)| {
        (parts.iter()).flat_map(move |part| PASSES.map(|flags| (flags, package, part)))
    });
    (shard.pick(all), problems)
}

/// The problem of `run`, a Miri run with `flags` of `part` of the crate `name`, if
/// any.
fn problem(run: &Run, flags: &str, name: &str, part: &Part) -> Option<String> {
    match run {
        Run::Failed => {
            Some(format!("Miri with `{flags}` failed on {part} of `{name}`"))
        }
        // Each test that a part names is listed and not ignored.
        Run::Empty => Some(format!(
            "Miri with `{flags}` ran none of {part} of `{name}`"
        )),
        Run::Passed => None,
    }
}

/// Runs `cargo miri test` on the units of `shard`, of each pass in [`PASSES`] and each
/// crate of [`packages`], one at a time on each core: a core takes the next unit that
/// no core took. A unit is a test of a lib target, the doc tests, or each other test
/// target whole. It uses rustup and the nightly in `rust-toolchain-nightly`. It fails
/// when a test fails, when a run of a lib test or of the doc tests runs none, or when
/// a crate has no test that Miri runs.
pub(crate) fn run(root: &Path, shard: Shard) -> Result<(), Vec<String>> {
    let nightly = crate::nightly::Toolchain::read(root).map_err(|e| vec![e])?;
    let metadata = crate::metadata(root).map_err(|e| vec![e])?;
    let crates = crates(&metadata).map_err(|e| vec![e])?;
    if crates.is_empty() {
        eprintln!("no crate names `unsafe_code`, so Miri has nothing to check");
        return Ok(());
    }
    let crates = (crates.into_iter())
        .map(|found| found.parts(|package, target| listed(&nightly, package, target)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| vec![e])?;
    let (units, mut problems) = plan(&crates, shard);
    let next = AtomicUsize::new(0);
    #[expect(clippy::disallowed_methods, reason = "a dev tool uses each core")]
    let workers = thread::available_parallelism().map_or(1, usize::from);
    let runs: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                #[expect(
                    clippy::disallowed_methods,
                    reason = "a dev tool uses each core"
                )]
                scope.spawn(|| worker(&nightly, &units, &next))
            })
            .collect();
        (handles.into_iter())
            .map(|handle| {
                handle
                    .join()
                    .expect("invariant: a Miri worker never panics")
            })
            .collect()
    });
    for run in runs {
        problems.extend(run.map_err(|e| vec![e])?);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Runs each unit of `units` from the index `next` that no other worker takes, one
/// at a time, and gives the problem of each run.
fn worker(
    nightly: &crate::nightly::Toolchain,
    units: &[Unit<'_>],
    next: &AtomicUsize,
) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    while let Some(&(flags, package, part)) =
        units.get(next.fetch_add(1, Ordering::Relaxed))
    {
        let mut command = command(nightly, package, flags);
        let run = part.outcome(libtest::run(command.args(part.args()))?);
        problems.extend(problem(&run, flags, &package.name, part));
    }
    Ok(problems)
}

/// The tests that Miri runs of the target of `package` that `target` selects
/// (`--lib` or `--doc`): each that it lists and does not ignore.
fn listed(
    nightly: &crate::nightly::Toolchain,
    package: &select::Package,
    target: &str,
) -> Result<Tests, String> {
    let list = |args: &[&str]| {
        let mut command = command(nightly, package, PASSES[0]);
        command
            .args([target, "--", "--list", "--format", "terse"])
            .args(args);
        let output = (command.stderr(Stdio::inherit()).output())
            .map_err(|e| format!("{}: {e}", command.get_program().to_string_lossy()))?;
        if !output.status.success() {
            return Err(format!(
                "Miri could not list the tests of `{target}` in `{}`",
                package.name
            ));
        }
        Ok(names(&String::from_utf8_lossy(&output.stdout)))
    };
    Ok(&list(&[])? - &list(&["--ignored"])?)
}

/// The name of each test in the output of libtest `--list --format terse`.
fn names(output: &str) -> Tests {
    (output.lines())
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_string)
        .collect()
}

/// The command that runs Miri with `flags` on `package`, on the toolchain `nightly`.
fn command(
    nightly: &crate::nightly::Toolchain,
    package: &select::Package,
    flags: &str,
) -> Command {
    let mut command = nightly.cargo();
    command
        .args(["miri", "test", "-p", &package.id])
        .env("MIRIFLAGS", flags);
    command
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn packages_are_the_crates_that_name_unsafe_code() {
        let metadata = crate::metadata(&crate::fixture()).unwrap();
        let picked = packages(&metadata).unwrap();
        let names: Vec<_> = picked.into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["model"]);
    }

    #[test]
    fn selects_the_package_by_its_id() {
        let package = select::Package {
            id: "path+file:///w/crates/model#0.0.0".to_string(),
            name: "model".to_string(),
        };
        let nightly = crate::nightly::Toolchain::new("/w", "nightly-x");
        let command = command(&nightly, &package, PASSES[0]);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "run",
                "nightly-x",
                "cargo",
                "miri",
                "test",
                "-p",
                &package.id
            ]
        );
    }

    #[test]
    fn a_shard_is_an_index_less_than_its_count() {
        assert_eq!(Shard::parse("7/8"), Ok(Shard { index: 7, count: 8 }));
        for text in ["8/8", "0/0", "-1/8", "a/8", "3", "3/8/9"] {
            assert_eq!(
                Shard::parse(text),
                Err(format!(
                    "`{text}` is not a shard `<index>/<count>` with index < count"
                ))
            );
        }
    }

    #[test]
    fn a_shard_picks_each_count_th_item_from_its_index() {
        let shard = |index| Shard { index, count: 3 };
        assert_eq!(shard(1).pick(0..10), [1, 4, 7]);
        assert_eq!(shard(0).pick(0..10), [0, 3, 6, 9]);
        assert_eq!(Shard::ALL.pick(0..3), [0, 1, 2]);
        assert_eq!(shard(2).pick(0..2), Vec::<i32>::new());
    }

    fn model() -> select::Package {
        select::Package {
            id: "path+file:///w/crates/model#0.0.0".to_string(),
            name: "model".to_string(),
        }
    }

    fn target(kind: &str, name: &str) -> Part {
        Part::Target {
            kind: kind.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn problem_names_each_run_that_does_not_pass_and_its_part() {
        let problem = |run, part| problem(&run, "-Zmiri-x", "model", &part);
        assert_eq!(
            problem(Run::Failed, Part::Test("a::b".to_string())),
            Some(
                "Miri with `-Zmiri-x` failed on the test `a::b` of `model`".to_string()
            )
        );
        assert_eq!(
            problem(Run::Failed, target("test", "it")),
            Some(
                "Miri with `-Zmiri-x` failed on the test target `it` of `model`"
                    .to_string()
            )
        );
        assert_eq!(
            problem(Run::Empty, Part::Doc),
            Some(
                "Miri with `-Zmiri-x` ran none of the doc tests of `model`".to_string()
            )
        );
        assert_eq!(problem(Run::Passed, Part::Doc), None);
    }

    #[test]
    fn a_part_gives_the_arguments_that_run_it() {
        assert_eq!(
            Part::Test("a::b".to_string()).args(),
            ["--lib", "--", "--exact", "a::b"]
        );
        assert_eq!(Part::Doc.args(), ["--doc"]);
        assert_eq!(target("bench", "fast").args(), ["--bench", "fast"]);
    }

    #[test]
    fn only_an_empty_run_of_a_target_passes() {
        let test = Part::Test("a".to_string());
        assert_eq!(target("test", "it").outcome(Run::Empty), Run::Passed);
        assert_eq!(target("test", "it").outcome(Run::Failed), Run::Failed);
        assert_eq!(test.outcome(Run::Empty), Run::Empty);
        assert_eq!(test.outcome(Run::Passed), Run::Passed);
        assert_eq!(Part::Doc.outcome(Run::Empty), Run::Empty);
    }

    #[test]
    fn a_crate_reads_its_lib_its_doc_tests_and_each_other_target_that_runs() {
        let target_object = |kind: &str, name: &str, test: bool| {
            let doctest = kind == "lib";
            json!({ "kind": [kind], "name": name, "test": test, "doctest": doctest })
        };
        let package = |targets: Vec<Value>| {
            let id = model().id;
            json!({ "id": id, "name": "model", "targets": targets })
        };
        let tested = package(vec![
            target_object("lib", "a", true),
            target_object("test", "it", true),
            target_object("test", "off", false),
            target_object("bin", "tool", true),
            target_object("example", "demo", true),
            target_object("bench", "fast", true),
            target_object("bench", "slow", false),
            target_object("custom-build", "build-script-build", false),
        ]);
        assert_eq!(
            Crate::read(&tested),
            Ok(Crate {
                package: model(),
                lib: true,
                doc: true,
                targets: vec![
                    target("test", "it"),
                    target("bin", "tool"),
                    target("example", "demo"),
                    target("bench", "fast"),
                ],
            })
        );
        let untested = package(vec![
            json!({ "kind": ["lib"], "name": "a", "test": false, "doctest": false }),
        ]);
        assert_eq!(
            Crate::read(&untested),
            Ok(Crate {
                package: model(),
                lib: false,
                doc: false,
                targets: vec![],
            })
        );
    }

    #[test]
    fn crates_are_the_packages_with_their_targets() {
        let metadata = crate::metadata(&crate::fixture()).unwrap();
        let crates = crates(&metadata).unwrap();
        let names: Vec<_> = crates.iter().map(|c| c.package.name.as_str()).collect();
        assert_eq!(names, ["model"]);
        assert!(crates[0].lib, "the lib of `model` has tests");
    }

    #[test]
    fn a_crate_has_each_listed_test_its_doc_tests_when_listed_and_its_targets() {
        let found = |lib, doc| Crate {
            package: model(),
            lib,
            doc,
            targets: vec![target("test", "it")],
        };
        let mut asked = Vec::new();
        let mut list = |package: &select::Package, target: &str| {
            asked.push((package.name.clone(), target.to_string()));
            Ok(match target {
                "--lib" => ["b", "a"].map(String::from).into(),
                _ => BTreeSet::from(["src/lib.rs - c (line 3)".to_string()]),
            })
        };
        let parts = vec![
            Part::Test("a".to_string()),
            Part::Test("b".to_string()),
            Part::Doc,
            target("test", "it"),
        ];
        assert_eq!(found(true, true).parts(&mut list), Ok((model(), parts)));
        assert_eq!(
            found(false, false).parts(&mut list),
            Ok((model(), vec![target("test", "it")]))
        );
        let asked_for = |target: &str| ("model".to_string(), target.to_string());
        assert_eq!(asked, [asked_for("--lib"), asked_for("--doc")]);
        let none = found(false, true).parts(|_, _| Ok(BTreeSet::new()));
        assert_eq!(none, Ok((model(), vec![target("test", "it")])));
        let failed = found(true, false).parts(|_, _| Err("no list".to_string()));
        assert_eq!(failed, Err("no list".to_string()));
    }

    #[test]
    fn a_crate_with_no_test_that_miri_runs_is_a_problem() {
        let found = Crate {
            package: model(),
            lib: true,
            doc: true,
            targets: vec![],
        };
        let crates = [found.parts(|_, _| Ok(BTreeSet::new())).unwrap()];
        let (units, problems) = plan(&crates, Shard::ALL);
        assert_eq!(units, []);
        let problem = "`model` names `unsafe_code` but runs no tests under Miri. Add \
                       tests that reach its unsafe code.";
        assert_eq!(problems, [problem]);
    }

    #[test]
    fn a_plan_puts_the_passes_of_a_part_next_to_each_other() {
        let other = || select::Package {
            id: "path+file:///w/crates/other#0.0.0".to_string(),
            name: "other".to_string(),
        };
        let (a, it) = (Part::Test("a".to_string()), target("test", "it"));
        let crates = [
            (model(), vec![a.clone(), Part::Doc]),
            (other(), vec![it.clone()]),
        ];
        let (units, problems) = plan(&crates, Shard::ALL);
        assert_eq!(problems, Vec::<String>::new());
        let [first, second] = PASSES;
        assert_eq!(
            units,
            [
                (first, &model(), &a),
                (second, &model(), &a),
                (first, &model(), &Part::Doc),
                (second, &model(), &Part::Doc),
                (first, &other(), &it),
                (second, &other(), &it),
            ]
        );
        let (units, _) = plan(&crates, Shard { index: 1, count: 2 });
        assert_eq!(
            units,
            [
                (second, &model(), &a),
                (second, &model(), &Part::Doc),
                (second, &other(), &it),
            ]
        );
    }
}
