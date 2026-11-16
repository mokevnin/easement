//! `suggest`: what the dependency graph says about the policy, which of its
//! suggestions rest on unread files, the patch of `suggest --patch`, and the
//! unused allowances `check --suggest` fails on.

use std::fs;

use easement::commands::{self, CheckOptions, Outcome};

use crate::common::{fixture, git_in, project};
use crate::run::{cache, check, check_with, config, error, json, pinned, text, try_check_with};

/// A policy where `app` may use `lib` and `old`, written as `allowed`, and only
/// imports `lib`.
fn unused_old(allowed: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    project(&[
        (
            "easement.yml",
            &format!(
                "version: 1
components:
  app: {{ in: src/app }}
  lib: {{ in: src/lib }}
  old: {{ in: src/old }}

rules:
  app:
    may_depend_on:
{allowed}"
            ),
        ),
        (
            "src/app/a.ts",
            "import { l } from \"../lib/l\";\nexport const a = l;\n",
        ),
        ("src/lib/l.ts", "export const l = 1;\n"),
        ("src/old/o.ts", "export const o = 1;\n"),
    ])
}

/// `check --suggest` fails on an allowance no import uses, at the line of the
/// policy that writes it, however the item is written; a plain `check` does not.
#[test]
fn check_suggest_fails_on_an_unused_allowance_at_its_line() {
    for allowed in [
        "      - lib\n      - old\n",
        "      - lib # the only one used\n      - { name: old, because: was needed once }\n",
    ] {
        let (_tmp, root) = unused_old(allowed);
        assert_eq!(check(&root).outcome(), Outcome::Passed);
        let checked = check_with(&root, |o| o.suggest = true);
        let report = json(&checked);
        assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
        let violations = report["violations"].as_array().unwrap();
        assert_eq!(violations.len(), 1, "{report}");
        let v = &violations[0];
        assert_eq!(v["rule"], "unused-allowance", "{report}");
        assert_eq!(v["file"], "easement.yml", "{report}");
        assert_eq!(v["line"], 11, "{report}");
        assert_eq!((&v["from"], &v["to"]), (&"app".into(), &"old".into()));
        assert_eq!(
            v["provenance"]["field"], "rules.app.may_depend_on[1]",
            "{report}"
        );
    }
    let (_tmp, root) = unused_old("      - lib\n      - old\n");
    let checked = check_with(&root, |o| o.suggest = true);
    pinned(|| insta::assert_snapshot!("check_suggest_text", text(&checked)));
}

/// `warn` takes `unused-allowance`: the entry is still reported, as a warning.
#[test]
fn check_suggest_warns_on_an_unused_allowance_listed_in_warn() {
    let (_tmp, root) = unused_old("      - lib\n      - old\nwarn: [unused-allowance]\n");
    let checked = check_with(&root, |o| o.suggest = true);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Passed, "{report}");
    assert_eq!(report["violations"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["warnings"][0]["rule"], "unused-allowance",
        "{report}"
    );
}

/// The baseline never accepts an unused allowance: it is about the policy.
#[test]
fn the_baseline_never_accepts_an_unused_allowance() {
    let (_tmp, root) = unused_old("      - lib\n      - old\n");
    commands::baseline(&config(&root), true, &mut cache()).unwrap();
    let checked = check_with(&root, |o| o.suggest = true);
    assert_eq!(checked.outcome(), Outcome::Failed, "{}", text(&checked));
}

/// Unused allowances are a property of the whole project: a narrowed run would
/// give half the answer, so `--suggest` refuses `--diff`, `--scope` and `--component`.
#[test]
fn check_suggest_refuses_a_narrowed_run() {
    let (_tmp, root) = unused_old("      - lib\n      - old\n");
    let narrowings: [&dyn Fn(&mut CheckOptions); 3] = [
        &|o| o.diff = Some("HEAD".into()),
        &|o| o.paths = vec!["src/app".into()],
        &|o| o.components = vec!["app".into()],
    ];
    for narrow in narrowings {
        let said = error(try_check_with(&root, |o| {
            o.suggest = true;
            narrow(o);
        }));
        assert!(
            said.contains("`--suggest` judges the whole project"),
            "{said}"
        );
    }
}

/// What `suggest --patch` would not remove does not fail: an allowance that comes
/// through a glob rule key, which another component it covers uses.
#[test]
fn check_suggest_passes_an_allowance_that_comes_through_a_glob() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1
components:
  api-a: { in: src/api-a }
  api-b: { in: src/api-b }
  db: { in: src/db }
rules:
  'api-*': { may_depend_on: [db] }
",
        ),
        (
            "src/api-a/a.ts",
            "import { d } from \"../db/d\";\nexport const a = d;\n",
        ),
        ("src/api-b/b.ts", "export const b = 1;\n"),
        ("src/db/d.ts", "export const d = 1;\n"),
    ]);
    let found = commands::suggest(&config(&root), &mut cache()).unwrap();
    assert!(found.text().contains("api-b -> db"), "{}", found.text());
    let checked = check_with(&root, |o| o.suggest = true);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
}

/// An entry next to files no indexer read does not fail: whether it is used is
/// not known, and the report lists it as such.
#[test]
fn check_suggest_lists_an_allowance_next_to_unread_files_as_unreliable() {
    let (_tmp, root) = project(&[
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\n  log: { in: log }\nrules:\n  app: { may_depend_on: [log] }\nuncovered_ok:\n  - { name: app/race.go, because: built with -race only }\n",
        ),
        ("app/main.go", "package app\n\nfunc Run() {}\n"),
        (
            "app/race.go",
            "//go:build race\n\npackage app\n\nimport \"example.com/m/log\"\n\nfunc R() { log.P() }\n",
        ),
        ("log/log.go", "package log\n\nfunc P() {}\n"),
    ]);
    let checked = check_with(&root, |o| o.suggest = true);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Passed, "{report}");
    assert_eq!(
        report["unreliable_allowances"],
        serde_json::json!([{ "from": "app", "to": "log", "unread": { "app": 1 } }]),
        "{report}"
    );
    let said = text(&checked);
    assert!(
        said.contains("app -> log (not indexed: 1 file(s) of app)"),
        "{said}"
    );
    // `uncovered_ok` accepts the file, so `--require-evidence` still passes.
    let required = check_with(&root, |o| {
        o.suggest = true;
        o.require_evidence = true;
    });
    assert_eq!(required.outcome(), Outcome::Passed, "{}", text(&required));
}

/// `suggest --format json` gives each suggestion as data, and an item next to
/// files no indexer read says how many: what an agent must not act on blindly.
#[test]
fn suggest_says_in_json_which_suggestions_rest_on_unread_files() {
    let (_tmp, root) = project(&[
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\n  lib: { in: lib }\n  log: { in: log }\nrules:\n  app: { may_depend_on: [lib, log] }\n",
        ),
        ("app/main.go", "package app\n\nfunc Run() {}\n"),
        // Built only with the `race` tag: no indexer reads it, and it uses lib.
        (
            "app/race.go",
            "//go:build race\n\npackage app\n\nimport \"example.com/m/lib\"\n\nfunc R() { lib.L() }\n",
        ),
        ("lib/lib.go", "package lib\n\nfunc L() {}\n"),
        ("log/log.go", "package log\n\nfunc P() {}\n"),
    ]);
    let found = commands::suggest(&config(&root), &mut cache()).unwrap();
    let report = serde_json::to_value(&found).unwrap();
    assert_eq!(
        report["unreliable"],
        serde_json::json!([
            { "from": "app", "to": "lib", "unread": { "app": 1 } },
            { "from": "app", "to": "log", "unread": { "app": 1 } },
        ]),
        "{report}"
    );
    assert_eq!(report["removable"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["evidence_gaps"][0]["files"],
        serde_json::json!(["app/race.go"])
    );
}

#[test]
fn suggest_reports_what_the_graph_says_about_the_config() {
    let (_tmp, root) = fixture("ts");
    let found = commands::suggest(&config(&root), &mut cache()).unwrap();
    let said = found.text();
    assert!(said.contains("Depended on but not allowed"), "{said}");
    assert!(said.contains("app: infra (1 dependency(ies))"), "{said}");
}

/// The patch names the config by its path in the project, however the config
/// was given (the binary passes `--config` as written, here it is absolute),
/// so `git apply` in the root of the project applies it.
#[test]
fn suggest_patch_drops_unused_allowances_and_keeps_the_rest() {
    let policy = "version: 1
# who may use whom
components:
  app: { in: src/app }
  lib: { in: src/lib }
  old: { in: src/old }

rules:
  app:
    may_depend_on:
      - lib # the only one used
      - { name: old, because: was needed once }
";
    let (_tmp, root) = project(&[
        ("easement.yml", policy),
        (
            "src/app/a.ts",
            "import { l } from \"../lib/l\";\nexport const a = l;\n",
        ),
        ("src/lib/l.ts", "export const l = 1;\n"),
        ("src/old/o.ts", "export const o = 1;\n"),
    ]);
    let config = config(&root);
    let diff = commands::suggest_patch(&config, &mut cache()).unwrap().diff;
    assert!(
        diff.contains("--- a/easement.yml\n+++ b/easement.yml\n"),
        "{diff}"
    );
    assert!(
        diff.contains("-      - { name: old, because: was needed once }"),
        "{diff}"
    );
    assert!(!diff.contains("-      - lib"), "{diff}");

    fs::write(root.join("fix.patch"), &diff).unwrap();
    let applied = git_in(&root).args(["apply", "fix.patch"]).status().unwrap();
    assert!(applied.success());
    let fixed = fs::read_to_string(&config).unwrap();
    assert!(fixed.contains("# who may use whom") && fixed.contains("- lib # the only one used"));
    assert!(!fixed.contains("old,"), "{fixed}");
    let found = commands::suggest(&config, &mut cache()).unwrap();
    assert!(found.suggestions.removable.is_empty());
    assert!(!found.text().contains("never imported"), "{}", found.text());
    // Nothing left to remove: an empty patch.
    assert_eq!(
        commands::suggest_patch(&config, &mut cache()).unwrap().diff,
        ""
    );
}

/// An unused allowance a file named in `extends` writes fails too, at line 1 of
/// the config, as every violation of a clause another input writes is reported,
/// with that input in its `provenance`: `suggest --patch` cannot edit it there.
#[test]
fn check_suggest_names_the_input_that_writes_an_unused_allowance() {
    let (_tmp, root) = project(&[
        ("easement.yml", "extends: base.yml\n"),
        (
            "base.yml",
            "version: 1
components:
  app: { in: src/app }
  lib: { in: src/lib }
rules:
  app: { may_depend_on: [lib] }
",
        ),
        ("src/app/a.ts", "export const a = 1;\n"),
        ("src/lib/l.ts", "export const l = 1;\n"),
    ]);
    let checked = check_with(&root, |o| o.suggest = true);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    let v = &report["violations"][0];
    assert_eq!(
        (&v["file"], &v["line"]),
        (&"easement.yml".into(), &1.into())
    );
    assert_eq!(v["provenance"]["input"], "base.yml", "{report}");
}
