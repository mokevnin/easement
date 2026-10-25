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
        .replace("const db = require(\"../infra/db\");\n", "const db = 1;\n");
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
