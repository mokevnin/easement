//! The npm package a name of an installed package is of: the one under the
//! innermost `node_modules/<pkg>/` of its path, whatever package the indexer
//! credited it to, with or without a `name` in either `package.json`.

use std::path::Path;

use easement::CacheUse;
use easement::commands::{self, CheckOptions, Outcome};

use crate::common::project;
use crate::run::{cache_in, config, json};

const TYPES: &str =
    "export declare class Thing {\n  run(): void;\n}\nexport declare const tool: number;\n";

/// A named project, the one scip-typescript credits a nameless package's names to.
const NAMED_PROJECT: &str = "{\"name\": \"app\", \"version\": \"1.0.0\"}\n";

const WITHOUT_NAME: &str = "{\"version\": \"1.0.0\", \"types\": \"index.d.ts\"}\n";

/// The policy, at `easement.yml`, and the code, under `src`: `web` forbids a name of
/// `package` and `package` itself, `api` may use no external module but `react`,
/// and nothing is declared.
fn policy_and_code(package: &str) -> [(String, String); 3] {
    [
        (
            "easement.yml".into(),
            format!(
                "version: 1\nmanifests: true\ncomponents:\n  web: {{ in: src/web }}\n  api: {{ in: src/api }}\nrules:\n  web: {{ forbid_symbol: ['{package}.Thing'], forbid_external: ['{package}'] }}\n  api: {{ may_use_external: [react] }}\n"
            ),
        ),
        (
            "src/web/a.ts".into(),
            format!(
                "import {{ Thing }} from \"{package}\";\nexport const t = new Thing();\nt.run();\n"
            ),
        ),
        (
            "src/api/b.ts".into(),
            format!("import {{ tool }} from \"{package}\";\nexport const b = tool;\n"),
        ),
    ]
}

/// What the check says of the code of [`policy_and_code`] when `package` is
/// installed: every rule fails.
fn every_rule_fails(package: &str) -> Vec<String> {
    let mut said = vec![
        format!("api -> external {package} is not in may_use_external"),
        format!(
            "{package} is not declared in package.json; add it to the dependencies, or drop the dependency"
        ),
        format!("web -> external {package} is forbidden"),
        format!("web must not use {package}.Thing"),
    ];
    said.sort();
    said
}

/// The violations of the check of `files`, sorted, and its evidence gaps.
fn judged(files: &[(String, String)]) -> (Vec<serde_json::Value>, serde_json::Value) {
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let (_tmp, root) = project(&files);
    verdict(&root)
}

/// The cases differ only in what is installed, which the cache key leaves to the
/// lock file: each is indexed afresh.
fn verdict(root: &Path) -> (Vec<serde_json::Value>, serde_json::Value) {
    let scratch = tempfile::TempDir::new().unwrap();
    let checked = commands::check(
        &CheckOptions::new(config(root)),
        &mut cache_in(scratch.path(), CacheUse::Off),
    )
    .unwrap();
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    let mut violations = report["violations"].as_array().unwrap().clone();
    violations.sort_by_key(ToString::to_string);
    (violations, report["evidence_gaps"].clone())
}

/// What the violations say, each once.
fn messages(violations: &[serde_json::Value]) -> Vec<&str> {
    let mut said: Vec<&str> = violations
        .iter()
        .map(|violation| violation["message"].as_str().unwrap())
        .collect();
    said.sort_unstable();
    said.dedup();
    said
}

#[test]
fn an_installed_package_is_judged_alike_whatever_either_package_json_names() {
    let projects = [
        ("named project", NAMED_PROJECT),
        (
            "nameless project",
            "{\"private\": true, \"dependencies\": {}}\n",
        ),
    ];
    let installed = [
        (
            "named package",
            Some("{\"name\": \"kit\", \"version\": \"1.0.0\", \"types\": \"index.d.ts\"}\n"),
        ),
        ("package without name", Some(WITHOUT_NAME)),
        ("package without package.json", None),
    ];
    let mut expected: Option<Vec<serde_json::Value>> = None;
    for (project_case, manifest) in projects {
        for (package_case, package) in installed {
            let mut files = vec![
                ("package.json".into(), manifest.into()),
                ("node_modules/kit/index.d.ts".into(), TYPES.into()),
            ];
            files.extend(policy_and_code("kit"));
            files.extend(package.map(|text| ("node_modules/kit/package.json".into(), text.into())));
            let (violations, gaps) = judged(&files);
            let cell = format!("{project_case}, {package_case}");
            assert_eq!(gaps, serde_json::json!([]), "{cell}");
            match &expected {
                None => {
                    // Every rule fails on the named package, so each cell is held to all four.
                    assert_eq!(messages(&violations), every_rule_fails("kit"), "{cell}");
                    expected = Some(violations);
                }
                Some(named) => assert_eq!(&violations, named, "{cell}"),
            }
        }
    }
}

#[test]
fn a_scoped_package_without_name_is_named_by_its_scope_and_directory() {
    let mut files = vec![
        ("package.json".into(), NAMED_PROJECT.into()),
        ("node_modules/@sc/kit/index.d.ts".into(), TYPES.into()),
        (
            "node_modules/@sc/kit/package.json".into(),
            WITHOUT_NAME.into(),
        ),
    ];
    files.extend(policy_and_code("@sc/kit"));
    let (violations, gaps) = judged(&files);
    assert_eq!(gaps, serde_json::json!([]));
    assert_eq!(messages(&violations), every_rule_fails("@sc/kit"));
}

/// pnpm installs a package in its store, under a `node_modules` of its own, and
/// links it into the project's.
#[cfg(unix)]
#[test]
fn a_package_pnpm_installs_is_named_by_its_innermost_node_modules() {
    let store = "node_modules/.pnpm/kit@1.0.0/node_modules/kit";
    let mut files = vec![
        ("package.json".into(), NAMED_PROJECT.into()),
        (format!("{store}/index.d.ts"), TYPES.into()),
        (format!("{store}/package.json"), WITHOUT_NAME.into()),
    ];
    files.extend(policy_and_code("kit"));
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let (_tmp, root) = project(&files);
    std::os::unix::fs::symlink(
        ".pnpm/kit@1.0.0/node_modules/kit",
        root.join("node_modules/kit"),
    )
    .unwrap();
    let (violations, gaps) = verdict(&root);
    assert_eq!(gaps, serde_json::json!([]));
    assert_eq!(messages(&violations), every_rule_fails("kit"));
}

/// A workspace hoists its packages' dependencies to the `node_modules` of its root,
/// whose package scip-typescript credits their names to.
#[test]
fn a_package_hoisted_to_the_root_of_a_workspace_is_named_not_the_root() {
    let mut files: Vec<(String, String)> = vec![
        (
            "package.json".into(),
            "{\"name\": \"root\", \"private\": true, \"workspaces\": [\"packages/*\"]}\n".into(),
        ),
        ("packages/app/package.json".into(), NAMED_PROJECT.into()),
        (
            "packages/app/tsconfig.json".into(),
            "{\"compilerOptions\": {\"module\": \"esnext\", \"moduleResolution\": \"bundler\"}, \"include\": [\"src\"]}\n".into(),
        ),
        ("node_modules/kit/index.d.ts".into(), TYPES.into()),
        ("node_modules/kit/package.json".into(), WITHOUT_NAME.into()),
    ];
    files.extend(
        policy_and_code("kit")
            .into_iter()
            .map(|(path, text)| match path.as_str() {
                "easement.yml" => (path, text.replace("in: src/", "in: packages/app/src/")),
                _ => (format!("packages/app/{path}"), text),
            }),
    );
    let (violations, gaps) = judged(&files);
    assert_eq!(gaps, serde_json::json!([]));
    let expected: Vec<String> = every_rule_fails("kit")
        .into_iter()
        .map(|said| said.replace("in package.json", "in packages/app/package.json"))
        .collect();
    assert_eq!(messages(&violations), expected);
}
