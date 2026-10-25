use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
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
