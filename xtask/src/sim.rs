//! Keeps each `sim` feature test-only: only a dev-dependency, the fuzz crate, or
//! another `sim` feature turns it on (rule 10 in `docs/decisions/crate-map.md`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::field;

const RULE: &str = "A `sim` feature is test-only: only a dev-dependency, the fuzz \
                    crate, or another `sim` feature turns it on. A product feature \
                    turns on `dep:sim` and `simulate` features. See rule 10 in \
                    docs/decisions/crate-map.md.";

/// The problems where a local crate of `graph`, the `cargo metadata` of a workspace
/// resolved with all features on, turns on a `sim` feature from a feature other than
/// `sim` or from a dependency that is not a dev-dependency. It checks each local crate
/// that a walk from the members reaches along edges that are not only dev-dependencies:
/// a member, also a tool or a benchmark, or a path crate outside the workspace. A
/// field that is missing or of another shape is a problem too.
pub(crate) fn check(graph: &Value) -> Vec<String> {
    let reached = match built(graph).and_then(|reached| packages(graph, reached)) {
        Ok(reached) => reached,
        Err(e) => return vec![e],
    };
    let mut problems = Vec::new();
    for package in reached {
        let found = local(package).and_then(|local| {
            if local {
                self::problems(package)
            } else {
                Ok(Vec::new())
            }
        });
        problems.extend(found.unwrap_or_else(|e| vec![e]));
    }
    problems
}

/// The package of `graph` for each id of `reached`.
fn packages<'a>(
    graph: &'a Value,
    reached: BTreeSet<&str>,
) -> Result<Vec<&'a Value>, String> {
    let mut packages = BTreeMap::new();
    for package in field::list(graph, "packages")? {
        packages.insert(field::text(package, "id")?, package);
    }
    reached
        .into_iter()
        .map(|id| {
            let package = packages.get(id).copied();
            package
                .ok_or_else(|| format!("JSON field `packages` has no package `{id}`"))
        })
        .collect()
}

/// The ids of the packages that a build of `graph` reaches: the members, and each
/// package on an edge from a reached one that is not only a dev-dependency.
fn built(graph: &Value) -> Result<BTreeSet<&str>, String> {
    let mut deps = BTreeMap::new();
    for node in field::list(&graph["resolve"], "nodes")? {
        deps.insert(field::text(node, "id")?, field::list(node, "deps")?);
    }
    let mut reached = BTreeSet::new();
    let mut next: Vec<&str> = field::list(graph, "workspace_members")?
        .iter()
        .map(|member| {
            member
                .as_str()
                .ok_or("JSON has a member that is not a string")
        })
        .collect::<Result<_, _>>()?;
    while let Some(id) = next.pop() {
        if !reached.insert(id) {
            continue;
        }
        let node = deps.get(id).copied();
        let node = node
            .ok_or_else(|| format!("JSON field `resolve.nodes` has no node `{id}`"))?;
        for dep in node {
            let kinds = field::list(dep, "dep_kinds")?;
            if kinds.is_empty() {
                return Err("JSON field `dep_kinds` is empty".to_string());
            }
            if kinds.iter().any(|kind| kind["kind"] != "dev") {
                next.push(field::text(dep, "pkg")?);
            }
        }
    }
    Ok(reached)
}

/// Whether `package` is a local crate: its `source` is null, where a registry or git
/// crate has a string.
fn local(package: &Value) -> Result<bool, String> {
    match &package["source"] {
        Value::Null => Ok(true),
        Value::String(_) => Ok(false),
        _ => Err(format!(
            "`{}`: JSON field `source` is neither null nor a string",
            package["name"].as_str().unwrap_or("?")
        )),
    }
}

/// The problems of one package of `cargo metadata`, as [`check`] states.
fn problems(package: &Value) -> Result<Vec<String>, String> {
    let name = field::text(package, "name")?;
    let features: BTreeMap<String, Vec<String>> =
        serde_json::from_value(package["features"].clone())
            .map_err(|e| format!("`{name}`: JSON field `features`: {e}"))?;
    let mut problems = Vec::new();
    for (feature, values) in features.iter().filter(|(feature, _)| *feature != "sim") {
        for value in values.iter().filter(|v| *v == "sim" || v.ends_with("/sim")) {
            problems.push(format!(
                "`{name}` turns on `{value}` in its feature `{feature}`. {RULE}"
            ));
        }
    }
    for dep in field::list(package, "dependencies")? {
        if dep["kind"] == "dev" {
            continue;
        }
        let mut sim = false;
        for feature in field::list(dep, "features")? {
            let feature = feature.as_str().ok_or_else(|| {
                format!(
                    "`{name}`: JSON field `features` of a dependency has an item that \
                     is not a string"
                )
            })?;
            sim |= feature == "sim";
        }
        if sim {
            let dep = field::text(dep, "name")?;
            problems.push(format!(
                "`{name}` turns on the `sim` feature of its dependency `{dep}`. Move \
                 it to the dev-dependencies. {RULE}"
            ));
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn of(name: &str) -> Vec<String> {
        let graph = crate::graph(&crate::fixture().join("features")).unwrap();
        let prefix = format!("`{name}` ");
        check(&graph)
            .into_iter()
            .filter(|p| p.starts_with(&prefix))
            .collect()
    }

    #[test]
    fn rejects_a_product_feature_that_turns_on_sim() {
        for (name, feature, value) in [
            ("own", "default", "sim"),
            ("weak", "simulate", "own?/sim"),
            ("strong", "simulate", "own/sim"),
        ] {
            assert_eq!(
                of(name),
                [format!(
                    "`{name}` turns on `{value}` in its feature `{feature}`. A `sim` \
                     feature is test-only: only a dev-dependency, the fuzz crate, or \
                     another `sim` feature turns it on. A product feature turns on \
                     `dep:sim` and `simulate` features. See rule 10 in \
                     docs/decisions/crate-map.md."
                )],
                "`{name}`"
            );
        }
    }

    #[test]
    fn rejects_a_normal_build_or_platform_dependency_that_turns_on_sim() {
        for name in ["normal", "build", "platform"] {
            assert_eq!(
                of(name),
                [format!(
                    "`{name}` turns on the `sim` feature of its dependency `own`. Move \
                     it to the dev-dependencies. A `sim` feature is test-only: only a \
                     dev-dependency, the fuzz crate, or another `sim` feature turns \
                     it on. A product feature turns on `dep:sim` and `simulate` \
                     features. See rule 10 in docs/decisions/crate-map.md."
                )],
                "`{name}`"
            );
        }
    }

    #[test]
    fn accepts_sim_from_sim_and_from_dev_dependencies() {
        assert_eq!(of("pass"), Vec::<String>::new());
    }

    #[test]
    fn rejects_sim_from_a_crate_that_only_a_feature_brings_in() {
        assert_eq!(
            of("hidden"),
            [
                "`hidden` turns on the `sim` feature of its dependency `own`. Move it \
                 to the dev-dependencies. A `sim` feature is test-only: only a \
                 dev-dependency, the fuzz crate, or another `sim` feature turns it \
                 on. A product feature turns on `dep:sim` and `simulate` features. \
                 See rule 10 in docs/decisions/crate-map.md."
            ]
        );
    }

    #[test]
    fn accepts_sim_from_a_crate_that_only_a_dev_dependency_reaches() {
        assert_eq!(of("helper"), Vec::<String>::new());
    }

    #[test]
    fn names_a_field_that_is_missing_or_of_another_shape() {
        for (package, error) in [
            (json!({}), "JSON has no string field `name`"),
            (
                json!({ "name": "a", "features": { "default": "sim" } }),
                "`a`: JSON field `features`: invalid type: string \"sim\", expected a \
                 sequence",
            ),
            (
                json!({ "name": "a", "features": {} }),
                "JSON has no array field `dependencies`",
            ),
            (
                json!({ "name": "a", "features": {}, "dependencies": [{}] }),
                "JSON has no array field `features`",
            ),
            (
                json!({
                    "name": "a",
                    "features": {},
                    "dependencies": [{ "features": ["sim"] }],
                }),
                "JSON has no string field `name`",
            ),
            (
                json!({
                    "name": "a",
                    "features": {},
                    "dependencies": [{ "name": "own", "kind": null, "features": [1] }],
                }),
                "`a`: JSON field `features` of a dependency has an item that is not a \
                 string",
            ),
        ] {
            assert_eq!(problems(&package), Err(error.to_string()), "{package}");
        }
    }

    #[test]
    fn checks_only_local_crates_and_names_a_malformed_graph() {
        assert_eq!(check(&json!({})), ["JSON has no array field `nodes`"]);
        let registry = json!({
            "id": "a",
            "name": "a",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "features": { "default": ["sim"] },
            "dependencies": [],
        });
        let member = json!({ "id": "a", "name": "a", "source": 1 });
        let graph = |packages: Value| {
            json!({
                "workspace_members": ["a"],
                "resolve": { "nodes": [{ "id": "a", "deps": [] }] },
                "packages": packages,
            })
        };
        assert_eq!(check(&graph(json!([registry]))), Vec::<String>::new());
        assert_eq!(
            check(&graph(json!([{ "id": "a" }]))),
            ["JSON has no string field `name`"]
        );
        assert_eq!(
            check(&graph(json!([]))),
            ["JSON field `packages` has no package `a`"]
        );
        assert_eq!(
            check(&graph(json!([member]))),
            ["`a`: JSON field `source` is neither null nor a string"]
        );
        let unresolved = json!({
            "workspace_members": ["a"],
            "resolve": { "nodes": [{
                "id": "a",
                "deps": [{ "pkg": "b", "dep_kinds": [{ "kind": null }] }],
            }] },
            "packages": [],
        });
        assert_eq!(
            check(&unresolved),
            ["JSON field `resolve.nodes` has no node `b`"]
        );
        let kindless = json!({
            "workspace_members": ["a"],
            "resolve": { "nodes": [{
                "id": "a",
                "deps": [{ "pkg": "b", "dep_kinds": [] }],
            }] },
            "packages": [
                {
                    "id": "a",
                    "name": "a",
                    "source": null,
                    "features": {},
                    "dependencies": [],
                },
                {
                    "id": "b",
                    "name": "b",
                    "source": null,
                    "features": { "default": ["sim"] },
                    "dependencies": [],
                },
            ],
        });
        assert_eq!(check(&kindless), ["JSON field `dep_kinds` is empty"]);
    }
}
