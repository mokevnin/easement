//! `check --diff <base> --plan <file>`: the imports the change added across
//! components, compared with the plan `propose --plan` judged. What the plan did
//! not name and what it named that the code lacks are reported; the verdict
//! stays the same.

use std::path::{Path, PathBuf};

use easement::commands::{Checked, Outcome};
use tempfile::TempDir;

use crate::common::{commit_all, fixture, git, on_feature, project, stub_packages, write_all};
use crate::diff::diff_main;
use crate::run::{check_with, error, json, pinned, text, try_check_with};

const POLICY: &str = "version: 1
components:
  web: { in: src/web }
  core: { in: src/core }
rules:
  web: { may_depend_on: [core] }
exclude: [src/legacy]
";

/// A project where `web` imports nothing yet, with `lodash` installed,
/// committed on `main` and checked out on `feature`.
fn web_and_core() -> (TempDir, PathBuf) {
    let (tmp, root) = project(&[
        ("easement.yml", POLICY),
        (".gitignore", "node_modules\n"),
        ("src/web/a.ts", "export const a = 1;\n"),
        ("src/web/other.ts", "export const other = 1;\n"),
        ("src/core/b.ts", "export const b = 1;\n"),
        ("src/core/c.ts", "export const c = 1;\n"),
        ("src/legacy/old.ts", "export const old = 1;\n"),
    ]);
    stub_packages(&root, &["lodash/fp"]);
    on_feature(&root);
    (tmp, root)
}

/// The report of `check --diff main --plan` once `src/web/a.ts` reads `source`.
fn compared(root: &Path, source: &str, plan: &str) -> serde_json::Value {
    write_all(root, &[("src/web/a.ts", source)]);
    json(&check_plan(root, plan))
}

/// `check --diff main --plan`, with the plan as its text.
fn check_plan(root: &Path, plan: &str) -> Checked {
    check_with(root, |o| {
        diff_main(o);
        o.plan = Some(plan.to_string());
    })
}

/// The unplanned imports of a report as `file:line -> target`.
fn unplanned(report: &serde_json::Value) -> Vec<String> {
    report["plan"]["unplanned"]
        .as_array()
        .unwrap_or_else(|| panic!("no plan in {report}"))
        .iter()
        .map(|u| {
            let target = match (u["to"].as_str(), u["external"].as_str()) {
                (Some(path), _) => path.to_string(),
                (None, Some(module)) => format!("external {module}"),
                (None, None) => panic!("no target in {u}"),
            };
            format!(
                "{}:{} -> {target}",
                u["from"].as_str().unwrap(),
                u["line"].as_u64().unwrap()
            )
        })
        .collect()
}

#[test]
fn an_import_the_plan_did_not_name_is_unplanned_and_the_verdict_stays() {
    let (_tmp, root) = web_and_core();
    write_all(
        &root,
        &[(
            "src/web/a.ts",
            "import { b } from \"../core/b\";\nimport { c } from \"../core/c\";\nexport const a = [b, c];\n",
        )],
    );
    let plan = r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"}]"#;
    let checked = check_plan(&root, plan);
    let report = json(&checked);
    assert_eq!(
        unplanned(&report),
        ["src/web/a.ts:2 -> src/core/c.ts"],
        "{report}"
    );
    let first = &report["plan"]["unplanned"][0];
    assert_eq!(first["component"], "web", "{report}");
    assert_eq!(first["target_component"], "core", "{report}");
    assert_eq!(
        report["plan"]["unrealized"],
        serde_json::json!([]),
        "{report}"
    );

    // Without the plan, the report is the same but for it.
    let without = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), without.outcome());
    let mut report = uncached(report);
    report.as_object_mut().unwrap().remove("plan");
    assert_eq!(report, uncached(json(&without)));
}

/// A report without what says whether its indexes came from the cache.
fn uncached(mut report: serde_json::Value) -> serde_json::Value {
    report.as_object_mut().unwrap().remove("cached");
    let stats = report["stats"].as_object_mut().unwrap();
    stats.remove("indexed");
    stats.remove("from_cache");
    report
}

#[test]
fn a_planned_directory_covers_an_import_of_a_file_inside_it() {
    let (_tmp, root) = web_and_core();
    let report = compared(
        &root,
        "import { c } from \"../core/c\";\nexport const a = c;\n",
        r#"[{"from": "src/web/a.ts", "to": "src/core"}]"#,
    );
    assert_eq!(unplanned(&report), Vec::<String>::new(), "{report}");
    assert_eq!(
        report["plan"]["unrealized"],
        serde_json::json!([]),
        "{report}"
    );
}

#[test]
fn a_new_external_module_is_unplanned_unless_the_plan_names_it_or_its_package() {
    let (_tmp, root) = web_and_core();
    let source = "import fp from \"lodash/fp\";\nexport const a = fp;\n";
    let report = compared(
        &root,
        source,
        r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"}]"#,
    );
    assert_eq!(
        unplanned(&report),
        ["src/web/a.ts:1 -> external lodash/fp"],
        "{report}"
    );
    assert_eq!(
        report["plan"]["unplanned"][0]["component"], "web",
        "{report}"
    );
    assert_eq!(
        report["plan"]["unplanned"][0]["target_component"],
        serde_json::Value::Null,
        "{report}"
    );
    for planned in ["lodash", "lodash/fp"] {
        let plan = format!(r#"[{{"from": "src/web/a.ts", "external": "{planned}"}}]"#);
        let report = compared(&root, source, &plan);
        assert_eq!(
            unplanned(&report),
            Vec::<String>::new(),
            "{planned}: {report}"
        );
        assert_eq!(
            report["plan"]["unrealized"],
            serde_json::json!([]),
            "{planned}: {report}"
        );
    }
}

#[test]
fn a_new_import_inside_one_component_is_not_compared() {
    let (_tmp, root) = web_and_core();
    let report = compared(
        &root,
        "import { other } from \"./other\";\nexport const a = other;\n",
        r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"}]"#,
    );
    assert_eq!(unplanned(&report), Vec::<String>::new(), "{report}");
}

/// An import is new as a dependency between components is new in
/// `new_dependencies`: when every line of it changed. An edited import line is
/// all of it, so it is reported, since the imports of the merge-base are not read.
#[test]
fn an_edited_import_line_is_new_as_new_dependencies_tells_it() {
    let (_tmp, root) = web_and_core();
    let before = "import { b } from \"../core/b\";\nexport const a = b;\n";
    write_all(&root, &[("src/web/a.ts", before)]);
    git(&root, &["commit", "-q", "-am", "web uses core"]);
    git(&root, &["branch", "-f", "main"]);
    let report = compared(
        &root,
        &before.replace("\"../core/b\"", "'../core/b'"),
        r#"[{"from": "src/web/a.ts", "to": "src/core/c.ts"}]"#,
    );
    assert_eq!(
        unplanned(&report),
        ["src/web/a.ts:1 -> src/core/b.ts"],
        "{report}"
    );
    assert_eq!(
        report["stats"]["new_dependencies"],
        serde_json::json!(["web -> core"]),
        "{report}"
    );
}

/// An existing import pointed at another file is a new import of that file,
/// although the lines that use what it imports did not change.
#[test]
fn an_import_pointed_at_another_file_is_new() {
    let (_tmp, root) = web_and_core();
    write_all(
        &root,
        &[(
            "src/web/a.ts",
            "import { b } from \"../core/b\";\nexport const a = b;\n",
        )],
    );
    git(&root, &["commit", "-q", "-am", "web uses core"]);
    git(&root, &["branch", "-f", "main"]);
    let report = compared(
        &root,
        "import { c as b } from \"../core/c\";\nexport const a = b;\n",
        r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"}]"#,
    );
    assert_eq!(
        unplanned(&report),
        ["src/web/a.ts:1 -> src/core/c.ts"],
        "{report}"
    );
}

/// A file of no component is a side of its own: what it imports crosses a boundary.
#[test]
fn an_import_of_a_file_of_no_component_is_compared() {
    let (_tmp, root) = web_and_core();
    write_all(
        &root,
        &[(
            "src/tools/x.ts",
            "import { b } from \"../core/b\";\nexport const x = b;\n",
        )],
    );
    let report = json(&check_plan(
        &root,
        r#"[{"from": "src/web/a.ts", "external": "lodash"}]"#,
    ));
    assert_eq!(
        unplanned(&report),
        ["src/tools/x.ts:1 -> src/core/b.ts"],
        "{report}"
    );
    let first = &report["plan"]["unplanned"][0];
    assert_eq!(first["component"], serde_json::Value::Null, "{report}");
    assert_eq!(first["target_component"], "core", "{report}");

    // Each file of no component is a side of its own.
    write_all(
        &root,
        &[
            (
                "src/tools/x.ts",
                "import { y } from \"./y\";\nexport const x = y;\n",
            ),
            ("src/tools/y.ts", "export const y = 1;\n"),
        ],
    );
    let report = json(&check_plan(
        &root,
        r#"[{"from": "src/web/a.ts", "external": "lodash"}]"#,
    ));
    assert_eq!(
        unplanned(&report),
        ["src/tools/x.ts:1 -> src/tools/y.ts"],
        "{report}"
    );
}

/// An import the change left alone is not compared, though its file changed.
#[test]
fn an_import_the_change_left_alone_is_not_compared() {
    let (_tmp, root) = web_and_core();
    let before = "import { b } from \"../core/b\";\nexport const a = b;\n";
    write_all(&root, &[("src/web/a.ts", before)]);
    git(&root, &["commit", "-q", "-am", "web uses core"]);
    git(&root, &["branch", "-f", "main"]);
    let report = compared(
        &root,
        &format!("{before}export const more = 2;\n"),
        r#"[{"from": "src/web/a.ts", "to": "src/core/c.ts"}]"#,
    );
    assert_eq!(unplanned(&report), Vec::<String>::new(), "{report}");
}

/// The checks do not read a file `exclude` hides, so the comparison cannot
/// tell what it imports: an entry of it is not reported as missing.
#[test]
fn an_entry_of_a_file_the_checks_do_not_read_is_not_unrealized() {
    let (_tmp, root) = web_and_core();
    let report = json(&check_plan(
        &root,
        r#"[{"from": "src/legacy/old.ts", "to": "src/core/b.ts"}]"#,
    ));
    assert_eq!(
        report["plan"]["unrealized"],
        serde_json::json!([]),
        "{report}"
    );
}

/// What no indexer read cannot be told to lack an import: the evidence gaps name
/// the file, and its entries are not reported as missing.
#[test]
fn an_entry_of_a_file_no_indexer_read_is_not_unrealized() {
    let (_tmp, root) = fixture("uncovered");
    commit_all(&root);
    let report = json(&check_plan(
        &root,
        r#"[{"from": "tools/gen.go", "external": "fmt"},
            {"from": "app/main.go", "external": "net/http"}]"#,
    ));
    assert_eq!(
        report["plan"]["unrealized"],
        serde_json::json!([{"from": "app/main.go", "external": "net/http"}]),
        "{report}"
    );
    assert_eq!(
        report["evidence_gaps"][0]["files"][0], "tools/gen.go",
        "{report}"
    );
}

#[test]
fn a_planned_import_the_code_lacks_is_unrealized() {
    let (_tmp, root) = web_and_core();
    let report = compared(
        &root,
        "import { b } from \"../core/b\";\nexport const a = b;\n",
        r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"},
            {"from": "./src/web/a.ts", "to": "src/core/c.ts"},
            {"from": "src/web/a.ts", "external": "lodash"},
            {"from": "src/web/new.ts", "to": "src/core/b.ts"}]"#,
    );
    assert_eq!(unplanned(&report), Vec::<String>::new(), "{report}");
    assert_eq!(
        report["plan"]["unrealized"],
        serde_json::json!([
            {"from": "src/web/a.ts", "to": "src/core/c.ts"},
            {"from": "src/web/a.ts", "external": "lodash"},
            {"from": "src/web/new.ts", "to": "src/core/b.ts"},
        ]),
        "{report}"
    );
}

#[test]
fn a_plan_needs_a_diff_and_is_read_as_propose_reads_it() {
    let (_tmp, root) = web_and_core();
    let plan = r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"}]"#;
    let said = error(try_check_with(&root, |o| o.plan = Some(plan.into())));
    assert!(said.contains("needs `--diff <base>`"), "{said}");
    for (plan, says) in [
        ("{", "a plan is a JSON list"),
        ("[]", "the plan proposes nothing"),
        (
            r#"[{"from": "a.ts"}]"#,
            "needs exactly one of `to` and `external`",
        ),
    ] {
        let said = error(try_check_with(&root, |o| {
            diff_main(o);
            o.plan = Some(plan.into());
        }));
        assert!(said.contains(says), "{plan}: {said}");
    }
}

/// Under `--scope`, only the imports and the entries of files in scope are compared.
#[test]
fn a_scoped_run_compares_only_the_files_in_scope() {
    let (_tmp, root) = web_and_core();
    write_all(
        &root,
        &[(
            "src/web/other.ts",
            "import { c } from \"../core/c\";\nexport const other = c;\n",
        )],
    );
    let checked = check_with(&root, |o| {
        diff_main(o);
        o.paths = vec!["src/web/a.ts".into()];
        o.plan = Some(
            r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"},
                {"from": "src/web/other.ts", "to": "src/core/b.ts"}]"#
                .into(),
        );
    });
    assert_eq!(checked.outcome(), Outcome::Passed);
    let said = text(&checked);
    assert!(
        said.contains("Planned, not in the code:\n  src/web/a.ts -> src/core/b.ts\n"),
        "{said}"
    );
    assert!(!said.contains("Not in the plan"), "{said}");
    assert!(!said.contains("matches the plan"), "{said}");
    let report = json(&checked);
    assert_eq!(unplanned(&report), Vec::<String>::new(), "{report}");
    assert_eq!(
        report["plan"]["unrealized"],
        serde_json::json!([{"from": "src/web/a.ts", "to": "src/core/b.ts"}]),
        "{report}"
    );
}

/// The report agents read of a change that departs from its plan both ways, and
/// of one that matches it.
#[test]
fn the_report_of_a_plan_is_pinned() {
    let (_tmp, root) = web_and_core();
    write_all(
        &root,
        &[(
            "src/web/a.ts",
            "import { c } from \"../core/c\";\nimport fp from \"lodash/fp\";\nexport const a = [c, fp];\n",
        )],
    );
    let plan = r#"[{"from": "src/web/a.ts", "to": "src/core/b.ts"},
                   {"from": "src/web/a.ts", "external": "zod"}]"#;
    let checked = check_plan(&root, plan);
    let mut report = uncached(json(&checked));
    let merge_base = report["diff"]["merge_base"].as_str().unwrap().to_string();
    report["diff"]["merge_base"] = "[merge-base]".into();
    // Whether a run read its index from the cache is not the report of the plan.
    let hide = |said: String| {
        said.replace(&merge_base[..12], "[merge-base]")
            .lines()
            .filter(|line| !line.contains("came from the cache"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let matched = check_plan(
        &root,
        r#"[{"from": "src/web/a.ts", "to": "src/core"}, {"from": "src/web/a.ts", "external": "lodash"}]"#,
    );
    pinned(|| {
        insta::assert_json_snapshot!("check_plan_json", report["plan"]);
        insta::assert_snapshot!("check_plan_text", hide(text(&checked)));
        insta::assert_snapshot!("check_plan_github", checked.report().github());
        insta::assert_snapshot!("check_plan_matched_text", hide(text(&matched)));
    });
}
