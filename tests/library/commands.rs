//! The commands that start or explain a policy: `init`, `explain`, `graph`,
//! `schema`, and what makes `check --watch` check again.

use std::fs;

use easement::GraphFormat;
use easement::commands::{self, Outcome};

use crate::common::{fixture, project};
use crate::run::{cache, check, config, error, text};

/// The allowances `init` writes come from the graph, so what no indexer read
/// they leave out: `init` says so, with the files.
#[test]
fn init_names_the_files_its_allowances_leave_out() {
    let (_tmp, root) = fixture("uncovered");
    let said = commands::init(&root, true, &mut cache()).unwrap();
    assert!(
        said.contains("Evidence gaps (the allowances leave out what this code depends on):"),
        "{said}"
    );
    assert!(said.contains("    tools/gen.go\n"), "{said}");
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
    let said = commands::init(&root, false, &mut cache()).unwrap();
    assert!(
        said.contains("3 components and 2 allowed dependencies"),
        "{said}"
    );
    let written = fs::read_to_string(config(&root)).unwrap();
    assert!(written.contains("  web: { in: src/web }"), "{written}");
    assert!(
        written.contains("  web: { may_depend_on: [core] }"),
        "{written}"
    );
    assert!(
        written.contains("  core: { may_depend_on: [data] }"),
        "{written}"
    );
    assert!(!written.contains("tests"), "{written}");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(text(&checked).contains("passed"), "{}", text(&checked));
    // Turning the generated allowance around is a violation.
    fs::write(
        root.join("src/data/q.ts"),
        "import { run } from '../core/run';\nexport const q = run;\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("data -> core is not allowed"),
        "{}",
        text(&checked)
    );
    let refused = error(commands::init(&root, false, &mut cache()));
    assert!(refused.contains("already exists"), "{refused}");
    commands::init(&root, true, &mut cache()).unwrap();
}

#[test]
fn explain_and_graph() {
    let (_tmp, root) = fixture("policy");
    let config = config(&root);

    let explained = commands::explain(&config, &root.join("src/web/page.ts"))
        .unwrap()
        .text()
        .unwrap();
    assert!(
        explained.contains("component: web (matches `src/web`)"),
        "{explained}"
    );
    assert!(explained.contains("may depend on: core (layers), shared (common)"));
    assert!(explained.contains("forbidden: data"));
    let stray =
        error(commands::explain(&config, &root.join("src/stray.ts")).and_then(|e| e.text()));
    assert!(stray.contains("belongs to no component"), "{stray}");

    let mermaid = commands::graph(&config, GraphFormat::Mermaid, None, &mut cache())
        .unwrap()
        .diagram;
    assert!(mermaid.starts_with("graph LR\n"), "{mermaid}");
    assert!(
        mermaid.contains("-.->"),
        "the forbidden web -> data arrow is dashed: {mermaid}"
    );
    assert!(mermaid.contains("linkStyle"));
    let dot = commands::graph(&config, GraphFormat::Dot, None, &mut cache())
        .unwrap()
        .diagram;
    assert!(dot.starts_with("digraph easement {"), "{dot}");
    assert!(dot.contains("color=red"));
}

/// The keys serde reports as the valid ones, read from its error for an unknown key.
fn config_keys(yaml: &str) -> Vec<String> {
    let (_tmp, root) = project(&[("easement.yml", yaml)]);
    let refused = error(crate::run::try_check_with(&root, |_| {}));
    let list = refused
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
    let schema = crate::run::parse(&easement::schema_text());
    assert_eq!(schema["type"], "object");
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
        keys(&schema["definitions"]["Component"]),
        config_keys("version: 1\ncomponents:\n  a: { in: a, bogus: 1 }\n")
    );
    assert_eq!(
        keys(&schema["definitions"]["Rule"]),
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
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(report.contains("ui -> db is not allowed"), "{report}");
    assert!(
        report.contains("because: the UI never touches the database"),
        "{report}"
    );
}

/// `check --watch` checks again when the fingerprint of the project changes:
/// after an edit of the code, and after one of a policy it extends from
/// outside the root.
#[test]
fn the_fingerprint_changes_with_the_code_and_with_an_extended_policy_outside_the_root() {
    let (tmp, _) = project(&[
        (
            "shared/base.yml",
            "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b }\nrules:\n  a: { may_depend_on: [b] }\n",
        ),
        ("app/easement.yml", "extends: ../shared/base.yml\n"),
        ("app/src/a/x.ts", "export {};\n"),
        ("app/src/b/y.ts", "export {};\n"),
    ]);
    let app = tmp.path().join("app");
    let policy = config(&app);
    let first = commands::fingerprint(&policy).unwrap();
    assert_eq!(commands::fingerprint(&policy), Some(first), "it is stable");

    fs::write(app.join("src/a/x.ts"), "import '../b/y';\n").unwrap();
    let edited = commands::fingerprint(&policy).unwrap();
    assert_ne!(edited, first, "an edit of the code changes it");

    fs::write(
        tmp.path().join("shared/base.yml"),
        "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b }\nrules:\n  a: { because: a stands alone }\n",
    )
    .unwrap();
    assert_ne!(
        commands::fingerprint(&policy),
        Some(edited),
        "an edit of the extended policy changes it"
    );
}
