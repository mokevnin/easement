//! The baseline: the debt it records, how it only shrinks unless new debt is
//! accepted, and which new code it still counts as the debt it holds.

use std::fs;
use std::path::Path;

use easement::commands::{self, Outcome, Recorded};

use crate::common::{fixture, project};
use crate::run::{cache, check, config, text};

/// What `baseline` records for the project at `root`, new debt too when `accept_new`.
fn baseline(root: &Path, accept_new: bool) -> Recorded {
    commands::baseline(&config(root), accept_new, &mut cache()).unwrap()
}

#[test]
fn the_baseline_only_shrinks_unless_new_debt_is_accepted() {
    let (_tmp, root) = fixture("ts");
    let recorded = baseline(&root, false);
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("2 violation(s)"),
        "{}",
        recorded.text()
    );

    // Pay off one debt and add a new one.
    let page = root.join("src/ui/legacy.js");
    let fixed = fs::read_to_string(&page)
        .unwrap()
        .replace("const db = require(\"../infra/db\");", "const db = 1;");
    fs::write(&page, fixed).unwrap();
    fs::write(
        root.join("src/domain/leak.ts"),
        "import { checkout } from \"../app/checkout\";\n",
    )
    .unwrap();

    // The paid debt leaves the baseline; the new one is not written.
    let recorded = baseline(&root, false);
    assert_eq!(recorded.outcome(), Outcome::Failed);
    let said = recorded.text();
    assert!(said.contains("1 violation(s) written"), "{said}");
    assert!(
        said.contains("src/domain/leak.ts:1: domain -> app is not allowed"),
        "{said}"
    );
    assert!(said.contains("1 new violation(s) not written"), "{said}");
    assert!(said.contains("--accept-new"), "{said}");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let said = text(&checked);
    assert!(said.contains("src/domain/leak.ts:1"), "{said}");
    assert!(!said.contains("Stale baseline entries"), "{said}");

    // Legalizing new debt takes the flag.
    let recorded = baseline(&root, true);
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("2 violation(s) written"),
        "{}",
        recorded.text()
    );
    assert_eq!(check(&root).outcome(), Outcome::Passed);
}

#[test]
fn the_baseline_keeps_the_debt_of_files_no_indexer_read() {
    let (_tmp, root) = fixture("ts");
    assert_eq!(baseline(&root, false).outcome(), Outcome::Passed);
    // The indexer stops covering the UI, so its debt is not seen, not paid.
    let tsconfig = fs::read_to_string(root.join("tsconfig.json")).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        tsconfig.replacen('{', "{\n  \"exclude\": [\"src/ui\"],", 1),
    )
    .unwrap();
    let recorded = baseline(&root, false);
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded
            .text()
            .contains("kept for files no indexer covered"),
        "{}",
        recorded.text()
    );
    let written = fs::read_to_string(root.join("easement.baseline.yml")).unwrap();
    assert!(written.contains("src/ui/legacy.js"), "{written}");
}

/// The debt of a file no indexer read is not seen, so it is not fixed either:
/// `check` holds it as `baseline` does, and fails on no stale entry that
/// `baseline` would keep.
#[test]
fn the_debt_of_files_no_indexer_read_is_not_stale() {
    let (_tmp, root) = fixture("ts");
    assert_eq!(baseline(&root, false).outcome(), Outcome::Passed);
    let tsconfig = fs::read_to_string(root.join("tsconfig.json")).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        tsconfig.replacen('{', "{\n  \"exclude\": [\"src/ui\"],", 1),
    )
    .unwrap();
    let checked = check(&root);
    let report = checked.report().json();
    assert!(report.contains("\"stale\": []"), "{report}");
    assert!(report.contains("src/ui/legacy.js"), "{report}");
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
}

#[test]
fn a_new_file_on_a_baselined_cycle_or_limit_is_not_new_debt() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\nno_cycles: true\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b }\nrules:\n  a: { may_depend_on: [b], max_fan_out: 0 }\n  b: { may_depend_on: [a] }\n",
        ),
        ("src/a/M.kt", "package a\nimport b.N\nclass M\n"),
        ("src/b/N.kt", "package b\nimport a.M\nclass N\n"),
    ]);
    let recorded = baseline(&root, false);
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("2 violation(s)"),
        "{}",
        recorded.text()
    );
    // A file that sorts first now carries the same dependency between components.
    fs::write(root.join("src/a/A.kt"), "package a\nimport b.N\nclass A\n").unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(text(&checked).contains("2 legalized"), "{}", text(&checked));
}

#[test]
fn a_new_file_in_a_directory_with_baselined_requirement_is_not_new_debt() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: src/a }\nrules:\n  a: { requires_file: [README.md] }\n",
        ),
        ("src/a/M.kt", "package a\nclass M\n"),
    ]);
    let recorded = baseline(&root, false);
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("1 violation(s)"),
        "{}",
        recorded.text()
    );
    fs::write(root.join("src/a/A.kt"), "package a\nclass A\n").unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(text(&checked).contains("1 legalized"), "{}", text(&checked));
}
