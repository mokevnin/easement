//! The surfaces a component opens: its public API and the named surfaces open
//! to their importers only, and the cycles, reasons and suppressions of the
//! rules that judge them, as `check`, `why` and `explain` tell them.

use std::path::{Path, PathBuf};

use easement::commands::{self, Outcome};
use tempfile::TempDir;

use crate::common::{fixture, project};
use crate::run::{cache, check, config, json, parse, pinned, text};

#[test]
fn public_api_cycles_reasons_and_suppressions() {
    let (_tmp, root) = fixture("rules");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let found: Vec<(String, u64, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["rule"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected = [
        ("src/billing/internal/charge.ts", 1, "cycle"),
        ("src/orders/service.ts", 2, "privacy"),
        ("src/ui/page.ts", 4, "dependency"),
        ("src/ui/page.ts", 4, "suppression-reason"),
        ("src/ui/page.ts", 5, "unused-suppression"),
    ];
    assert_eq!(
        found,
        expected
            .iter()
            .map(|(f, l, r)| (f.to_string(), *l, r.to_string()))
            .collect::<Vec<_>>()
    );
    // Line 3 of the page depends on `billing` twice, both excused by the directive
    // above it: on the index it imports, and on the file that defines the `charge`
    // the index re-exports.
    let suppressions = report["suppressions"].as_array().unwrap();
    assert_eq!(suppressions.len(), 1, "{report}");
    assert_eq!(
        (&suppressions[0]["file"], &suppressions[0]["line"]),
        (&"src/ui/page.ts".into(), &2.into()),
        "{report}"
    );
    let excused: Vec<&str> = suppressions[0]["excused"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["target"].as_str().unwrap())
        .collect();
    assert_eq!(
        excused,
        ["src/billing/index.ts", "src/billing/internal/charge.ts"],
        "{report}"
    );
    assert!(report["stats"].get("suppressed").is_none(), "{report}");
    let violations = report["violations"].as_array().unwrap();
    assert_eq!(
        violations[0]["message"],
        "component dependency cycle: billing -> orders -> billing"
    );
    assert_eq!(
        violations[1]["because"],
        "billing internals change without notice"
    );
    assert_eq!(violations[2]["because"], "the UI talks to orders only");

    // Text output carries the rule id and the reason.
    let said = text(&checked);
    assert!(
        said.contains(
            "src/orders/service.ts:2: orders -> billing: src/billing/internal/charge.ts is not in the public surface of billing",
        ),
        "{said}"
    );
    assert!(
        said.contains("  because: billing internals change without notice"),
        "{said}"
    );

    // Cycles and the other rules are baselined like dependency violations.
    let recorded = commands::baseline(&config(&root), false, &mut cache()).unwrap();
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("5 violation(s)"),
        "{}",
        recorded.text()
    );
    assert_eq!(check(&root).outcome(), Outcome::Passed);
}

/// Billing opens its API to every consumer and its admin code to jobs alone;
/// billing imports checkout, which closes a cycle through the admin code.
const SURFACES: &str = "version: 1
components:
  checkout: { in: src/checkout }
  jobs: { in: src/jobs }
  billing:
    in: src/billing
    public: src/billing/api
    surfaces:
      admin: { in: src/billing/admin, importers: [jobs], because: maintenance only }
no_cycles: true
rules:
  checkout: { may_depend_on: [billing] }
  jobs: { may_depend_on: [billing] }
  billing: { may_depend_on: [checkout], max_fan_out: 0 }
";

fn surfaces_project(policy: &str) -> (TempDir, PathBuf) {
    project(&[
        ("easement.yml", policy),
        (
            "src/checkout/cart.ts",
            "import { refund } from \"../billing/admin/refund\";\nexport const cart = refund;\n",
        ),
        (
            "src/jobs/nightly.ts",
            "import { refund } from \"../billing/admin/refund\";\nexport const nightly = refund;\n",
        ),
        ("src/billing/admin/refund.ts", "export const refund = 1;\n"),
        (
            "src/billing/api/charge.ts",
            "import { cart } from \"../../checkout/cart\";\nexport const charge = cart;\n",
        ),
    ])
}

/// What `why` says of the dependency of `from` on `to` in the project at `root`.
fn why(root: &Path, from: &str, to: &str) -> String {
    commands::why(&config(root), from, to, &mut cache())
        .unwrap()
        .text()
}

#[test]
fn a_named_surface_is_open_only_to_its_importers() {
    let (_tmp, root) = surfaces_project(SURFACES);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let found = &report["violations"];
    let privacy: Vec<&serde_json::Value> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["rule"] == "privacy")
        .collect();
    assert_eq!(privacy.len(), 1, "{found:?}");
    assert_eq!(privacy[0]["file"], "src/checkout/cart.ts");
    assert_eq!(
        privacy[0]["message"],
        "checkout -> billing: src/billing/admin/refund.ts is billing's admin surface, open to jobs (written as src/billing/admin/refund.ts)"
    );
    assert_eq!(privacy[0]["because"], "maintenance only");
    // The run above indexed the project, so this one reads the cache.
    let cached = check(&root);
    assert_eq!(cached.outcome(), Outcome::Failed);
    pinned(|| {
        insta::assert_snapshot!("check_surfaces_text", text(&cached));
    });

    // Both importers still depend on billing as one component: its cycle and
    // its metrics are those of the same code with no named surface.
    let (_plain_tmp, plain) = surfaces_project(&SURFACES.replace(
        "    surfaces:\n      admin: { in: src/billing/admin, importers: [jobs], because: maintenance only }\n",
        "",
    ));
    let not_privacy = |root: &Path| -> Vec<serde_json::Value> {
        let checked = check(root);
        assert_eq!(checked.outcome(), Outcome::Failed);
        json(&checked)["violations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["rule"] != "privacy")
            .cloned()
            .collect()
    };
    let structural = not_privacy(&root);
    assert!(
        structural.iter().any(|v| v["rule"] == "cycle"),
        "{structural:?}"
    );
    assert!(
        structural.iter().any(|v| v["target"] == "max_fan_out"),
        "{structural:?}"
    );
    assert_eq!(structural, not_privacy(&plain));

    // The dependency decision says which surfaces each consumer may use.
    let said = why(&root, "jobs", "billing");
    assert!(
        said.contains(
            "but only through the public surface of billing: src/billing/api; or its admin surface: src/billing/admin\n",
        ),
        "{said}"
    );
    let said = why(&root, "checkout", "billing");
    assert!(
        said.contains("but only through the public surface of billing: src/billing/api\n"),
        "{said}"
    );
    let said = commands::explain(&config(&root), &root.join("src/billing"))
        .unwrap()
        .text()
        .unwrap();
    assert!(
        said.contains("  surface admin: src/billing/admin (open to jobs: maintenance only)\n"),
        "{said}"
    );
}

#[test]
fn surfaces_without_public_close_the_rest_and_overlapping_ones_join_their_importers() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1
components:
  checkout: { in: src/checkout }
  jobs: { in: src/jobs }
  ops: { in: src/ops }
  billing:
    in: src/billing
    surfaces:
      admin: { in: src/billing/admin, importers: [jobs] }
      audit: { in: src/billing/admin/audit, importers: [ops] }
tests: auto
rules:
  checkout: { may_depend_on: [billing] }
  jobs: { may_depend_on: [billing] }
  ops: { may_depend_on: [billing] }
",
        ),
        ("src/billing/core.ts", "export const core = 1;\n"),
        ("src/billing/admin/audit/log.ts", "export const log = 1;\n"),
        (
            "src/billing/core.test.ts",
            "import { core } from \"./core\";\nexport const t = core;\n",
        ),
        (
            "src/checkout/a.ts",
            "import { core } from \"../billing/core\";\nimport { log } from \"../billing/admin/audit/log\";\nexport const a = core + log;\n",
        ),
        (
            "src/jobs/j.ts",
            "import { log } from \"../billing/admin/audit/log\";\nexport const j = log;\n",
        ),
        (
            "src/ops/o.ts",
            "import { log } from \"../billing/admin/audit/log\";\nexport const o = log;\n",
        ),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let found: Vec<(String, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            assert_eq!(v["rule"], "privacy", "{v}");
            (
                v["file"].as_str().unwrap().to_string(),
                v["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "src/billing/core.test.ts".to_string(),
                "tests -> billing: src/billing/core.ts is not in the public surface of billing (written as src/billing/core.ts)".to_string()
            ),
            (
                "src/checkout/a.ts".to_string(),
                "checkout -> billing: src/billing/core.ts is not in the public surface of billing (written as src/billing/core.ts)".to_string()
            ),
            (
                "src/checkout/a.ts".to_string(),
                "checkout -> billing: src/billing/admin/audit/log.ts is billing's admin and audit surfaces, open to jobs, ops (written as src/billing/admin/audit/log.ts)".to_string()
            ),
        ]
    );
    let said = why(&root, "checkout", "billing");
    assert!(
        said.contains("  but through no path: billing opens no surface to it\n"),
        "{said}"
    );
    let said = why(&root, "ops", "billing");
    assert!(
        said.contains("  but only through billing's audit surface: src/billing/admin/audit\n"),
        "{said}"
    );
}

/// A proposal outside every surface open to its component says which are:
/// the public surface and the named ones that list the component.
#[test]
fn propose_names_the_surfaces_open_to_the_proposer() {
    let (_tmp, root) = surfaces_project(SURFACES);
    let propose = |from: &str| {
        let proposed = commands::Proposed::One {
            source: root.join(from),
            target: Some(root.join("src/billing/internal/ledger.ts")),
            external: None,
        };
        commands::propose(&config(&root), &proposed, false, &mut cache()).unwrap()
    };
    let report = propose("src/jobs/nightly.ts");
    assert!(!report.passed());
    let said = report.text();
    assert!(
        said.contains("public surface of billing: src/billing/api\n"),
        "{said}"
    );
    assert!(
        said.contains("    admin surface of billing, open to jobs: src/billing/admin\n"),
        "{said}"
    );
    let report = parse(&report.json());
    let proposal = &report["proposals"][0];
    // The first run indexed the project, so the text comes from the cache.
    pinned(|| {
        insta::assert_snapshot!(
            "propose_surfaces_text",
            propose("src/jobs/nightly.ts").text()
        )
    });
    assert_eq!(proposal["rule"], "privacy", "{report}");
    assert_eq!(
        proposal["named_surfaces"],
        serde_json::json!([{ "name": "admin", "paths": ["src/billing/admin"] }]),
        "{report}"
    );
    // checkout is open to no named surface, so none is listed.
    let checkout = propose("src/checkout/cart.ts");
    assert!(!checkout.passed());
    let report = parse(&checkout.json());
    assert!(
        report["proposals"][0].get("named_surfaces").is_none(),
        "{report}"
    );
}
