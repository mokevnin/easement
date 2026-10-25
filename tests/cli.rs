use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use tempfile::TempDir;

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A scratch copy of a fixture project, so tests can write baselines and edit code.
fn fixture(name: &str) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    copy_dir(&src, tmp.path());
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

fn easement(root: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("easement")
        .unwrap()
        .current_dir(root)
        .args(args)
        .assert()
}

#[test]
fn go_reports_layer_violation() {
    let (_tmp, root) = fixture("go");
    easement(&root, &["check"])
        .code(1)
        .stdout(contains(
            "internal/handlers/books.go:6: handler -> repository is not allowed",
        ))
        .stdout(contains("1 violation(s)"));
}

#[test]
fn ts_resolves_relative_alias_require_and_js_extension() {
    let (_tmp, root) = fixture("ts");
    let out = easement(&root, &["check", "--format", "json"])
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let pairs: Vec<(String, String, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().into(),
                v["to"].as_str().unwrap().into(),
                v["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            (
                "src/app/checkout.ts".into(),
                "infra".into(),
                "src/infra/db.ts".into()
            ),
            (
                "src/ui/page.tsx".into(),
                "infra".into(),
                "src/infra/db.ts".into()
            ),
        ]
    );
    assert_eq!(report["passed"], false);
    assert_eq!(report["stats"]["internal_deps"], 6);
    assert_eq!(report["stats"]["unresolved_deps"], 0);
}

#[test]
fn baseline_is_a_ratchet() {
    let (_tmp, root) = fixture("ts");

    easement(&root, &["baseline"])
        .success()
        .stdout(contains("2 violation(s)"));
    easement(&root, &["check"])
        .success()
        .stdout(contains("2 legalized"));

    // Paying off debt without shrinking the baseline fails the gate.
    let page = root.join("src/ui/page.tsx");
    let fixed = fs::read_to_string(&page)
        .unwrap()
        .replace("const db = require(\"../infra/db\");", "const db = 1;");
    fs::write(&page, fixed).unwrap();
    easement(&root, &["check"])
        .code(1)
        .stdout(contains("Stale baseline entries"));

    easement(&root, &["baseline"])
        .success()
        .stdout(contains("1 violation(s)"));
    easement(&root, &["check"]).success();

    // New debt is never accepted silently.
    fs::write(
        root.join("src/domain/leak.ts"),
        "import { checkout } from \"../app/checkout\";\n",
    )
    .unwrap();
    easement(&root, &["check"]).code(1).stdout(contains(
        "src/domain/leak.ts:1: domain -> app is not allowed",
    ));

    easement(&root, &["check", "--no-baseline"])
        .code(1)
        .stdout(contains("2 violation(s)"));
}

#[test]
fn exclude_skips_files() {
    let (_tmp, root) = fixture("go");
    let config = fs::read_to_string(root.join("easement.yml")).unwrap();
    fs::write(
        root.join("easement.yml"),
        format!("{config}\nexclude: [internal/handlers]\n"),
    )
    .unwrap();
    easement(&root, &["check"]).success();
}

#[test]
fn config_errors_exit_with_2() {
    let (_tmp, root) = fixture("go");
    fs::write(
        root.join("easement.yml"),
        "version: 1\ncomponents:\n  a: { in: a }\nrules:\n  a: { may_depend_on: [ghost] }\n",
    )
    .unwrap();
    easement(&root, &["check"])
        .code(2)
        .stderr(contains("unknown component `ghost`"));
    easement(&root, &["check", "-c", "missing.yml"]).code(2);
}

fn violations(root: &Path) -> (Vec<(String, String)>, serde_json::Value) {
    let out = easement(root, &["check", "--format", "json"])
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let pairs = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["target"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    (pairs, report["stats"].clone())
}

#[test]
fn python_resolves_absolute_relative_and_from_imports() {
    let (_tmp, root) = fixture("python");
    let (pairs, stats) = violations(&root);
    assert_eq!(
        pairs,
        vec![
            (
                "src/shop/api/handlers.py".to_string(),
                "src/shop/infra/db.py".to_string()
            ),
            (
                "src/shop/api/views.py".to_string(),
                "src/shop/infra/__init__.py".to_string()
            ),
        ]
    );
    // handlers: shop.domain.order, shop.infra.db, .views; db: ..domain.order; views: shop.infra
    assert_eq!(stats["internal_deps"], 5);
    assert_eq!(stats["external_deps"], 2);
    assert_eq!(stats["unresolved_deps"], 0);
}

#[test]
fn python_roots_are_configurable() {
    let (_tmp, root) = fixture("python");
    let config = fs::read_to_string(root.join("easement.yml")).unwrap();
    fs::write(
        root.join("easement.yml"),
        format!("{config}\npython:\n  roots: [lib]\n"),
    )
    .unwrap();
    // With a wrong root absolute imports become external; relative ones still resolve.
    let (pairs, stats) = violations(&root);
    assert!(pairs.is_empty());
    assert_eq!(stats["internal_deps"], 2);
}

#[test]
fn java_resolves_by_package_declaration() {
    let (_tmp, root) = fixture("java");
    let (pairs, stats) = violations(&root);
    assert_eq!(
        pairs,
        vec![(
            "src/main/java/com/acme/shop/web/OrderController.java".to_string(),
            "src/main/java/com/acme/shop/infra".to_string()
        )]
    );
    // repository -> Order, controller -> Order (static), controller -> infra.*
    assert_eq!(stats["internal_deps"], 3);
    assert_eq!(stats["external_deps"], 1);
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn diff_mode_judges_only_changed_lines() {
    let (_tmp, root) = fixture("ts");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "base"]);
    git(&root, &["checkout", "-q", "-b", "feature"]);

    // Legacy violations exist on main, but this change touches nothing.
    easement(&root, &["check"]).code(1);
    easement(&root, &["check", "--diff", "main"])
        .success()
        .stdout(contains("Checking lines changed since main"));

    // A committed new violation in an existing file.
    let checkout = root.join("src/app/checkout.ts");
    let text = fs::read_to_string(&checkout).unwrap();
    fs::write(
        &checkout,
        format!("import * as ui from \"../ui/page\";\n{text}"),
    )
    .unwrap();
    git(&root, &["commit", "-q", "-am", "leak"]);
    // An uncommitted, untracked file with another one.
    fs::write(
        root.join("src/domain/leak.ts"),
        "import { save } from \"../infra/db\";\n",
    )
    .unwrap();

    let out = easement(&root, &["check", "--diff", "main", "--format", "json"])
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let found: Vec<(String, u64)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().into(),
                v["line"].as_u64().unwrap(),
            )
        })
        .collect();
    // checkout.ts -> infra moved to line 4 but is untouched, so it is not reported.
    assert_eq!(
        found,
        vec![
            ("src/app/checkout.ts".to_string(), 1),
            ("src/domain/leak.ts".to_string(), 1),
        ]
    );
    assert_eq!(report["diff"]["base"], "main");

    easement(&root, &["check", "--diff", "no-such-branch"])
        .code(2)
        .stderr(contains("merge-base"));
}

#[test]
fn public_api_cycles_reasons_and_suppressions() {
    let (_tmp, root) = fixture("rules");
    let out = easement(&root, &["check", "--format", "json"])
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let found: Vec<(String, u64, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["rule"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected = [
        ("src/billing/internal/charge.ts", 1, "cycle"),
        ("src/orders/service.ts", 2, "privacy"),
        ("src/ui/page.ts", 4, "dependency"),
        ("src/ui/page.ts", 4, "suppression-reason"),
        ("src/ui/page.ts", 5, "unused-suppression"),
    ];
    assert_eq!(
        found,
        expected
            .iter()
            .map(|(f, l, r)| (f.to_string(), *l, r.to_string()))
            .collect::<Vec<_>>()
    );
    assert_eq!(report["stats"]["suppressed"], 1);
    let violations = report["violations"].as_array().unwrap();
    assert_eq!(
        violations[0]["message"],
        "component dependency cycle: billing -> orders -> billing"
    );
    assert_eq!(
        violations[1]["because"],
        "billing internals change without notice"
    );
    assert_eq!(violations[2]["because"], "the UI talks to orders only");

    // Text output carries the rule id and the reason.
    easement(&root, &["check"])
        .code(1)
        .stdout(contains(
            "src/orders/service.ts:2: orders -> billing: src/billing/internal/charge.ts is not in the public API of billing",
        ))
        .stdout(contains("  because: billing internals change without notice"));

    // Cycles and the other rules are baselined like dependency violations.
    easement(&root, &["baseline"])
        .success()
        .stdout(contains("5 violation(s)"));
    easement(&root, &["check"]).success();
}

#[test]
fn layers_forbidden_externals_metrics_and_strict() {
    let (_tmp, root) = fixture("policy");
    let out = easement(&root, &["check", "--format", "json"])
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let found: Vec<(String, String, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["rule"].as_str().unwrap().into(),
                v["file"].as_str().unwrap().into(),
                v["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    let has = |rule: &str, file: &str, target: &str| {
        assert!(
            found.contains(&(rule.into(), file.into(), target.into())),
            "missing {rule} {file} {target} in {found:?}"
        );
    };
    has("dependency", "src/web/page.ts", "src/data/db.ts");
    has("dependency", "src/core/run.ts", "src/web/page.ts");
    has("external", "src/web/page.ts", "moment/locale/ru");
    has("external", "src/core/run.ts", "axios");
    has("metric", "src/core/run.ts", "max_fan_out");
    has("orphan", "src/stray.ts", "src/stray.ts");
    assert_eq!(found.len(), 6, "{found:?}");
    // Layers allow web -> core, core -> data, and common allows shared.
    assert!(!found.iter().any(|(_, _, t)| t == "src/core/run.ts"));
    assert!(!found.iter().any(|(_, _, t)| t == "src/shared/fmt.ts"));
    assert!(!found.iter().any(|(_, _, t)| t == "lodash/fp"));
}

#[test]
fn external_violations_can_be_suppressed() {
    let (_tmp, root) = fixture("policy");
    let page = root.join("src/web/page.ts");
    let text = fs::read_to_string(&page).unwrap();
    fs::write(
        &page,
        text.replace(
            "import moment",
            "// easement:disable-next-line external -- legacy locale\nimport moment",
        ),
    )
    .unwrap();
    easement(&root, &["check"])
        .code(1)
        .stdout(predicates::str::contains("moment").not())
        .stdout(contains("1 suppressed"));
}

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

/// A scratch project made of the given files.
fn project(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    for (path, text) in files {
        let path = tmp.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

#[test]
fn ci_formats() {
    let (_tmp, root) = fixture("policy");

    let github = stdout_of(easement(&root, &["check", "--format", "github"]).code(1));
    assert!(
        github.contains(
            "::error file=src/web/page.ts,line=2,title=easement dependency::web -> data is forbidden"
        ),
        "{github}"
    );

    let sarif: serde_json::Value = serde_json::from_str(&stdout_of(
        easement(&root, &["check", "--format", "sarif"]).code(1),
    ))
    .unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 6);
    assert!(results.iter().any(|r| {
        r["ruleId"] == "orphan"
            && r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"] == "src/stray.ts"
    }));

    let junit = stdout_of(easement(&root, &["check", "--format", "junit"]).code(1));
    assert!(junit.contains("<testsuite name=\"easement\" tests=\"6\" failures=\"6\">"));
    assert!(junit.contains("type=\"external\""));
}

#[test]
fn init_writes_a_config_that_passes() {
    let (_tmp, root) = project(&[
        (
            "src/web/page.ts",
            "import { run } from '../core/run';\nexport const p = run;\n",
        ),
        (
            "src/core/run.ts",
            "import { q } from '../data/q';\nexport const run = q;\n",
        ),
        ("src/data/q.ts", "export const q = 1;\n"),
        ("tests/t.ts", "export {};\n"),
    ]);
    easement(&root, &["init"])
        .success()
        .stdout(contains("3 components and 2 allowed dependencies"));
    let config = fs::read_to_string(root.join("easement.yml")).unwrap();
    assert!(config.contains("  web: { in: src/web }"), "{config}");
    assert!(
        config.contains("  web: { may_depend_on: [core] }"),
        "{config}"
    );
    assert!(
        config.contains("  core: { may_depend_on: [data] }"),
        "{config}"
    );
    assert!(!config.contains("tests"), "{config}");
    easement(&root, &["check"])
        .success()
        .stdout(contains("passed"));
    // Turning the generated allowance around is a violation.
    fs::write(
        root.join("src/data/q.ts"),
        "import { run } from '../core/run';\nexport const q = run;\n",
    )
    .unwrap();
    easement(&root, &["check"])
        .code(1)
        .stdout(contains("data -> core is not allowed"));
    easement(&root, &["init"])
        .code(2)
        .stderr(contains("already exists"));
    easement(&root, &["init", "--force"]).success();
}

#[test]
fn explain_graph_and_schema() {
    let (_tmp, root) = fixture("policy");

    let explained = stdout_of(easement(&root, &["explain", "src/web/page.ts"]).success());
    assert!(
        explained.contains("component: web (matches `src/web`)"),
        "{explained}"
    );
    assert!(explained.contains("may depend on: core (layers), shared (common)"));
    assert!(explained.contains("forbidden: data"));
    easement(&root, &["explain", "src/stray.ts"])
        .code(2)
        .stderr(contains("belongs to no component"));

    let mermaid = stdout_of(easement(&root, &["graph"]).success());
    assert!(mermaid.starts_with("graph LR\n"), "{mermaid}");
    assert!(
        mermaid.contains("-.->"),
        "the forbidden web -> data edge is dashed: {mermaid}"
    );
    assert!(mermaid.contains("linkStyle"));
    let dot = stdout_of(easement(&root, &["graph", "--format", "dot"]).success());
    assert!(dot.starts_with("digraph easement {"), "{dot}");
    assert!(dot.contains("color=red"));

    let schema: serde_json::Value =
        serde_json::from_str(&stdout_of(easement(&root, &["schema"]).success())).unwrap();
    assert_eq!(schema["type"], "object");
}

/// The keys serde reports as the valid ones, read from its error for an unknown key.
fn config_keys(yaml: &str) -> Vec<String> {
    let (_tmp, root) = project(&[("easement.yml", yaml)]);
    let stderr = easement(&root, &["check"])
        .code(2)
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(stderr).unwrap();
    let list = stderr
        .split("expected one of ")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    let mut keys: Vec<String> = list
        .split('`')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect();
    keys.sort();
    keys
}

#[test]
fn schema_lists_exactly_the_config_keys() {
    let (_tmp, root) = fixture("policy");
    let schema: serde_json::Value =
        serde_json::from_str(&stdout_of(easement(&root, &["schema"]).success())).unwrap();
    let keys = |value: &serde_json::Value| -> Vec<String> {
        let mut keys: Vec<String> = value["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    // `extends` is merged away before the config is read, so serde never lists it.
    let mut top = keys(&schema);
    top.retain(|k| k != "extends");
    assert_eq!(
        top,
        config_keys("version: 1\nbogus: 1\ncomponents:\n  a: { in: a }\n")
    );
    assert_eq!(
        keys(&schema["definitions"]["component"]),
        config_keys("version: 1\ncomponents:\n  a: { in: a, bogus: 1 }\n")
    );
    assert_eq!(
        keys(&schema["definitions"]["rule"]),
        config_keys("version: 1\ncomponents:\n  a: { in: a }\nrules:\n  a: { bogus: 1 }\n")
    );
}

#[test]
fn extends_shares_a_config_between_packages() {
    let (_tmp, root) = project(&[
        (
            "shared/easement.base.yml",
            "version: 1\ncomponents:\n  ui: { in: src/ui }\n  db: { in: src/db }\nrules:\n  ui: { because: the UI never touches the database }\n",
        ),
        ("easement.yml", "extends: shared/easement.base.yml\n"),
        ("src/ui/a.ts", "import '../db/q';\n"),
        ("src/db/q.ts", "export {};\n"),
    ]);
    easement(&root, &["check"])
        .code(1)
        .stdout(contains("ui -> db is not allowed"))
        .stdout(contains("because: the UI never touches the database"));
}

#[test]
fn dynamic_imports_are_reported_not_checked() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: src/a }\n",
        ),
        (
            "src/a/m.py",
            "import importlib\nimportlib.import_module('x')\nimportlib.import_module(name)\n",
        ),
    ]);
    easement(&root, &["check"])
        .success()
        .stdout(contains("1 dynamic imports not analysed"));
    let out = stdout_of(easement(&root, &["check", "--format", "json"]).success());
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        report["stats"]["dynamic"],
        serde_json::json!(["src/a/m.py:3"])
    );
}

#[test]
fn watch_checks_again_after_a_change() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b }\n",
        ),
        ("src/a/x.ts", "export {};\n"),
        ("src/b/y.ts", "export {};\n"),
    ]);
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("easement"))
        .current_dir(&root)
        .args(["check", "--watch"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let wait_for = |text: &str| -> bool {
        loop {
            match rx.recv_timeout(Duration::from_secs(20)) {
                Ok(line) if line.contains(text) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    };
    let passed = wait_for("easement passed");
    if passed {
        fs::write(root.join("src/a/x.ts"), "import '../b/y';\n").unwrap();
    }
    let failed = passed && wait_for("a -> b is not allowed");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(passed, "the first check did not pass");
    assert!(failed, "the change was not picked up");
}

/// Every layered fixture has one forbidden import, web -> infra, and allowed ones
/// that must be seen: the count of internal dependencies proves the adapter resolved them.
fn layered(name: &str, file: &str, target: &str, internal_deps: u64) {
    let (_tmp, root) = fixture(name);
    if name == "rust" {
        // Not stored in the fixture: tools that walk the repository would take it for a crate.
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\n").unwrap();
    }
    let (found, stats) = violations(&root);
    assert_eq!(
        found,
        vec![(file.to_string(), target.to_string())],
        "{name}"
    );
    assert_eq!(stats["internal_deps"], internal_deps, "{name}");
}

#[test]
fn rust_resolves_the_module_tree() {
    // `use super::*` in the inline test module adds nothing; `use std::fmt` is external.
    layered("rust", "src/web/mod.rs", "src/infra/repo.rs", 8);
}

#[test]
fn csharp_resolves_namespaces() {
    layered("csharp", "Web/OrderController.cs", "Infra", 3);
}

#[test]
fn kotlin_resolves_packages_and_declarations() {
    layered(
        "kotlin",
        "src/main/kotlin/com/acme/shop/web/OrderController.kt",
        "src/main/kotlin/com/acme/shop/infra/OrderRepository.kt",
        3,
    );
}

#[test]
fn php_resolves_psr4_names() {
    layered(
        "php",
        "src/Web/OrderController.php",
        "src/Infra/OrderRepository.php",
        3,
    );
}

#[test]
fn swift_resolves_package_modules() {
    layered(
        "swift",
        "Sources/Web/OrderController.swift",
        "Sources/Infra",
        3,
    );
}

fn reports(root: &Path, args: &[&str]) -> Vec<serde_json::Value> {
    let mut all = vec!["check", "--format", "json"];
    all.extend_from_slice(args);
    let out = easement(root, &all).get_output().stdout.clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    report["violations"].as_array().unwrap().clone()
}

fn find<'a>(found: &'a [serde_json::Value], file: &str, rule: &str) -> &'a serde_json::Value {
    found
        .iter()
        .find(|v| v["file"] == file && v["rule"] == rule)
        .unwrap_or_else(|| panic!("no {rule} in {file}: {found:?}"))
}

#[test]
fn instances_importers_priority_reasons_and_unowned() {
    let (_tmp, root) = fixture("structure");
    let found = reports(&root, &[]);
    assert_eq!(found.len(), 7, "{found:?}");

    // Instances of `pages` must not import each other.
    let isolation = find(&found, "src/pages/contacts/index.ts", "isolation");
    assert!(
        isolation["message"]
            .as_str()
            .unwrap()
            .contains("src/pages/contacts -> src/pages/segments")
    );
    assert_eq!(isolation["because"], "pages ship as separate bundles");

    // `ee` is imported only from wiring, even though `common` lets everyone in.
    let importers = find(&found, "src/pages/segments/list.ts", "dependency");
    assert!(
        importers["message"]
            .as_str()
            .unwrap()
            .contains("may be imported only from wiring")
    );
    assert_eq!(importers["because"], "ee is licensed separately");

    // The priority of `tests` beats the longer pattern of `billing`.
    let test = find(&found, "src/billing/feature/area/ui.test.ts", "dependency");
    assert_eq!(test["from"], "tests");

    // A reason on the listed name beats the reason of the rule.
    assert_eq!(
        find(&found, "src/core/util.ts", "dependency")["because"],
        "core never knows enterprise code"
    );
    let externals: Vec<_> = found.iter().filter(|v| v["rule"] == "external").collect();
    assert_eq!(externals.len(), 2);
    let because =
        |target: &str| externals.iter().find(|v| v["target"] == target).unwrap()["because"].clone();
    assert_eq!(because("moment"), "moment is deprecated, use date-fns");
    assert_eq!(because("lodash/fp"), "core stays dependency-free");

    // Only the file outside every component and outside `unowned` is an orphan.
    let orphans: Vec<_> = found.iter().filter(|v| v["rule"] == "orphan").collect();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0]["file"], "src/stray.ts");
}

#[test]
fn isolation_can_be_suppressed() {
    let (_tmp, root) = fixture("structure");
    let file = root.join("src/pages/contacts/index.ts");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        format!("// easement:disable-next-line isolation -- shared list\n{text}"),
    )
    .unwrap();
    let found = reports(&root, &[]);
    assert!(!found.iter().any(|v| v["rule"] == "isolation"), "{found:?}");
}

#[test]
fn component_and_scope_narrow_the_report() {
    let (_tmp, root) = fixture("structure");

    let core = reports(&root, &["--component", "core"]);
    assert_eq!(core.len(), 3, "{core:?}");
    assert!(core.iter().all(|v| v["file"] == "src/core/util.ts"));

    let pages = reports(&root, &["--scope", "src/pages"]);
    let files: Vec<_> = pages.iter().map(|v| v["file"].as_str().unwrap()).collect();
    assert_eq!(
        files,
        ["src/pages/contacts/index.ts", "src/pages/segments/list.ts"]
    );

    // Both restrictions must hold.
    assert!(reports(&root, &["--component", "core", "--scope", "src/pages"]).is_empty());

    // The counters follow the scope, the cycles and limits still see everything.
    let out = easement(
        &root,
        &["check", "--format", "json", "--component", "pages"],
    )
    .get_output()
    .stdout
    .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["stats"]["files"], 2);
    assert_eq!(report["stats"]["internal_deps"], 3);

    easement(&root, &["check", "--component", "ghost"])
        .code(2)
        .stderr(contains("unknown component `ghost`"));
}

#[test]
fn scoped_runs_do_not_report_stale_entries_of_other_files() {
    let (_tmp, root) = fixture("structure");
    easement(&root, &["baseline"]).success();
    // Fix a violation inside `pages`: its baseline entry is stale there, not in `core`.
    fs::write(
        root.join("src/pages/segments/list.ts"),
        "export const list = 1;\n",
    )
    .unwrap();
    easement(&root, &["check", "--component", "core"])
        .success()
        .stdout(contains("0 stale"));
    easement(&root, &["check", "--component", "pages"])
        .code(1)
        .stdout(contains("1 stale"));
}

#[test]
fn required_imports_modules_and_files_are_checked_per_directory() {
    let (_tmp, root) = fixture("required");
    let found = reports(&root, &[]);
    let got: Vec<(String, String)> = found
        .iter()
        .map(|v| {
            assert_eq!(v["rule"], "required");
            (
                v["file"].as_str().unwrap().into(),
                v["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    let has = |file: &str, target: &str| {
        assert!(
            got.contains(&(file.into(), target.into())),
            "{file} {target}: {got:?}"
        );
    };
    // `books` has everything; `users` imports shared from one file but misses the rest.
    has("src/api/users/resources/b.ts", "external goverter");
    has("src/api/users/resources/b.ts", "file README.md");
    has("src/api/orders/resources/d.ts", "import shared");
    has("src/api/orders/resources/d.ts", "external goverter");
    has("src/api/orders/resources/d.ts", "file README.md");
    assert_eq!(got.len(), 5, "{got:?}");
    assert_eq!(
        found[0]["because"], "mappers are generated with goverter",
        "{found:?}"
    );

    // Adding what is missing clears the violations of that directory.
    let users = root.join("src/api/users/resources");
    fs::write(users.join("README.md"), "# users\n").unwrap();
    fs::write(
        users.join("b.ts"),
        "import goverter from \"goverter\";\nexport const b = goverter;\n",
    )
    .unwrap();
    let found = reports(&root, &[]);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(
        found
            .iter()
            .all(|v| v["file"] == "src/api/orders/resources/d.ts")
    );
}
