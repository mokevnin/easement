//! The inline suppressions a run honours: each directive that excused a
//! violation is listed with what it excused, narrowed as the violations are by
//! `--diff`, `--component` and `--scope`, and never changes the verdict.

use std::fs;
use std::path::{Path, PathBuf};

use easement::commands::{CheckOptions, Outcome};
use tempfile::TempDir;

use crate::common::{git, on_feature, project};
use crate::diff::diff_main;
use crate::run::{by_rule, check_with, config, json, parse, pinned, text};

const POLICY: &str = "version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
rules:
  web: { may_depend_on: [] }
";

/// `old.ts` has imported `storage` since `main`, under a directive that excuses it.
/// One literal per line: easement checks this file too, and a line of a string
/// that spans lines reads as a comment.
const OLD: &str = concat!(
    "// easement:disable-next-line dependency -- legacy save, remove with #12\n",
    "import { save } from \"../storage/db\";\n",
    "export const old = save;\n",
);

/// A project of `web` and `storage`, committed on `main` and checked out on
/// `feature`, where `web` may depend on nothing and `old.ts` is excused.
fn excused() -> (TempDir, PathBuf) {
    let (tmp, root) = project(&[
        ("easement.yml", POLICY),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/web/old.ts", OLD),
        ("src/storage/db.ts", "export const save = 1;\n"),
    ]);
    on_feature(&root);
    (tmp, root)
}

fn write(root: &Path, file: &str, text: &str) {
    fs::write(root.join(file), text).unwrap();
}

/// A listed suppression as its file, directive line and the rule and line of
/// each violation it excused.
type Listed = (String, u64, Vec<(String, u64)>);

/// The suppressions a report lists.
fn listed(report: &serde_json::Value) -> Vec<Listed> {
    report["suppressions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let excused = s["excused"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    (
                        v["rule"].as_str().unwrap().to_string(),
                        v["line"].as_u64().unwrap(),
                    )
                })
                .collect();
            (
                s["file"].as_str().unwrap().to_string(),
                s["line"].as_u64().unwrap(),
                excused,
            )
        })
        .collect()
}

/// A change that adds a forbidden import and a directive above it passes, and
/// the report says where the directive is, what it silenced and why.
#[test]
fn a_suppression_the_change_adds_is_listed() {
    let (_tmp, root) = excused();
    write(
        &root,
        "src/web/page.ts",
        "// easement:disable-next-line dependency -- the page saves drafts until #40\nimport { save } from \"../storage/db\";\nexport const page = save;\n",
    );
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    let mut report = json(&checked);
    // `old.ts` is not part of the change.
    assert_eq!(
        listed(&report),
        [(
            "src/web/page.ts".to_string(),
            1,
            vec![("dependency".to_string(), 2)]
        )],
        "{report}"
    );
    let entry = &report["suppressions"][0];
    assert_eq!(entry["target_line"], 2, "{report}");
    assert_eq!(
        entry["rules"],
        serde_json::json!(["dependency"]),
        "{report}"
    );
    assert_eq!(
        entry["reason"], "the page saves drafts until #40",
        "{report}"
    );
    assert_eq!(entry["excused"][0]["to"], "storage", "{report}");
    assert!(report["stats"].get("suppressed").is_none(), "{report}");

    let merge_base = report["diff"]["merge_base"].as_str().unwrap().to_string();
    report["diff"]["merge_base"] = "[merge-base]".into();
    let said = text(&checked).replace(&merge_base[..12], "[merge-base]");
    let github = checked.report().github();
    let sarif = parse(&checked.report().sarif());
    // The tool's version changes with every release.
    let results = &sarif["runs"][0]["results"];
    pinned(|| {
        insta::assert_json_snapshot!("check_suppression_added_json", report);
        insta::assert_snapshot!("check_suppression_added_text", said);
        insta::assert_snapshot!("check_suppression_added_github", github);
        insta::assert_json_snapshot!("check_suppression_added_sarif", results);
    });
}

/// A directive the change adds above an old violation is listed: the change
/// silenced it, though the import line did not move.
#[test]
fn a_directive_added_over_an_old_violation_is_listed() {
    let (_tmp, root) = excused();
    write(
        &root,
        "src/web/page.ts",
        "import { save } from \"../storage/db\";\nexport const page = save;\n",
    );
    git(&root, &["commit", "-q", "-am", "page saves"]);
    git(&root, &["checkout", "-q", "-B", "main"]);
    git(&root, &["checkout", "-q", "-b", "silence"]);
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);
    write(
        &root,
        "src/web/page.ts",
        "// easement:disable-next-line dependency -- drafts until #40\nimport { save } from \"../storage/db\";\nexport const page = save;\n",
    );
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    let report = json(&checked);
    assert_eq!(
        listed(&report),
        [(
            "src/web/page.ts".to_string(),
            1,
            vec![("dependency".to_string(), 2)]
        )],
        "{report}"
    );
}

/// A directive the change leaves alone is listed when what it excuses is on a
/// changed line: the change wrote a new violation under an old exception.
#[test]
fn an_old_directive_over_a_changed_line_is_listed() {
    let (_tmp, root) = excused();
    write(
        &root,
        "src/web/old.ts",
        &OLD.replace("{ save }", "{ save as keep }")
            .replace("= save", "= keep"),
    );
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    let report = json(&checked);
    assert_eq!(
        listed(&report),
        [(
            "src/web/old.ts".to_string(),
            1,
            vec![("dependency".to_string(), 2)]
        )],
        "{report}"
    );
    let said = text(&checked);
    assert!(said.contains("Inline suppressions in this change:\n  src/web/old.ts:1 [dependency] legacy save, remove with #12\n"), "{said}");
    assert!(said.contains(", 1 suppressed"), "{said}");
}

/// A change that touches neither a directive nor what it excuses lists none.
#[test]
fn a_suppression_the_change_does_not_touch_is_not_listed() {
    let (_tmp, root) = excused();
    write(&root, "src/storage/db.ts", "export const save = 2;\n");
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert!(report.get("suppressions").is_none(), "{report}");
    let said = text(&checked);
    assert!(!said.contains("suppress"), "{said}");
    // The whole project lists it.
    let checked = check_with(&root, |_| {});
    assert_eq!(listed(&json(&checked)).len(), 1);
    let said = text(&checked);
    assert!(
        said.contains("Inline suppressions:\n  src/web/old.ts:1"),
        "{said}"
    );
}

/// A change to the policy alone is held to what it causes: a violation the
/// policy at the merge-base produced too, excused there as here, is not.
#[test]
fn a_change_to_the_policy_lists_no_suppression_it_did_not_cause() {
    let (_tmp, root) = excused();
    fs::write(
        config(&root),
        POLICY.replace(
            "{ may_depend_on: [] }",
            "{ may_depend_on: [], because: web renders }",
        ),
    )
    .unwrap();
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    let report = json(&checked);
    assert_eq!(
        report["diff"]["policy_changed"],
        serde_json::json!(["easement.yml"]),
        "{report}"
    );
    assert!(report.get("suppressions").is_none(), "{report}");
}

/// `--component` and `--scope` keep a suppression when what it excused is in
/// scope, as they keep a violation.
#[test]
fn a_narrowed_run_lists_the_suppressions_in_scope() {
    let (_tmp, root) = excused();
    // How a run is narrowed, and how many suppressions it lists.
    type Run = (fn(&mut CheckOptions), usize);
    let runs: [Run; 4] = [
        (|o| o.components = vec!["web".into()], 1),
        (|o| o.components = vec!["storage".into()], 0),
        (|o| o.paths = vec!["src/web/old.ts".into()], 1),
        (|o| o.paths = vec!["src/storage".into()], 0),
    ];
    for (scoped, listed_there) in runs {
        let checked = check_with(&root, scoped);
        assert_eq!(checked.outcome(), Outcome::Passed);
        let report = json(&checked);
        assert_eq!(listed(&report).len(), listed_there, "{report}");
    }
}

/// A directive without a reason or matching nothing fails as before, and is
/// not listed as honoured.
#[test]
fn a_directive_that_excuses_nothing_fails_and_is_not_listed() {
    let (_tmp, root) = excused();
    write(
        &root,
        "src/web/page.ts",
        "// easement:disable-next-line dependency\nimport { save } from \"../storage/db\";\n// easement:disable-next-line dependency -- nothing here\nexport const page = save;\n",
    );
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    assert_eq!(
        by_rule(&report),
        [
            (
                "src/web/page.ts".to_string(),
                1,
                "suppression-reason".to_string()
            ),
            ("src/web/page.ts".to_string(), 2, "dependency".to_string()),
            (
                "src/web/page.ts".to_string(),
                3,
                "unused-suppression".to_string()
            ),
        ],
        "{report}"
    );
    assert!(report.get("suppressions").is_none(), "{report}");
}

/// Judged in full, the run lists every suppression, as it reports every violation.
#[test]
fn a_run_judged_in_full_lists_every_suppression() {
    let (_tmp, root) = excused();
    write(&root, "easement.yml", &format!("{POLICY}exclude: [docs]\n"));
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    let report = json(&checked);
    assert!(
        report["diff"]["in_full"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("(`exclude`)"),
        "{report}"
    );
    assert_eq!(
        listed(&report),
        [(
            "src/web/old.ts".to_string(),
            1,
            vec![("dependency".to_string(), 2)]
        )],
        "{report}"
    );
    let said = text(&checked);
    assert!(
        said.contains("\nInline suppressions:\n  src/web/old.ts:1 [dependency]"),
        "{said}"
    );
}
