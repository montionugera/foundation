//! Runs Miri on the pinned nightly.

use std::collections::BTreeSet;
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

/// A part of the Miri tests for one CI job: of the tests of each pass in order, each
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

/// What one unit of a crate's tests runs.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    /// A test of the lib target, by its name.
    Test(String),
    /// Each test of another target, by the arguments that select the target. A run
    /// of it takes no filter: a target with no harness ignores the filter.
    Target(Vec<String>),
}

/// A part of one pass: its `MIRIFLAGS`, its crate, and the part.
type Unit<'a> = (&'a str, &'a select::Package, &'a Part);

/// The problem of `run`, a Miri run with `flags` of tests of the crate `name`, if any.
fn problem(run: &Run, flags: &str, name: &str) -> Option<String> {
    match run {
        Run::Failed => Some(format!("Miri with `{flags}` failed in `{name}`")),
        // Each test that a run names is listed and not ignored.
        Run::Empty => Some(format!(
            "Miri with `{flags}` ran none of the tests that it names in `{name}`"
        )),
        Run::Passed => None,
    }
}

/// Runs `cargo miri test` on the units of `shard`, of each pass in [`PASSES`] and each
/// crate of [`packages`], one at a time on each core: a core takes the next unit that
/// no core took. A unit is a test of a lib target, or each other test target whole.
/// It uses rustup and the nightly in `rust-toolchain-nightly`. It fails when a test
/// fails, when a run of a lib test runs none, or when a crate has no test that Miri
/// runs.
pub(crate) fn run(root: &Path, shard: Shard) -> Result<(), Vec<String>> {
    let nightly = crate::nightly::Toolchain::read(root).map_err(|e| vec![e])?;
    let metadata = crate::metadata(root).map_err(|e| vec![e])?;
    let packages = packages(&metadata).map_err(|e| vec![e])?;
    if packages.is_empty() {
        eprintln!("no crate names `unsafe_code`, so Miri has nothing to check");
        return Ok(());
    }
    let mut problems = Vec::new();
    let mut parts = Vec::new();
    for package in &packages {
        let (lib, targets) = targets(find(&metadata, package).map_err(|e| vec![e])?)
            .map_err(|e| vec![e])?;
        let tests = if lib {
            listed(&nightly, package).map_err(|e| vec![e])?
        } else {
            BTreeSet::new()
        };
        if tests.is_empty() && targets.is_empty() {
            problems.push(format!(
                "`{}` names `unsafe_code` but runs no tests under Miri. Add tests that \
                 reach its unsafe code.",
                package.name
            ));
        }
        let crate_parts = (tests.into_iter().map(Part::Test))
            .chain(targets.into_iter().map(Part::Target));
        parts.extend(crate_parts.map(|part| (package, part)));
    }
    let all = (PASSES.into_iter())
        .flat_map(|flags| parts.iter().map(move |(p, part)| (flags, *p, part)));
    let units: Vec<Unit<'_>> = shard.pick(all);
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
        let run = match part {
            Part::Test(test) => {
                libtest::run(command.args(["--lib", "--", "--exact", test]))?
            }
            // A target with no harness gives no count of the tests that it ran.
            Part::Target(args) => match libtest::run(command.args(args))? {
                Run::Empty => Run::Passed,
                run => run,
            },
        };
        problems.extend(problem(&run, flags, &package.name));
    }
    Ok(problems)
}

/// The package object of `package` in `metadata`.
fn find<'a>(
    metadata: &'a Value,
    package: &select::Package,
) -> Result<&'a Value, String> {
    (field::list(metadata, "packages")?.iter())
        .find(|p| p["id"] == package.id.as_str())
        .ok_or_else(|| format!("`cargo metadata` has no package `{}`", package.id))
}

/// Whether the lib target of `package`, a package object of `cargo metadata`, has
/// tests, and the arguments that select each other target that `cargo test` runs.
fn targets(package: &Value) -> Result<(bool, Vec<Vec<String>>), String> {
    let (mut lib, mut others) = (false, Vec::new());
    for target in field::list(package, "targets")? {
        let kind = field::list(target, "kind")?.first().and_then(Value::as_str);
        let tested = field::flag(target, "test")?;
        match kind {
            Some(kind @ ("test" | "bin" | "example")) => {
                if tested {
                    let name = field::text(target, "name")?.to_string();
                    others.push(vec![format!("--{kind}"), name]);
                }
            }
            Some("bench" | "custom-build") => {}
            _ => {
                lib = tested;
                if field::flag(target, "doctest")? {
                    others.push(vec!["--doc".to_string()]);
                }
            }
        }
    }
    Ok((lib, others))
}

/// The tests of the lib target of `package` that Miri runs: each that it lists and
/// does not ignore.
fn listed(
    nightly: &crate::nightly::Toolchain,
    package: &select::Package,
) -> Result<BTreeSet<String>, String> {
    let list = |args: &[&str]| {
        let mut command = command(nightly, package, PASSES[0]);
        command
            .args(["--lib", "--", "--list", "--format", "terse"])
            .args(args);
        let output = (command.stderr(Stdio::inherit()).output())
            .map_err(|e| format!("{}: {e}", command.get_program().to_string_lossy()))?;
        if !output.status.success() {
            return Err(format!(
                "Miri could not list the tests of `{}`",
                package.name
            ));
        }
        Ok(names(&String::from_utf8_lossy(&output.stdout)))
    };
    Ok(&list(&[])? - &list(&["--ignored"])?)
}

/// The name of each test in the output of libtest `--list --format terse`.
fn names(output: &str) -> BTreeSet<String> {
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
    fn problem_names_each_run_that_does_not_pass() {
        let problem = |run| problem(&run, "-Zmiri-x", "model");
        assert_eq!(
            problem(Run::Failed),
            Some("Miri with `-Zmiri-x` failed in `model`".to_string())
        );
        assert_eq!(
            problem(Run::Empty),
            Some(
                "Miri with `-Zmiri-x` ran none of the tests that it names in `model`"
                    .to_string()
            )
        );
        assert_eq!(problem(Run::Passed), None);
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

    #[test]
    fn targets_gives_whether_the_lib_has_tests_and_each_other_target_that_runs() {
        let target = |kind: &str, name: &str, test: bool| json!({ "kind": [kind], "name": name, "test": test, "doctest": kind == "lib" });
        let package = json!({ "targets": [
            target("lib", "a", true),
            target("test", "it", true),
            target("test", "off", false),
            target("bin", "tool", true),
            target("example", "demo", true),
            target("bench", "fast", true),
            target("custom-build", "build-script-build", false),
        ]});
        let args =
            |args: &[&str]| args.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            targets(&package),
            Ok((
                true,
                vec![
                    args(&["--doc"]),
                    args(&["--test", "it"]),
                    args(&["--bin", "tool"]),
                    args(&["--example", "demo"]),
                ]
            ))
        );
        let untested = json!({ "targets": [
            json!({ "kind": ["lib"], "name": "a", "test": false, "doctest": false }),
        ]});
        assert_eq!(targets(&untested), Ok((false, vec![])));
    }

    #[test]
    fn find_gives_the_package_object_by_its_id() {
        let metadata = crate::metadata(&crate::fixture()).unwrap();
        let model = &packages(&metadata).unwrap()[0];
        assert_eq!(find(&metadata, model).unwrap()["name"], "model");
        let missing = select::Package {
            id: "path+file:///w/none#0.0.0".to_string(),
            name: "none".to_string(),
        };
        assert_eq!(
            find(&metadata, &missing),
            Err(
                "`cargo metadata` has no package `path+file:///w/none#0.0.0`"
                    .to_string()
            )
        );
    }

    #[test]
    fn names_reads_each_test_of_a_terse_list() {
        let output = "a::b: test\nsrc/lib.rs - c (line 3): test\nd: bench\n\n2 tests\n";
        assert_eq!(
            names(output),
            BTreeSet::from(["a::b".to_string(), "src/lib.rs - c (line 3)".to_string()])
        );
    }
}
