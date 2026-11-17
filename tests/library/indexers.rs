//! The indexers easement runs: what `doctor` says of them, and the settings
//! of `indexers:` that change what they read.

use std::fs;

use easement::commands::{self, CheckOptions, Outcome};
use easement::{Cache, CacheUse, Environment, Indexing, Jobs, Platform};

use crate::common::{fixture, project, write_all};
use crate::run::{cannot_index_here, check, config, json, parse, pinned, text};

/// Indexing as on Windows, by the indexers of the tests' environment, with no
/// cache: what the report says does not depend on what another test indexed.
fn on_windows() -> Cache {
    Cache::new(Indexing {
        cache: CacheUse::Off,
        cache_dir: None,
        environment: Environment::of_process().on(Platform::Windows),
        jobs: Jobs::default(),
    })
}

/// A Go project with Python scripts beside it.
fn go_and_python() -> (tempfile::TempDir, std::path::PathBuf) {
    let (tmp, root) = fixture("go");
    write_all(
        &root,
        &[
            ("scripts/seed.py", "print(1)\n"),
            ("scripts/util.py", "X = 1\n"),
        ],
    );
    (tmp, root)
}

/// On Windows scip-python cannot run: easement does not start it, its files are
/// uncovered with the reason, and the Go code is checked all the same.
#[test]
fn on_windows_the_files_of_an_indexer_that_cannot_run_there_are_uncovered_with_the_reason() {
    let (_tmp, root) = go_and_python();
    let checked = commands::check(&CheckOptions::new(config(&root)), &mut on_windows()).unwrap();
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    assert_eq!(
        report["stats"]["indexed"], 1,
        "only Go is indexed: {report}"
    );
    assert!(
        report["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["file"].as_str().unwrap().ends_with(".go")),
        "{report}"
    );
    pinned(|| {
        insta::assert_json_snapshot!("windows_uncovered_gaps_json", report["evidence_gaps"]);
        insta::assert_snapshot!("windows_uncovered_text", text(&checked));
    });

    // Accepted in `uncovered_ok`, the files are no evidence gap.
    let policy = fs::read_to_string(config(&root)).unwrap();
    fs::write(
        config(&root),
        format!("{policy}uncovered_ok:\n  - {{ name: scripts/**, because: checked on Linux }}\n"),
    )
    .unwrap();
    let checked = commands::check(&CheckOptions::new(config(&root)), &mut on_windows()).unwrap();
    let report = json(&checked);
    assert_eq!(report["evidence_gaps"], serde_json::json!([]), "{report}");
}

/// Off Windows the same project indexes its Python files.
#[test]
fn off_windows_the_python_files_are_indexed() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = go_and_python();
    let report = json(&check(&root));
    assert_eq!(report["evidence_gaps"], serde_json::json!([]), "{report}");
}

#[test]
fn doctor_reports_the_indexer_of_each_language() {
    let (_tmp, root) = fixture("uncovered");
    let diagnosed = commands::doctor(&config(&root), &Environment::of_process()).unwrap();
    assert_eq!(diagnosed.outcome(), Outcome::Passed);
    let said = diagnosed.text();
    assert!(said.contains("go: scip-go"), "{said}");
    assert!(said.contains("ok"), "{said}");
    let report = parse(&diagnosed.json());
    assert_eq!(report["ok"], true);
    assert_eq!(report["languages"][0]["language"], "go");
    assert_eq!(report["languages"][0]["binary"], "scip-go");
    assert_eq!(report["languages"][0]["found"], true);
}

#[test]
fn doctor_says_when_dependencies_are_not_installed() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src }\n",
        ),
        (
            "package.json",
            r#"{ "dependencies": { "left-pad": "1.0.0" } }"#,
        ),
        ("src/a.ts", "export const a = 1;\n"),
    ]);
    let diagnosed = commands::doctor(&config(&root), &Environment::of_process()).unwrap();
    assert_eq!(diagnosed.outcome(), Outcome::Failed);
    assert!(
        diagnosed.text().contains("dependencies are not installed"),
        "{}",
        diagnosed.text()
    );
}

#[test]
fn indexer_settings_bring_go_files_behind_a_build_constraint_into_the_index() {
    let (_tmp, root) = fixture("gotags");
    // The race file is not built without its tag: no indexer reads it.
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let said = text(&checked);
    assert!(said.contains("  pkg/race.go"), "{said}");
    assert!(said.contains("1 files not covered by an indexer"), "{said}");
    let policy = fs::read_to_string(config(&root)).unwrap();
    fs::write(
        config(&root),
        format!("{policy}indexers:\n  go: {{ tags: [race] }}\n"),
    )
    .unwrap();
    // Same files, other tags: the cached index of the first run must not be used.
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(
        !text(&checked).contains("not covered"),
        "{}",
        text(&checked)
    );
}

/// Without the indexer on the `PATH` of the environment it runs in, `doctor`
/// names the indexer and how to install it.
#[test]
fn doctor_names_a_missing_indexer_and_how_to_install_it() {
    let (_tmp, root) = fixture("uncovered");
    let diagnosed =
        commands::doctor(&config(&root), &Environment::of_process().with("PATH", "")).unwrap();
    assert_eq!(diagnosed.outcome(), Outcome::Failed);
    pinned(|| {
        insta::assert_snapshot!("doctor_missing_text", diagnosed.text());
        insta::assert_json_snapshot!("doctor_missing_json", parse(&diagnosed.json()));
    });
}

/// `doctor` on Windows names the indexers that cannot run there, with why and
/// how to check their files, and no install hint; it fails no more than a check
/// does, which runs and reports their files as uncovered.
#[test]
fn on_windows_doctor_names_the_indexers_that_cannot_run_there() {
    let (_tmp, root) = go_and_python();
    let windows = Environment::of_process().on(Platform::Windows);
    let diagnosed = commands::doctor(&config(&root), &windows).unwrap();
    assert_eq!(diagnosed.outcome(), Outcome::Passed, "{}", diagnosed.text());
    let report = parse(&diagnosed.json());
    assert_eq!(report["ok"], true);
    assert_eq!(report["languages"][0]["language"], "go");
    assert_eq!(
        report["languages"][0]["cannot_run"],
        serde_json::Value::Null
    );
    assert_eq!(report["languages"][1]["language"], "python");
    assert_eq!(
        report["languages"][1]["cannot_run"]["indexer"],
        "scip-python"
    );
    // Nothing to install, and the indexer is not started, not even for its version.
    for field in ["install", "mise", "version", "path"] {
        assert_eq!(
            report["languages"][1][field],
            serde_json::Value::Null,
            "{field}: {report}"
        );
    }
    assert!(report["languages"][0]["install"].is_string(), "{report}");

    // Not even missing, the indexer is no install hint there.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\n",
        ),
        ("app/a.py", "print(1)\n"),
    ]);
    let diagnosed = commands::doctor(&config(&root), &windows.with("PATH", "")).unwrap();
    assert_eq!(diagnosed.outcome(), Outcome::Passed, "{}", diagnosed.text());
    pinned(|| insta::assert_snapshot!("windows_doctor_text", diagnosed.text()));
}
