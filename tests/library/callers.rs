//! `impact --diff` beside the files that depend on a change: the definitions the
//! change touched and the places the head index says reference them. A report of
//! what to re-check, never a claim that a caller is broken or that the list is whole.

use std::fs;
use std::path::Path;

use easement::commands;
use serde_json::Value;
use tempfile::TempDir;

use crate::common::{commit_all, project};
use crate::run::{cache, config, parse, pinned};

const POLICY: &str = "version: 1\ncomponents:\n  lib: { in: src/lib }\n  app: { in: src/app }\nrules:\n  app: { may_depend_on: [lib] }\n";

/// What `impact --diff main` says of the project at `root`, as data.
fn impact_diff(root: &Path) -> Value {
    let impacted = commands::impact(&config(root), &[], Some("main"), &mut cache()).unwrap();
    parse(&impacted.json().unwrap())
}

/// The files of the callers of each touched entry, by the entry's name (or part).
fn callers_by_entry(report: &Value) -> Vec<(String, Vec<String>)> {
    report["touched"]
        .as_array()
        .unwrap_or_else(|| panic!("no `touched`: {report}"))
        .iter()
        .map(|entry| {
            let name = entry["name"]
                .as_str()
                .or_else(|| entry["part"].as_str())
                .unwrap()
                .to_string();
            let mut files: Vec<String> = entry["callers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|caller| caller["file"].as_str().unwrap().to_string())
                .collect();
            files.dedup();
            (name, files)
        })
        .collect()
}

fn dependents(report: &Value) -> Vec<&str> {
    report["dependents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect()
}

/// Every caller is a changed file or one of the dependents: the dependents stay the
/// whole answer.
fn callers_within_dependents(report: &Value) {
    let changed: Vec<&str> = report["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    let dependents = dependents(report);
    for entry in report["touched"].as_array().unwrap() {
        for caller in entry["callers"].as_array().unwrap() {
            let file = caller["file"].as_str().unwrap();
            assert!(
                changed.contains(&file) || dependents.contains(&file),
                "{file} is a caller but no dependent: {report}"
            );
        }
    }
}

const SHAPES: &str = "export function area(w: number, h: number): number {\n  const product = w * h;\n  return product;\n}\n\nexport function perimeter(w: number, h: number): number {\n  return 2 * (w + h);\n}\n";

/// A TypeScript module of two functions, each called by one file of `app`.
fn shapes() -> (TempDir, std::path::PathBuf) {
    let (tmp, root) = project(&[
        ("easement.yml", POLICY),
        ("src/lib/shapes.ts", SHAPES),
        (
            "src/app/a.ts",
            "import { area } from \"../lib/shapes\";\nexport const a = area(1, 2);\n",
        ),
        (
            "src/app/b.ts",
            "import { perimeter } from \"../lib/shapes\";\nexport const b = perimeter(1, 2);\n",
        ),
    ]);
    commit_all(&root);
    (tmp, root)
}

#[test]
fn a_change_to_one_function_names_only_its_callers() {
    let (_tmp, root) = shapes();
    fs::write(
        root.join("src/lib/shapes.ts"),
        SHAPES.replace("w * h;", "w * h * 1;"),
    )
    .unwrap();
    let report = impact_diff(&root);
    assert_eq!(
        callers_by_entry(&report),
        [("area".to_string(), vec!["src/app/a.ts".to_string()])],
        "{report}"
    );
    assert_eq!(
        dependents(&report),
        ["src/app/a.ts", "src/app/b.ts"],
        "{report}"
    );
    callers_within_dependents(&report);
}

const GO_POLICY: &str = "version: 1\ncomponents:\n  lib: { in: lib }\n  app: { in: app }\nrules:\n  app: { may_depend_on: [lib] }\n";

const GO_SHAPES: &str = "package lib\n\nfunc Area(w, h int) int {\n\tproduct := w * h\n\treturn product\n}\n\nfunc Perimeter(w, h int) int {\n\treturn 2 * (w + h)\n}\n";

#[test]
fn a_change_to_one_go_function_names_only_its_callers() {
    let (_tmp, root) = project(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("easement.yml", GO_POLICY),
        ("lib/shapes.go", GO_SHAPES),
        (
            "app/a.go",
            "package app\n\nimport \"example.com/app/lib\"\n\nfunc A() int { return lib.Area(1, 2) }\n",
        ),
        (
            "app/b.go",
            "package app\n\nimport \"example.com/app/lib\"\n\nfunc B() int { return lib.Perimeter(1, 2) }\n",
        ),
    ]);
    commit_all(&root);
    fs::write(
        root.join("lib/shapes.go"),
        GO_SHAPES.replace("w * h\n", "w * h * 1\n"),
    )
    .unwrap();
    let report = impact_diff(&root);
    assert_eq!(
        callers_by_entry(&report),
        [("Area".to_string(), vec!["app/a.go".to_string()])],
        "{report}"
    );
    assert_eq!(dependents(&report), ["app/a.go", "app/b.go"], "{report}");
    callers_within_dependents(&report);
}

/// A body that only loses lines touches its function, and nothing else.
#[test]
fn a_body_that_only_loses_lines_touches_its_function() {
    let (_tmp, root) = shapes();
    fs::write(
        root.join("src/lib/shapes.ts"),
        SHAPES.replace("  return product;\n", ""),
    )
    .unwrap();
    let report = impact_diff(&root);
    assert_eq!(
        callers_by_entry(&report),
        [("area".to_string(), vec!["src/app/a.ts".to_string()])],
        "{report}"
    );
    assert!(
        report["evidence_gaps"].as_array().unwrap().is_empty(),
        "{report}"
    );
}

/// A change outside every function touches the file's own code, whose callers use
/// its classes and constants; a file that only calls one of its functions is none.
#[test]
fn a_change_to_top_level_code_touches_the_files_own_code() {
    let rates = "export const RATE = 2;\n\nexport class Meter {}\n\nexport function scale(x: number): number {\n  const y = x;\n  return y * RATE;\n}\n";
    let (_tmp, root) = project(&[
        ("easement.yml", POLICY),
        ("src/lib/rates.ts", rates),
        (
            "src/app/a.ts",
            "import { RATE } from \"../lib/rates\";\nexport const a = RATE;\n",
        ),
        (
            "src/app/b.ts",
            "import { scale } from \"../lib/rates\";\nexport const b = scale(1);\n",
        ),
        (
            "src/app/c.ts",
            "import { Meter } from \"../lib/rates\";\nexport const c = new Meter();\n",
        ),
    ]);
    commit_all(&root);
    fs::write(
        root.join("src/lib/rates.ts"),
        rates.replace("RATE = 2", "RATE = 3"),
    )
    .unwrap();
    let report = impact_diff(&root);
    // `scale` uses the constant from a method of the file: a caller too.
    assert_eq!(
        callers_by_entry(&report),
        [(
            "own_code".to_string(),
            vec![
                "src/app/a.ts".to_string(),
                "src/app/c.ts".to_string(),
                "src/lib/rates.ts".to_string()
            ]
        )],
        "{report}"
    );
    // Its line that uses the constant, not the one that only reads its parameter,
    // which scip-typescript names as a symbol of its own (`scale().(x)`).
    let own: Vec<&Value> = report["touched"][0]["callers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|caller| caller["file"] == "src/lib/rates.ts")
        .map(|caller| &caller["line"])
        .collect();
    assert_eq!(own, [7], "{report}");
    callers_within_dependents(&report);
}

/// The head index no longer has a definition the change deleted: it is not touched,
/// and an evidence gap says its former callers are not named. The lines it held lay
/// outside every function the index has, in the file's own code.
#[test]
fn a_deleted_function_is_an_evidence_gap() {
    let (_tmp, root) = shapes();
    fs::write(
        root.join("src/lib/shapes.ts"),
        SHAPES.split("\nexport function perimeter").next().unwrap(),
    )
    .unwrap();
    let report = impact_diff(&root);
    assert_eq!(
        callers_by_entry(&report),
        [("own_code".to_string(), Vec::<String>::new())],
        "{report}"
    );
    let gaps = report["evidence_gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(gaps[0]["kind"], "removed_definitions", "{report}");
    assert_eq!(gaps[0]["files"], serde_json::json!(["src/lib/shapes.ts"]));
    assert!(
        gaps[0]["message"]
            .as_str()
            .unwrap()
            .contains("former callers are not named"),
        "{report}"
    );
}

/// Lines deleted outside every function change the file's own code: it is touched,
/// with the uses of its classes and constants as callers, and an evidence gap says
/// that what the lines held may be gone.
#[test]
fn lines_deleted_outside_every_function_touch_the_files_own_code() {
    let rates = "export const RATE = 2;\nexport const SPARE = 1;\n\nexport class Meter {}\n\nexport function scale(x: number): number {\n  return x * 2;\n}\n";
    let (_tmp, root) = project(&[
        ("easement.yml", POLICY),
        ("src/lib/rates.ts", rates),
        (
            "src/app/a.ts",
            "import { RATE } from \"../lib/rates\";\nexport const a = RATE;\n",
        ),
        (
            "src/app/b.ts",
            "import { scale } from \"../lib/rates\";\nexport const b = scale(1);\n",
        ),
        (
            "src/app/c.ts",
            "import { Meter } from \"../lib/rates\";\nexport const c = new Meter();\n",
        ),
    ]);
    commit_all(&root);
    fs::write(
        root.join("src/lib/rates.ts"),
        rates.replace("export const SPARE = 1;\n", ""),
    )
    .unwrap();
    let report = impact_diff(&root);
    assert_eq!(
        callers_by_entry(&report),
        [(
            "own_code".to_string(),
            vec!["src/app/a.ts".to_string(), "src/app/c.ts".to_string()]
        )],
        "{report}"
    );
    let gaps = report["evidence_gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(gaps[0]["kind"], "removed_definitions", "{report}");
    assert_eq!(gaps[0]["files"], serde_json::json!(["src/lib/rates.ts"]));
    callers_within_dependents(&report);
}

/// Kotlin's files are read by an adapter, which gives no definition ranges: a
/// changed file is touched whole, and an evidence gap names the language.
#[test]
fn a_file_of_a_language_without_ranges_is_touched_whole() {
    let (_tmp, root) = project(&[
        ("easement.yml", POLICY),
        ("src/lib/Rates.kt", "package lib\nclass Rates\n"),
        (
            "src/app/App.kt",
            "package app\nimport lib.Rates\nclass App\n",
        ),
    ]);
    commit_all(&root);
    fs::write(
        root.join("src/lib/Rates.kt"),
        "package lib\nclass Rates(val rate: Int)\n",
    )
    .unwrap();
    let report = impact_diff(&root);
    assert_eq!(
        report["touched"],
        serde_json::json!([{ "file": "src/lib/Rates.kt", "part": "file", "component": "lib", "callers": [] }]),
        "{report}"
    );
    let gaps = report["evidence_gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(gaps[0]["kind"], "no_definition_ranges", "{report}");
    // Kotlin has no index: no use of the file is named, though `App.kt` imports it.
    assert!(
        gaps[0]["message"]
            .as_str()
            .unwrap()
            .contains("kotlin is read without an index, so no use of them is named"),
        "{report}"
    );
    assert_eq!(dependents(&report), ["src/app/App.kt"], "{report}");
}

/// Without `--diff` no line is known to have changed: `touched` is not said.
#[test]
fn impact_of_named_paths_has_no_touched() {
    let (_tmp, root) = shapes();
    let impacted = commands::impact(
        &config(&root),
        &[root.join("src/lib/shapes.ts")],
        None,
        &mut cache(),
    )
    .unwrap();
    let report = parse(&impacted.json().unwrap());
    assert!(report.get("touched").is_none(), "{report}");
    assert!(!impacted.text().contains("Touched"), "{}", impacted.text());
}

/// The answer agents read, in text and JSON: a definition with its callers, a file's
/// own code, and a deleted function's evidence gap.
#[test]
fn impact_diff_names_the_callers_of_what_it_touched() {
    let rates = "export const RATE = 2;\n\nexport function scale(x: number): number {\n  return x * RATE;\n}\n\nexport const LIMIT = 10;\n\nexport function double(x: number): number {\n  return x * 2;\n}\n";
    let (_tmp, root) = project(&[
        ("easement.yml", POLICY),
        ("src/lib/rates.ts", rates),
        (
            "src/app/a.ts",
            "import { RATE } from \"../lib/rates\";\nexport const a = RATE;\n",
        ),
        (
            "src/app/b.ts",
            "import { scale } from \"../lib/rates\";\nexport const b = scale(1);\n",
        ),
    ]);
    commit_all(&root);
    // git joins a deletion next to an edit into one hunk that adds lines, which
    // leaves no point of deletion: `LIMIT` keeps them apart.
    let changed = rates
        .replace("RATE = 2", "RATE = 3")
        .replace("x * RATE;", "x * RATE * 1;")
        .replace(
            "\nexport function double(x: number): number {\n  return x * 2;\n}\n",
            "",
        );
    fs::write(root.join("src/lib/rates.ts"), changed).unwrap();
    let impacted = commands::impact(&config(&root), &[], Some("main"), &mut cache()).unwrap();
    pinned(|| {
        insta::assert_snapshot!("impact_diff_touched_text", impacted.text());
        insta::assert_json_snapshot!("impact_diff_touched_json", parse(&impacted.json().unwrap()));
    });
}
