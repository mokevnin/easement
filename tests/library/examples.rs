//! The examples of a policy: the decisions it pins that no code exercises,
//! which `check` fails on once the policy stops meaning them, and which
//! `review` lists when a change adds, removes or breaks one.

use std::fs;
use std::path::PathBuf;

use easement::commands::{self, Outcome};
use tempfile::TempDir;

use crate::common::{commit_all, project};
use crate::run::{cache, check, check_with, config, failed_json, parse, text};

/// A project of three components where no code imports another: only the
/// policy examples say what the policy means for `web`.
fn examples_project(policy: &str) -> (TempDir, PathBuf) {
    project(&[
        ("easement.yml", policy),
        ("src/web/Page.kt", "package web\n"),
        ("src/storage/Db.kt", "package storage\n"),
        ("src/billing/api/Charge.kt", "package billing.api\n"),
        (
            "src/billing/internal/Ledger.kt",
            "package billing.internal\n",
        ),
    ])
}

const EXAMPLES_POLICY: &str = "version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
  billing: { in: src/billing, public: src/billing/api }
common: [storage]
rules:
  web: { may_depend_on: [billing], must_not_depend_on: [storage] }
examples:
  - { from: src/web/Page.kt, to: src/storage/Db.kt, expect: rejected, because: web reads through the api }
  - { from: src/web/Page.kt, to: src/billing/internal/Ledger.kt, expect: rejected, rule: privacy }
  - { from: src/web/Page.kt, to: src/billing/api/Charge.kt, expect: allowed }
";

/// The examples of the policy pin decisions no code exercises: deleting the
/// prohibition changes no source line, yet `check` fails at the example's line.
#[test]
fn a_policy_example_fails_when_the_policy_stops_meaning_it() {
    let (_tmp, root) = examples_project(EXAMPLES_POLICY);
    assert_eq!(check(&root).outcome(), Outcome::Passed);

    let granted = EXAMPLES_POLICY.replace(", must_not_depend_on: [storage]", "");
    fs::write(root.join("easement.yml"), &granted).unwrap();
    let report = failed_json(&root);
    let found = report["violations"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{report}");
    let v = &found[0];
    assert_eq!(v["rule"], "example");
    assert_eq!(v["file"], "easement.yml");
    assert_eq!(v["line"], 10);
    assert_eq!(v["because"], "web reads through the api");
    let message = v["message"].as_str().unwrap();
    assert!(message.contains("expects rejected"), "{message}");
    assert!(message.contains("allowed by common"), "{message}");
}

/// Widening a public surface to the whole component fails the example that
/// pins `privacy`, while the one with no rule still holds.
#[test]
fn a_policy_example_pins_the_rule_that_rejects() {
    let widened = EXAMPLES_POLICY.replace("public: src/billing/api", "public: src/billing");
    let (_tmp, root) = examples_project(&widened);
    let report = failed_json(&root);
    let found = report["violations"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{report}");
    assert_eq!(found[0]["rule"], "example");
    assert_eq!(found[0]["line"], 11);
    let message = found[0]["message"].as_str().unwrap();
    assert!(message.contains("expects rejected [privacy]"), "{message}");
    assert!(message.contains("allowed by may_depend_on"), "{message}");

    // Without `rule`, a rejection for another reason still satisfies it.
    let other = EXAMPLES_POLICY
        .replace(", rule: privacy", "")
        .replace("may_depend_on: [billing], ", "");
    fs::write(root.join("easement.yml"), other).unwrap();
    let report = failed_json(&root);
    let found = report["violations"].as_array().unwrap();
    // Only the `allowed` example fails: billing is no longer granted.
    assert_eq!(found.len(), 1, "{report}");
    assert_eq!(found[0]["line"], 12);
    assert!(
        found[0]["message"]
            .as_str()
            .unwrap()
            .contains("expects allowed, but the policy now decides rejected [dependency]"),
        "{report}"
    );
}

/// An example that no component owns says nothing of the policy, so it fails
/// rather than passing: a source or a target path that belongs to no component,
/// one that `exclude` hides among them.
#[test]
fn a_policy_example_no_component_owns_cannot_be_judged() {
    let policy = format!(
        "{EXAMPLES_POLICY}  - {{ from: src/gone/Old.kt, to: src/storage/Db.kt, expect: rejected }}\n  - {{ from: src/web/Page.kt, to: src/gone/Old.kt, expect: allowed }}\n  - {{ from: src/web/Page.kt, to: src/gen/Api.kt, expect: rejected }}\nexclude: [src/gen]\n"
    );
    let (_tmp, root) = examples_project(&policy);
    let report = failed_json(&root);
    let found: Vec<(u64, &str)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["line"].as_u64().unwrap(), v["message"].as_str().unwrap()))
        .collect();
    assert_eq!(found.len(), 3, "{report}");
    assert_eq!(found[0].0, 13);
    assert!(
        found[0]
            .1
            .contains("cannot be judged: src/gone/Old.kt belongs to no component"),
        "{report}"
    );
    assert_eq!(found[1].0, 14);
    assert!(found[1].1.contains("cannot be judged"), "{report}");
    assert_eq!(found[2].0, 15);
    assert!(
        found[2]
            .1
            .contains("cannot be judged: `exclude` hides src/gen/Api.kt"),
        "{report}"
    );
}

/// A shared base policy ships its examples: `extends` joins them, and a
/// failing one is reported at its line in the file that writes it.
#[test]
fn a_policy_example_comes_through_extends() {
    let (_tmp, root) = examples_project(
        "extends: base.yml
examples:
  - { from: src/web/Page.kt, to: src/billing/api/Charge.kt, expect: rejected }
",
    );
    fs::write(root.join("base.yml"), EXAMPLES_POLICY).unwrap();
    let report = failed_json(&root);
    let found = report["violations"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{report}");
    assert_eq!(found[0]["file"], "easement.yml");
    assert_eq!(found[0]["line"], 3);

    let granted = EXAMPLES_POLICY.replace(", must_not_depend_on: [storage]", "");
    fs::write(root.join("base.yml"), granted).unwrap();
    let report = failed_json(&root);
    let mut found: Vec<(String, u64)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("base.yml".to_string(), 10),
            ("easement.yml".to_string(), 3)
        ]
    );
}

/// A failing example is about the policy: `--diff` and `--scope` keep it,
/// and the baseline never accepts it.
#[test]
fn a_policy_example_survives_diff_scope_and_the_baseline() {
    let granted = EXAMPLES_POLICY.replace(", must_not_depend_on: [storage]", "");
    let (_tmp, root) = examples_project(&granted);
    commit_all(&root);
    fs::write(root.join("src/web/Other.kt"), "package web\n").unwrap();
    let failing = "easement.yml:10: example src/web/Page.kt -> src/storage/Db.kt";
    let checked = check_with(&root, |o| o.diff = Some("main".into()));
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(text(&checked).contains(failing), "{}", text(&checked));
    let checked = check_with(&root, |o| o.components = vec!["billing".into()]);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(text(&checked).contains(failing), "{}", text(&checked));
    let recorded = commands::baseline(&config(&root), false, &mut cache()).unwrap();
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded
            .text()
            .contains("1 violation(s) of the config left out"),
        "{}",
        recorded.text()
    );
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(text(&checked).contains(failing), "{}", text(&checked));
}

/// A change to the policy that breaks an example shows in the review as an
/// `example` violation the change introduces.
#[test]
fn review_reports_a_policy_example_the_change_breaks() {
    let (_tmp, root) = examples_project(EXAMPLES_POLICY);
    commit_all(&root);
    let granted = EXAMPLES_POLICY.replace(", must_not_depend_on: [storage]", "");
    fs::write(root.join("easement.yml"), granted).unwrap();
    let said = commands::review(&config(&root), "main", &mut cache())
        .unwrap()
        .text();
    assert!(said.contains("introduced (1):"), "{said}");
    assert!(
        said.contains(
            "easement.yml:10: example src/web/Page.kt -> src/storage/Db.kt: expects rejected, but the policy now decides allowed by common (web -> storage)",
        ),
        "{said}"
    );
}

/// Dropping an example weakens the policy without breaking anything, so the
/// review lists the examples a change adds, removes or changes.
#[test]
fn review_lists_the_policy_examples_a_change_adds_removes_or_changes() {
    let (_tmp, root) = examples_project(EXAMPLES_POLICY);
    commit_all(&root);
    let head = EXAMPLES_POLICY
        .replace(
            "  - { from: src/web/Page.kt, to: src/storage/Db.kt, expect: rejected, because: web reads through the api }\n",
            "",
        )
        .replace(
            "to: src/billing/internal/Ledger.kt, expect: rejected, rule: privacy }",
            "to: src/billing/internal/Ledger.kt, expect: rejected }",
        )
        .replace(
            "expect: allowed }\n",
            "expect: allowed }\n  - { from: src/storage/Db.kt, external: lodash, expect: allowed }\n",
        );
    fs::write(root.join("easement.yml"), head).unwrap();

    let review = commands::review(&config(&root), "main", &mut cache()).unwrap();
    let text = review.text();
    for line in [
        "Policy examples:",
        "  removed (1), so the policy no longer pins their decision:",
        "    src/web/Page.kt -> src/storage/Db.kt: expects rejected",
        "      because: web reads through the api",
        "  added (1):",
        "    src/storage/Db.kt -> external lodash: expects allowed",
        "  changed (1):",
        "    src/web/Page.kt -> src/billing/internal/Ledger.kt: expects rejected (was rejected [privacy])",
    ] {
        assert!(text.contains(&format!("{line}\n")), "{line:?} in\n{text}");
    }

    let report = parse(&review.json());
    assert_eq!(
        report["examples"],
        serde_json::json!({
            "added": [{
                "from": "src/storage/Db.kt",
                "to": null,
                "external": "lodash",
                "expect": "allowed",
                "rule": null,
                "because": null,
            }],
            "removed": [{
                "from": "src/web/Page.kt",
                "to": "src/storage/Db.kt",
                "external": null,
                "expect": "rejected",
                "rule": null,
                "because": "web reads through the api",
            }],
            "changed": [{
                "before": {
                    "from": "src/web/Page.kt",
                    "to": "src/billing/internal/Ledger.kt",
                    "external": null,
                    "expect": "rejected",
                    "rule": "privacy",
                    "because": null,
                },
                "after": {
                    "from": "src/web/Page.kt",
                    "to": "src/billing/internal/Ledger.kt",
                    "external": null,
                    "expect": "rejected",
                    "rule": null,
                    "because": null,
                },
            }],
        }),
        "{report}"
    );
}

/// A Go project where no file uses a name of another: only the symbol examples
/// say who may use `ent.Client`.
fn symbol_examples_project(policy: &str) -> (TempDir, PathBuf) {
    project(&[
        ("easement.yml", policy),
        ("go.mod", "module example.com/app\n\ngo 1.25\n"),
        ("ent/client.go", "package ent\n\ntype Client struct{}\n"),
        ("internal/api/site/access.go", "package site\n"),
        ("internal/app/entclient/client.go", "package entclient\n"),
        ("internal/web/page.go", "package web\n"),
        (
            "internal/billing/charge.go",
            "package billing\n\ntype Charge struct{}\n\ntype Invoice struct{}\n",
        ),
    ])
}

const SYMBOL_EXAMPLES_POLICY: &str = "version: 1
components:
  ent: { in: ent }
  api: { in: internal/api }
  wiring: { in: internal/app }
  web: { in: internal/web }
  billing: { in: internal/billing, public_symbols: [Invoice] }
forbid_symbol:
  - name: example.com/app/ent.Client
    allow: [wiring]
rules:
  web: { forbid_symbol: [ent.Tx] }
examples:
  - { from: internal/api/site/access.go, symbol: example.com/app/ent.Client, expect: rejected, because: a handler takes a scoped client }
  - { from: internal/app/entclient/client.go, symbol: example.com/app/ent.Client, expect: allowed }
";

/// The violations of a failed check as their line and message.
fn example_failures(root: &std::path::Path) -> Vec<(u64, String)> {
    let report = failed_json(root);
    report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .inspect(|v| assert_eq!(v["rule"], "example", "{report}"))
        .map(|v| {
            (
                v["line"].as_u64().unwrap(),
                v["message"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// `forbid_symbol` with `allow` is judged from the policy alone: flipping the
/// grant fails both examples at their lines although no code uses the name.
#[test]
fn a_symbol_example_pins_who_may_use_a_name() {
    let (_tmp, root) = symbol_examples_project(SYMBOL_EXAMPLES_POLICY);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));

    let flipped = SYMBOL_EXAMPLES_POLICY.replace("allow: [wiring]", "allow: [api]");
    fs::write(root.join("easement.yml"), flipped).unwrap();
    let report = failed_json(&root);
    let found = report["violations"].as_array().unwrap();
    assert_eq!(found.len(), 2, "{report}");
    assert_eq!(found[0]["rule"], "example");
    assert_eq!(found[0]["line"], 14);
    assert_eq!(found[0]["because"], "a handler takes a scoped client");
    assert_eq!(
        found[0]["message"],
        "example internal/api/site/access.go uses example.com/app/ent.Client: expects rejected, but the policy now decides allowed by forbid_symbol allow (api); restore what the policy meant, or update the example if the change is meant"
    );
    assert_eq!(found[1]["line"], 15);
    assert_eq!(
        found[1]["message"],
        "example internal/app/entclient/client.go uses example.com/app/ent.Client: expects allowed, but the policy now decides rejected [symbol]: wiring must not use example.com/app/ent.Client (only api may); restore what the policy meant, or update the example if the change is meant"
    );
}

/// The examples of the uses of names that the rest of the symbol rules decide:
/// the own `forbid_symbol` of the user's component, on a name nothing defines,
/// a member through its banned type, and the `public_symbols` of the component
/// `defined_in` names, which without it is not consulted.
#[test]
fn a_symbol_example_is_judged_by_every_symbol_rule() {
    let policy = format!(
        "{SYMBOL_EXAMPLES_POLICY}  - {{ from: internal/web/page.go, symbol: example.com/app/ent.Tx, expect: rejected, rule: symbol }}
  - {{ from: internal/web/page.go, symbol: example.com/app/billing.Charge, defined_in: internal/billing/charge.go, expect: rejected, rule: symbol }}
  - {{ from: internal/web/page.go, symbol: example.com/app/billing.Charge, expect: allowed }}
  - {{ from: internal/web/page.go, symbol: example.com/app/billing.Invoice.Total, defined_in: internal/billing/charge.go, expect: allowed }}
  - {{ from: internal/api/site/access.go, symbol: example.com/app/ent.Client.Query, expect: rejected }}
"
    );
    let (_tmp, root) = symbol_examples_project(&policy);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));

    let loosened = policy
        .replace("  web: { forbid_symbol: [ent.Tx] }\n", "  web: {}\n")
        .replace(", public_symbols: [Invoice]", "")
        .replace("allow: [wiring]", "allow: [wiring, api]");
    fs::write(root.join("easement.yml"), loosened).unwrap();
    let failed = example_failures(&root);
    let lines: Vec<u64> = failed.iter().map(|(line, _)| *line).collect();
    assert_eq!(lines, [14, 16, 17, 20], "{failed:?}");
    assert!(
        failed[1].1.contains(
            "example internal/web/page.go uses example.com/app/ent.Tx: expects rejected [symbol], but the policy now decides allowed (web): no rule restricts it"
        ),
        "{failed:?}"
    );
    assert!(
        failed[2].1.contains(
            "example internal/web/page.go uses example.com/app/billing.Charge defined in internal/billing/charge.go: expects rejected [symbol], but the policy now decides allowed (web -> billing): no rule restricts it"
        ),
        "{failed:?}"
    );
    assert!(
        failed[3].1.contains(
            "example internal/api/site/access.go uses example.com/app/ent.Client.Query: expects rejected, but the policy now decides allowed by forbid_symbol allow (api)"
        ),
        "{failed:?}"
    );
}

/// A use of a name in a path that no component owns, or that `exclude` hides,
/// says nothing of the policy, so its example fails rather than passing; so does
/// a `defined_in` no component owns. A source no component owns is judged by the
/// top-level `forbid_symbol` alone, as the scan judges it.
#[test]
fn a_symbol_example_no_component_owns_cannot_be_judged() {
    let policy = format!(
        "{SYMBOL_EXAMPLES_POLICY}  - {{ from: internal/gone/x.go, symbol: example.com/app/billing.Charge, expect: allowed }}
  - {{ from: internal/gen/x.go, symbol: example.com/app/ent.Client, expect: rejected }}
  - {{ from: internal/web/page.go, symbol: example.com/app/billing.Charge, defined_in: internal/gone/x.go, expect: allowed }}
  - {{ from: internal/gone/x.go, symbol: example.com/app/ent.Client, expect: rejected, rule: symbol }}
  - {{ from: internal/pb/x.go, symbol: example.com/app/billing.Charge, expect: allowed }}
  - {{ from: internal/web/page.go, symbol: example.com/app/billing.Charge, defined_in: internal/gen/y.go, expect: allowed }}
exclude: [internal/gen]
target_only: [internal/pb]
"
    );
    let (_tmp, root) = symbol_examples_project(&policy);
    let failed = example_failures(&root);
    assert_eq!(
        failed,
        [
            (16, "example internal/gone/x.go uses example.com/app/billing.Charge: cannot be judged: internal/gone/x.go belongs to no component; point it at a path a component owns, or drop it".to_string()),
            (17, "example internal/gen/x.go uses example.com/app/ent.Client: cannot be judged: `exclude` hides internal/gen/x.go; point it at a path a component owns, or drop it".to_string()),
            (18, "example internal/web/page.go uses example.com/app/billing.Charge defined in internal/gone/x.go: cannot be judged: internal/gone/x.go belongs to no component; point it at a path a component owns, or drop it".to_string()),
            (20, "example internal/pb/x.go uses example.com/app/billing.Charge: cannot be judged: `target_only` names internal/pb/x.go; point it at a path a component owns, or drop it".to_string()),
            (21, "example internal/web/page.go uses example.com/app/billing.Charge defined in internal/gen/y.go: cannot be judged: `exclude` hides internal/gen/y.go; point it at a path a component owns, or drop it".to_string()),
        ]
    );
}

/// An `allow_in` grant reaches a use of a name by its path: dropping it fails
/// the example that pins the use as allowed.
#[test]
fn a_symbol_example_is_granted_by_a_path() {
    let policy = SYMBOL_EXAMPLES_POLICY
        .replace(
            "allow: [wiring]\n",
            "allow: [wiring]\n    allow_in: [internal/web/admin]\n",
        )
        .replace(
            "examples:\n",
            "examples:\n  - { from: internal/web/admin/raw.go, symbol: example.com/app/ent.Client, expect: allowed }\n",
        );
    let (_tmp, root) = symbol_examples_project(&policy);
    // An `allow_in` pattern that matches no file fails as `unmatched`.
    fs::create_dir_all(root.join("internal/web/admin")).unwrap();
    fs::write(root.join("internal/web/admin/raw.go"), "package admin\n").unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));

    let dropped = policy.replace("    allow_in: [internal/web/admin]\n", "");
    fs::write(root.join("easement.yml"), dropped).unwrap();
    assert_eq!(
        example_failures(&root),
        [(
            14,
            "example internal/web/admin/raw.go uses example.com/app/ent.Client: expects allowed, but the policy now decides rejected [symbol]: web must not use example.com/app/ent.Client (only wiring may); restore what the policy meant, or update the example if the change is meant".to_string()
        )]
    );
}
