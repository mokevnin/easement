//! What a verdict rests on: the evidence gaps of a run, the files no indexer
//! read and the ones `accepted_gaps` accepts, what `--require-evidence` does
//! with them, and what a narrowed run (`--diff`, `--component`, `--scope`)
//! says it left out.

use std::fs;
use std::path::Path;

use easement::commands::{self, CheckOptions, Checked, Outcome, Proposed};
use easement::{CacheUse, Coverage, GapKind, GraphFormat, Place};
use tempfile::TempDir;

use crate::common::{commit_all, fixture, project};
use crate::run::{
    cache, cache_in, check, check_with, config, json, leaves_out, parse, pinned, text,
};

/// The message of each gap of `kind`.
fn gaps(checked: &Checked, kind: GapKind) -> Vec<&str> {
    checked
        .coverage
        .evidence_gaps
        .iter()
        .filter(|gap| gap.kind == kind)
        .map(|gap| gap.message.as_str())
        .collect()
}

/// scip-go resolves one side of an import cycle only: the import it dropped is
/// an evidence gap that names it, not a dependency that does not exist, and
/// not a clean pass on the forbidden `a -> b` it hides.
#[test]
fn an_import_the_indexer_did_not_resolve_is_an_evidence_gap() {
    let (_tmp, root) = fixture("go_import_cycle");
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    let [gap] = checked.coverage.evidence_gaps.as_slice() else {
        panic!("one gap: {:?}", checked.coverage.evidence_gaps);
    };
    assert_eq!(gap.kind, GapKind::UnresolvedImports);
    assert_eq!(gap.files, ["a/a.go"]);
    assert_eq!(
        gap.places,
        [Place {
            file: "a/a.go".into(),
            line: 3,
            specifier: Some("example.com/repro/b".into()),
        }]
    );
}

/// A source file easement cannot read leaves out what it imports: the gap says
/// it could not be read, not that no indexer covered it, and how to close it.
#[test]
#[cfg(unix)]
fn a_source_file_that_cannot_be_read_is_an_evidence_gap_that_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root) = fixture("kotlin");
    let controller = root.join("src/main/kotlin/com/acme/shop/web/OrderController.kt");
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o000)).unwrap();
    let checked = check(&root);
    fs::set_permissions(&controller, fs::Permissions::from_mode(0o644)).unwrap();
    let [gap] = checked.coverage.evidence_gaps.as_slice() else {
        panic!("one gap: {:?}", checked.coverage.evidence_gaps);
    };
    assert_eq!(gap.kind, GapKind::Uncovered);
    assert_eq!(
        gap.files,
        ["src/main/kotlin/com/acme/shop/web/OrderController.kt"]
    );
    assert!(gap.message.contains("could not be read"), "{}", gap.message);
    assert!(!gap.message.contains("indexer"), "{}", gap.message);
}

/// An import the indexer dropped is a dependency missing from the graph, so
/// every answer about the graph says it may be short.
#[test]
fn answers_about_the_graph_say_an_unresolved_import_may_hide_a_dependency() {
    let (_tmp, root) = fixture("go_import_cycle");
    let config = root.join("easement.yml");
    let says_so = |coverage: &Coverage| {
        coverage
            .evidence_gaps
            .iter()
            .any(|gap| gap.kind == GapKind::UnresolvedImports && gap.files == ["a/a.go"])
    };
    let why = commands::why(&config, "a", "b", &mut cache()).unwrap();
    assert!(says_so(&why.coverage), "why: {:?}", why.coverage);
    let impacted = commands::impact(&config, &[root.join("b/b.go")], None, &mut cache()).unwrap();
    assert!(
        says_so(&impacted.impact.coverage),
        "impact: {:?}",
        impacted.impact.coverage
    );
    let proposed = Proposed::One {
        source: root.join("b/b.go"),
        target: Some(root.join("a/a.go")),
        external: None,
    };
    let report = commands::propose(&config, &proposed, false, &mut cache()).unwrap();
    assert!(says_so(&report.coverage), "propose: {:?}", report.coverage);
}

/// A `forbid_symbol` item that a deeper name shows to be misspelled fails as `unmatched` only
/// over a whole index (ADR 0004): an import the indexer dropped leaves it short,
/// so the item says nothing until the import is resolved.
#[test]
fn a_forbid_symbol_that_can_never_fire_waits_for_an_unresolved_import() {
    let (_tmp, root) = fixture("go_import_cycle");
    fs::write(
        root.join("a/t.go"),
        "package a\n\ntype T struct{}\n\nfunc (T) M() {}\n",
    )
    .unwrap();
    let config = root.join("easement.yml");
    let policy = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        format!("{policy}forbid_symbol: [\"example.com/repro/a.M\"]\n"),
    )
    .unwrap();
    let unmatched = |checked: &Checked| {
        checked
            .verdict
            .new
            .iter()
            .filter(|v| {
                v.message.contains("`example.com/repro/a.M`")
                    && v.message.contains("can never fire")
            })
            .count()
    };
    let checked = check(&root);
    assert!(!gaps(&checked, GapKind::UnresolvedImports).is_empty());
    assert_eq!(unmatched(&checked), 0, "{:?}", checked.verdict.new);

    // With the cycle gone, the index is whole and the item can be judged.
    fs::write(root.join("a/a.go"), "package a\n").unwrap();
    let checked = check(&root);
    assert!(gaps(&checked, GapKind::UnresolvedImports).is_empty());
    assert_eq!(unmatched(&checked), 1, "{:?}", checked.verdict.new);
}

/// The message names the first three unresolved imports and counts the rest;
/// `places` holds every one of them.
#[test]
fn the_message_names_three_unresolved_imports_and_places_holds_them_all() {
    let (_tmp, root) = fixture("go_import_cycle");
    for name in ["c", "d", "e"] {
        fs::write(
            root.join(format!("a/{name}.go")),
            "package a\n\nimport _ \"example.com/repro/b\"\n",
        )
        .unwrap();
    }
    let checked = check(&root);
    let [gap] = checked.coverage.evidence_gaps.as_slice() else {
        panic!("one gap: {:?}", checked.coverage.evidence_gaps);
    };
    assert_eq!(
        gap.message,
        "the indexer did not resolve 4 import(s), so the dependencies they write were not checked (for example, a package that does not type-check, or an import cycle): a/a.go:3 (`example.com/repro/b`), a/c.go:3 (`example.com/repro/b`), a/d.go:3 (`example.com/repro/b`) and 1 more"
    );
    let places: Vec<(&str, usize, Option<&str>)> = gap
        .places
        .iter()
        .map(|p| (p.file.as_str(), p.line, p.specifier.as_deref()))
        .collect();
    let b = Some("example.com/repro/b");
    assert_eq!(
        places,
        [
            ("a/a.go", 3, b),
            ("a/c.go", 3, b),
            ("a/d.go", 3, b),
            ("a/e.go", 3, b)
        ]
    );
    assert_eq!(gap.count, Some(4));
}

/// An import is resolved by a module on its own path, not by any on its line:
/// the dropped import of `b` beside a resolved `os` is still a gap.
#[test]
fn an_unresolved_import_beside_a_resolved_one_on_its_line_is_an_evidence_gap() {
    let (_tmp, root) = fixture("go_import_cycle");
    fs::write(
        root.join("a/a.go"),
        "package a\n\nimport ( \"os\"; _ \"example.com/repro/b\" )\n\nvar _ = os.Args\n",
    )
    .unwrap();
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    assert_eq!(
        gaps(&checked, GapKind::UnresolvedImports),
        [
            "the indexer did not resolve 1 import(s), so the dependencies they write were not checked (for example, a package that does not type-check, or an import cycle): a/a.go:3 (`example.com/repro/b`)"
        ]
    );
}

/// Every way Go writes an import is resolved by the indexer, so none is a gap.
/// An import of a package the module does not have is a dependency on the
/// module it spells, which the rules judge: never silent.
#[test]
fn every_form_of_a_go_import_is_resolved_and_a_missing_package_is_judged() {
    let (_tmp, root) = fixture("go_imports");
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert!(
        checked.coverage.evidence_gaps.is_empty(),
        "{:?}",
        checked.coverage.evidence_gaps
    );
    assert_eq!(checked.outcome(), Outcome::Passed);

    // scip-go files the missing package under the standard library, but `$std`
    // is told by the path, which names a domain.
    fs::write(
        root.join("lib/missing.go"),
        "package lib\n\nimport \"example.com/forms/nowhere\"\n\nvar _ = nowhere.X\n",
    )
    .unwrap();
    let checked = check(&root);
    assert!(
        checked
            .verdict
            .new
            .iter()
            .any(|v| v.file == "lib/missing.go" && v.line == 3),
        "{:?}",
        checked.verdict.new
    );
    assert!(gaps(&checked, GapKind::UnresolvedImports).is_empty());
}

/// cgo's `import "C"` names no package, so it is never an unresolved import,
/// whether or not the indexer could read the file (that takes a C toolchain).
#[test]
fn cgo_is_no_unresolved_import() {
    let (_tmp, root) = fixture("go_imports");
    fs::write(
        root.join("app/cgo.go"),
        "package app\n\n// #include <stdlib.h>\nimport \"C\"\n\nimport \"os\"\n\nvar _ = os.Args\n",
    )
    .unwrap();
    let checked = check(&root);
    assert!(
        checked
            .coverage
            .evidence_gaps
            .iter()
            .all(|gap| gap.kind != GapKind::UnresolvedImports),
        "{:?}",
        checked.coverage.evidence_gaps
    );
}

/// Appends `text` to the policy of the project at `root`.
fn add_to_policy(root: &Path, text: &str) {
    let policy = fs::read_to_string(config(root)).unwrap();
    fs::write(config(root), format!("{policy}{text}")).unwrap();
}

#[test]
fn uncovered_files_are_listed_as_an_evidence_gap() {
    let (_tmp, root) = fixture("uncovered");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let said = text(&checked);
    assert!(said.contains("Evidence gaps"), "{said}");
    assert!(
        said.contains("to close: add them to a project the indexer reads"),
        "{said}"
    );
    assert!(said.contains("    tools/gen.go"), "{said}");
    assert!(said.contains("    tools/other.go"), "{said}");
    assert!(!said.contains("Accepted gaps"), "{said}");
    assert!(said.contains("2 files not covered by an indexer"), "{said}");
    let report = json(&checked);
    assert_eq!(report["passed"], true);
    assert_eq!(
        report["evidence_gaps"][0]["message"],
        "2 source files were not covered by an indexer, so the dependencies in or on them were not checked"
    );
    assert_eq!(report["narrowed"], serde_json::json!([]));
    assert_eq!(report["accepted_gaps"], serde_json::json!([]));
    let gaps = report["evidence_gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0]["kind"], "uncovered");
    assert_eq!(gaps[0]["count"], 2);
    assert_eq!(
        gaps[0]["files"],
        serde_json::json!(["tools/gen.go", "tools/other.go"])
    );
    assert!(
        gaps[0]["close"].as_str().unwrap().contains("accepted_gaps"),
        "{gaps:?}"
    );
}

/// The go tool ignores a committed `vendor/` and every `testdata/`: the code in
/// them is not the project's own, so no indexer leaves it uncovered and the
/// project's files do not count it.
#[test]
fn vendored_and_test_data_go_files_are_not_project_code_left_uncovered() {
    let (_tmp, root) = fixture("go_vendor");
    let checked = check_with(&root, |o| o.require_evidence = true);
    let report = json(&checked);
    assert_eq!(
        report["evidence_gaps"],
        serde_json::json!([]),
        "{}",
        text(&checked)
    );
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert_eq!(report["stats"]["files"], 2);
}

#[test]
fn accepted_gaps_accepts_known_uncovered_files() {
    let (_tmp, root) = fixture("uncovered");
    add_to_policy(
        &root,
        "accepted_gaps:\n  - { kind: uncovered, files: 'tools/**', because: build helpers with no go.mod on purpose }\n",
    );
    // Accepted, the files are no evidence gap, but they are still reported.
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let said = text(&checked);
    assert!(!said.contains("Evidence gaps"), "{said}");
    assert!(!said.contains("not covered by an indexer"), "{said}");
    assert!(said.contains("Accepted gaps"), "{said}");
    assert!(
        said.contains(
            "  accepted_gaps[0] at easement.yml:7 because: build helpers with no go.mod on purpose\n"
        ),
        "{said}"
    );
    assert!(said.contains("    tools/gen.go"), "{said}");
    let report = json(&checked);
    assert_eq!(report["evidence_gaps"], serde_json::json!([]));
    assert_eq!(
        report["accepted_gaps"],
        serde_json::json!([{
            "kind": "uncovered",
            "because": "build helpers with no go.mod on purpose",
            "provenance": { "field": "accepted_gaps[0]", "input": "easement.yml" },
            "line": 7,
            "files": ["tools/gen.go", "tools/other.go"],
        }])
    );
}

/// An accepted file is still unread: `suggest`, `impact` and `baseline` cannot
/// see its imports, so they stay as careful as without `accepted_gaps`.
#[test]
fn an_accepted_file_passes_check_but_stays_unread_for_suggest_and_impact() {
    let (_tmp, root) = project(&[
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1
components:
  app: { in: app }
  lib: { in: lib }
rules:
  app: { may_depend_on: [lib] }
accepted_gaps:
  - { kind: uncovered, files: app/race.go, because: race-only helpers }
",
        ),
        ("app/main.go", "package app\n\nfunc Run() {}\n"),
        (
            "app/race.go",
            "//go:build race\n\npackage app\n\nimport \"example.com/m/lib\"\n\nfunc R() { lib.L() }\n",
        ),
        ("lib/lib.go", "package lib\n\nfunc L() {}\n"),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert!(!leaves_out(&report), "{report}");

    // The import of lib is in the file no indexer read.
    let config = config(&root);
    let found = commands::suggest(&config, &mut cache()).unwrap();
    assert!(found.suggestions.removable.is_empty());
    let checked = check_with(&root, |o| o.suggest = true);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    let said = found.text();
    assert!(
        said.contains("Evidence gaps (a suggestion about this code may be wrong):"),
        "{said}"
    );
    assert!(said.contains("    app/race.go\n"), "{said}");
    let patched = commands::suggest_patch(&config, &mut cache()).unwrap();
    assert!(!patched.diff.contains("may_depend_on"), "{}", patched.diff);
    let impacted =
        commands::impact(&config, &[root.join("lib/lib.go")], None, &mut cache()).unwrap();
    let impact = parse(&impacted.json().unwrap());
    assert!(leaves_out(&impact), "{impact}");
    assert_eq!(
        impact["evidence_gaps"][0]["files"],
        serde_json::json!(["app/race.go"])
    );
    assert_eq!(
        impact["accepted_gaps"],
        serde_json::json!([{
            "kind": "uncovered",
            "because": "race-only helpers",
            "provenance": { "field": "accepted_gaps[0]", "input": "easement.yml" },
            "line": 8,
            "files": ["app/race.go"],
        }])
    );
    // A drawing without the dependencies of the accepted file says so beside it.
    let drawn = commands::graph(&config, GraphFormat::Mermaid, None, &mut cache()).unwrap();
    let notes = drawn.notes();
    assert!(
        notes.contains(
            "Evidence gaps (an arrow through this code may be missing from the drawing):\n"
        ),
        "{notes}"
    );
    assert!(notes.contains("    to close: "), "{notes}");
    assert!(notes.contains("    app/race.go\n"), "{notes}");
    assert!(notes.contains("Accepted gaps (`accepted_gaps`"), "{notes}");
    assert!(
        notes.contains("  accepted_gaps[0] at easement.yml:8 because: race-only helpers\n"),
        "{notes}"
    );
    assert!(
        !drawn.diagram.contains("Evidence gaps"),
        "{}",
        drawn.diagram
    );
}

/// `--require-evidence` fails a pass that rests on code the check could not read,
/// with `Outcome::EvidenceRequired`; a gap accepted in `accepted_gaps` does not count.
#[test]
fn require_evidence_fails_a_pass_with_an_evidence_gap() {
    let (_tmp, root) = fixture("uncovered");
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    let said = text(&checked);
    assert!(said.contains("Evidence required"), "{said}");
    assert!(said.contains("    tools/gen.go"), "{said}");
    assert!(said.contains("easement failed"), "{said}");
    let report = json(&checked);
    assert_eq!(report["passed"], false);
    assert_eq!(report["evidence_gaps"][0]["kind"], "uncovered");
    // Without the flag the same run passes, as before.
    assert_eq!(check(&root).outcome(), Outcome::Passed);

    add_to_policy(
        &root,
        "accepted_gaps:\n  - { kind: uncovered, files: tools/**, because: build helpers }\n",
    );
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert_eq!(report["passed"], true);
    assert_eq!(
        report["accepted_gaps"][0]["files"],
        serde_json::json!(["tools/gen.go", "tools/other.go"])
    );
}

/// An import the indexer did not resolve (scip-go drops one side of an import
/// cycle) fails `--require-evidence`, is named in the text, and is the
/// `unresolved_imports` gap of the JSON, from the cache too; what the gap holds
/// is tested by `an_import_the_indexer_did_not_resolve_is_an_evidence_gap`.
#[test]
fn an_import_the_indexer_did_not_resolve_fails_require_evidence() {
    let (_tmp, root) = fixture("go_import_cycle");
    let cache = TempDir::new().unwrap();
    let run = || {
        let mut options = CheckOptions::new(config(&root));
        options.require_evidence = true;
        commands::check(&options, &mut cache_in(cache.path(), CacheUse::On)).unwrap()
    };
    let checked = run();
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    let places = |checked: &Checked| {
        checked
            .coverage
            .evidence_gaps
            .iter()
            .flat_map(|gap| gap.places.clone())
            .collect::<Vec<_>>()
    };
    let fresh = places(&checked);
    assert!(!fresh.is_empty());
    let said = text(&checked);
    assert!(said.contains("Evidence required"), "{said}");
    // The gap lists the place of the import, not only its file.
    assert!(
        said.contains("\n    a/a.go:3 (`example.com/repro/b`)\n"),
        "{said}"
    );
    // The second run reads the index from the cache, and the source beside it.
    let checked = run();
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    assert_eq!(places(&checked), fresh, "a cache hit gives the same places");
    let report = json(&checked);
    assert!(report["cached"].is_string(), "{report}");
    pinned(|| {
        insta::assert_json_snapshot!("unresolved_imports_gap", report["evidence_gaps"]);
    });
}

/// A scope narrows what is reported, not what is read: a file outside it that
/// no indexer read can hide a cycle or a reach chain through the scope, so its
/// gap counts, and `accepted_gaps` still accepts it.
#[test]
fn require_evidence_counts_the_gaps_outside_a_scope() {
    let (_tmp, root) = fixture("uncovered");
    let by_component = |o: &mut CheckOptions| o.components = vec!["app".into()];
    let by_scope = |o: &mut CheckOptions| o.paths = vec!["app".into()];
    for (narrowed, narrow) in [
        (
            "--component app",
            &by_component as &dyn Fn(&mut CheckOptions),
        ),
        ("--scope app", &by_scope),
    ] {
        let checked = check_with(&root, |o| {
            o.require_evidence = true;
            narrow(o);
        });
        assert_eq!(checked.outcome(), Outcome::EvidenceRequired, "{narrowed}");
        let report = json(&checked);
        assert_eq!(
            report["evidence_gaps"][0]["files"],
            serde_json::json!(["tools/gen.go", "tools/other.go"]),
            "{narrowed}: {report}"
        );
    }

    add_to_policy(
        &root,
        "accepted_gaps:\n  - { kind: uncovered, files: tools/**, because: build helpers }\n",
    );
    let checked = check_with(&root, |o| {
        o.require_evidence = true;
        o.components = vec!["app".into()];
    });
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert_eq!(
        json(&checked)["accepted_gaps"][0]["files"],
        serde_json::json!(["tools/gen.go", "tools/other.go"])
    );
}

/// A narrowed run reads the whole project all the same, so `--diff` alone is no
/// reason to fail; that is the combination CI is meant to run.
#[test]
fn require_evidence_passes_a_narrowed_run_without_gaps() {
    let (_tmp, root) = fixture("go");
    commit_all(&root);
    let checked = check_with(&root, |o| {
        o.diff = Some("main".into());
        o.require_evidence = true;
    });
    assert_eq!(checked.outcome(), Outcome::Passed);
}

/// Violations come before missing evidence: such a run fails (exit code 1), it
/// does not ask for evidence (exit code 4).
#[test]
fn require_evidence_leaves_a_run_with_violations_failed() {
    let (_tmp, root) = project(&[
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\n  lib: { in: lib }\n",
        ),
        (
            "app/main.go",
            "package app\n\nimport \"example.com/m/lib\"\n\nfunc Run() { lib.L() }\n",
        ),
        // Built only with the `race` tag: no indexer reads it.
        ("app/race.go", "//go:build race\n\npackage app\n"),
        ("lib/lib.go", "package lib\n\nfunc L() {}\n"),
    ]);
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    assert_eq!(report["violations"][0]["rule"], "dependency", "{report}");
    assert_eq!(
        report["evidence_gaps"][0]["files"],
        serde_json::json!(["app/race.go"])
    );
}

#[test]
fn an_accepted_gap_takes_a_list_of_globs_under_one_reason() {
    let (_tmp, root) = fixture("uncovered");
    let policy = fs::read_to_string(config(&root)).unwrap();
    fs::write(
        config(&root),
        format!(
            "{policy}accepted_gaps:\n  - {{ kind: uncovered, files: ['tools/gen.go', 'tools/other.go'], because: build helpers }}\n"
        ),
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(
        !text(&checked).contains("not covered by an indexer"),
        "{}",
        text(&checked)
    );
    // One glob of the list is not enough for the other file.
    fs::write(
        config(&root),
        format!(
            "{policy}accepted_gaps:\n  - {{ kind: uncovered, files: ['tools/gen.go'], because: build helpers }}\n"
        ),
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert_eq!(
        report["evidence_gaps"][0]["files"],
        serde_json::json!(["tools/other.go"])
    );
    assert_eq!(
        report["accepted_gaps"][0]["files"],
        serde_json::json!(["tools/gen.go"])
    );
}

/// An entry that accepts nothing is a typo or a gap gone: it fails as
/// `unmatched` at its line, which the baseline cannot accept, while an entry
/// whose files are all accepted by an earlier one still matches them.
#[test]
fn an_accepted_gap_that_accepts_nothing_fails_as_unmatched_at_its_line() {
    let (_tmp, root) = fixture("uncovered");
    add_to_policy(
        &root,
        "accepted_gaps:\n  - { kind: uncovered, files: tools/**, because: build helpers }\n  - { kind: uncovered, files: tools/gen.go, because: the generator }\n  - { kind: uncovered, files: [app/**, gone/**], because: an old helper }\n",
    );
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed, "{}", text(&checked));
    let report = json(&checked);
    let violations = report["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "{report}");
    assert_eq!(violations[0]["rule"], "unmatched");
    assert_eq!(violations[0]["file"], "easement.yml");
    assert_eq!(violations[0]["line"], 9);
    assert_eq!(violations[0]["target"], "app/**, gone/**");
    assert!(
        violations[0]["message"]
            .as_str()
            .unwrap()
            .contains("`accepted_gaps[2]` accepts no uncovered file"),
        "{report}"
    );
    // The baseline never accepts a mistake in the policy, and `--diff` keeps it,
    // though no changed line is behind it.
    let recorded = commands::baseline(&config(&root), false, &mut cache()).unwrap();
    assert_eq!((recorded.written, recorded.config_rules), (0, 1));
    assert_eq!(check(&root).outcome(), Outcome::Failed);
    commit_all(&root);
    let narrowed = check_with(&root, |o| o.diff = Some("main".into()));
    assert_eq!(narrowed.outcome(), Outcome::Failed, "{}", text(&narrowed));
    assert_eq!(json(&narrowed)["violations"][0]["rule"], "unmatched");
}

/// A file easement could not read is accepted like any unread file, so an
/// entry that names only such a file accepts it and is not stale.
#[test]
fn an_accepted_gap_over_an_unreadable_file_accepts_it_and_is_not_stale() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root) = fixture("kotlin");
    let file = "src/main/kotlin/com/acme/shop/web/OrderController.kt";
    add_to_policy(
        &root,
        &format!(
            "accepted_gaps:\n  - {{ kind: uncovered, files: {file}, because: kept unreadable }}\n"
        ),
    );
    fs::set_permissions(root.join(file), fs::Permissions::from_mode(0o000)).unwrap();
    let checked = check(&root);
    fs::set_permissions(root.join(file), fs::Permissions::from_mode(0o644)).unwrap();
    let report = json(&checked);
    assert_eq!(report["evidence_gaps"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["accepted_gaps"][0]["files"],
        serde_json::json!([file])
    );
    assert!(
        !report["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["rule"] == "unmatched"),
        "{report}"
    );
}

#[test]
fn a_narrowed_run_says_it_judged_only_a_part() {
    let (_tmp, root) = fixture("symbol_uses");
    let whole = json(&check(&root));
    assert_eq!(whole["narrowed"], serde_json::json!([]));
    let scoped = check_with(&root, |o| o.paths = vec!["internal/db".into()]);
    assert_eq!(
        json(&scoped)["narrowed"],
        serde_json::json!([{ "kind": "scope", "limited_to": "internal/db" }])
    );
    assert_eq!(scoped.outcome(), Outcome::Passed);
    assert!(
        text(&scoped).contains("limited to path internal/db"),
        "{}",
        text(&scoped)
    );
    let by_component = check_with(&root, |o| {
        o.components = vec!["db".into()];
        o.paths = vec!["internal".into()];
    });
    assert_eq!(
        json(&by_component)["narrowed"],
        serde_json::json!([
            { "kind": "component", "limited_to": "db" },
            { "kind": "scope", "limited_to": "internal" },
        ])
    );
}
