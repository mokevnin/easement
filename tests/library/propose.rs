//! `propose`: the decision on a dependency before its code exists, and what
//! the proposals of a plan do together to the graph: the cycles, reach
//! chains, crossed limits and missing diagram arrows `check` would report once
//! the imports are written.
//!
//! The paths of a proposal are the absolute paths of files under the root,
//! most of which do not exist yet.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use easement::commands::{self, Outcome, Proposed};
use tempfile::TempDir;

use crate::common::{SHOP, fixture, project, shop, stub_packages};
use crate::run::{cache, check, check_with, config, error, json, parse, text};

/// The shop with a diagram that draws only `billing -> checkout`, and a
/// `max_instability` on `notify`.
fn drawn_shop() -> (TempDir, PathBuf) {
    let (tmp, root) = shop();
    let policy = SHOP.replace(
        "notify: { may_depend_on: [shipping, billing] }",
        "notify: { may_depend_on: [shipping, billing], max_instability: 0.4 }",
    );
    fs::write(
        root.join("easement.yml"),
        format!("{policy}diagram: ARCH.md\n"),
    )
    .unwrap();
    fs::write(
        root.join("ARCH.md"),
        "```mermaid\nflowchart LR\n  billing --> checkout\n```\n",
    )
    .unwrap();
    (tmp, root)
}

/// The shop where `notify` may use `nodemailer` alone, with `nodemailer` and
/// `lodash` installed.
fn mailing_shop() -> (TempDir, PathBuf) {
    let (tmp, root) = shop();
    let policy = SHOP.replace(
        "notify: { may_depend_on: [shipping, billing] }",
        "notify: { may_depend_on: [shipping, billing], may_use_external: [nodemailer] }",
    );
    fs::write(root.join("easement.yml"), policy).unwrap();
    stub_packages(&root, &["nodemailer", "lodash"]);
    (tmp, root)
}

/// The shop where `exclude` hides the scripts and every file must have a component.
fn strict_shop() -> (TempDir, PathBuf) {
    let (tmp, root) = shop();
    fs::write(
        root.join("easement.yml"),
        format!("{SHOP}strict: true\nexclude: [\"src/scripts/**\"]\n"),
    )
    .unwrap();
    (tmp, root)
}

/// What `propose` answers: whether it passes, and its report as text and as data.
struct Answer {
    passed: bool,
    text: String,
    json: serde_json::Value,
}

/// What `propose` answers for `proposed` in the project at `root`, or the
/// error it stops on.
fn propose(root: &Path, proposed: &Proposed, no_graph: bool) -> anyhow::Result<Answer> {
    let report = commands::propose(&config(root), proposed, no_graph, &mut cache())?;
    Ok(Answer {
        passed: report.passed(),
        text: report.text(),
        json: parse(&report.json()),
    })
}

/// An external proposal judged by a rule that names `$std` says when `$std` cannot
/// tell the standard library of its source's language, as `check` does, and says
/// nothing of a language it can tell.
#[test]
fn an_external_proposal_says_when_std_cannot_tell_its_standard_library() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  ui: { in: src/ui }\nrules:\n  ui: { may_use_external: [$std, lodash] }\n",
        ),
        ("src/ui/a.ts", "export const a = 1;\n"),
    ]);
    let said = |source: &str, module: &str| {
        propose(
            &root,
            &Proposed::One {
                source: root.join(source),
                target: None,
                external: Some(module.into()),
            },
            true,
        )
        .unwrap()
        .text
    };
    let gap = "`$std` cannot tell the standard library of .ts files";
    let ts = said("src/ui/b.ts", "lodash");
    assert!(ts.contains(gap), "{ts}");
    // The summary names what the answer leaves out by kind.
    assert!(
        ts.ends_with("graph not judged (narrowed: --no-graph; 1 evidence gap)\n"),
        "{ts}"
    );
    let go = said("src/ui/b.go", "fmt");
    assert!(!go.contains("`$std` cannot tell"), "{go}");
    assert!(go.ends_with("(narrowed: --no-graph)\n"), "{go}");
}

/// The proposal of an import of the file `to` from the file `from`, both
/// relative to `root`.
fn one(root: &Path, from: &str, to: &str) -> Proposed {
    Proposed::One {
        source: root.join(from),
        target: Some(root.join(to)),
        external: None,
    }
}

#[test]
fn propose_rejects_a_path_outside_the_public_surface_and_names_the_surface() {
    let (_tmp, root) = shop();
    let answer = propose(
        &root,
        &one(
            &root,
            "src/checkout/pay.ts",
            "src/billing/internal/ledger.ts",
        ),
        false,
    )
    .unwrap();
    assert!(!answer.passed);
    let said = &answer.text;
    assert!(
        said.contains("src/checkout/pay.ts -> src/billing/internal/ledger.ts: rejected [privacy]"),
        "{said}"
    );
    assert!(
        said.contains("public surface of billing: src/billing/api"),
        "{said}"
    );
    assert!(
        said.contains("because: billing internals change without notice"),
        "{said}"
    );
    let report = &answer.json;
    let proposal = &report["proposals"][0];
    assert_eq!(proposal["decision"], "rejected");
    assert_eq!(proposal["rule"], "privacy");
    assert_eq!(proposal["component"], "checkout");
    assert_eq!(proposal["target_component"], "billing");
    assert_eq!(
        proposal["public_surface"],
        serde_json::json!(["src/billing/api"])
    );
    assert_eq!(report["passed"], false);
}

#[test]
fn propose_reports_the_cycle_an_allowed_proposal_closes_with_its_witness() {
    let (_tmp, root) = shop();
    let answer = propose(
        &root,
        &one(&root, "src/checkout/pay.ts", "src/billing/api/charge.ts"),
        false,
    )
    .unwrap();
    assert!(!answer.passed);
    let said = &answer.text;
    assert!(
        said.contains(
            "src/checkout/pay.ts -> src/billing/api/charge.ts: allowed by may_depend_on (checkout -> billing)",
        ),
        "{said}"
    );
    assert!(
        said.contains(
            "component dependency cycle: checkout -> billing -> checkout [cycle]\n    checkout -> billing: proposed: src/checkout/pay.ts -> src/billing/api/charge.ts\n    billing -> checkout: src/billing/api/charge.ts:1\n",
        ),
        "{said}"
    );
    let report = &answer.json;
    assert_eq!(report["proposals"][0]["decision"], "allowed");
    assert_eq!(
        report["consequences"],
        serde_json::json!([{
            "rule": "cycle",
            "message": "component dependency cycle: checkout -> billing -> checkout",
            "from": "billing,checkout",
            "witness": [
                {"from": "checkout", "to": "billing", "proposal": 0},
                {"from": "billing", "to": "checkout", "file": "src/billing/api/charge.ts", "line": 1}
            ],
            "proposals": [0]
        }])
    );
}

#[test]
fn propose_reports_a_cycle_two_allowed_proposals_close_together() {
    let (_tmp, root) = shop();
    let plan = r#"[{"from": "src/shipping/ship.ts", "to": "src/notify/mail.ts"},
                   {"from": "src/notify/mail.ts", "to": "src/shipping/ship.ts"}]"#;
    // Each alone closes nothing.
    for (from, to) in [
        ("src/shipping/ship.ts", "src/notify/mail.ts"),
        ("src/notify/mail.ts", "src/shipping/ship.ts"),
    ] {
        let answer = propose(&root, &one(&root, from, to), false).unwrap();
        assert!(answer.passed, "{}", answer.text);
        assert!(answer.text.contains("no new cycle"), "{}", answer.text);
    }
    let answer = propose(&root, &Proposed::Plan(plan.into()), false).unwrap();
    assert!(!answer.passed);
    let report = &answer.json;
    assert_eq!(report["proposals"][0]["decision"], "allowed");
    assert_eq!(report["proposals"][1]["decision"], "allowed");
    let consequences = report["consequences"].as_array().unwrap();
    assert_eq!(consequences.len(), 1, "{report:#}");
    assert_eq!(consequences[0]["rule"], "cycle");
    assert_eq!(consequences[0]["proposals"], serde_json::json!([0, 1]));
    let said = &answer.text;
    assert!(
        said.contains("component dependency cycle: notify -> shipping -> notify [cycle]"),
        "{said}"
    );
    assert!(
        said.contains("proposed: src/notify/mail.ts -> src/shipping/ship.ts"),
        "{said}"
    );
    assert!(
        said.contains("proposed: src/shipping/ship.ts -> src/notify/mail.ts"),
        "{said}"
    );
}

/// The `../x` specifier that imports `to` from `from`, both relative to the root.
fn specifier(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = from.split('/').collect();
    let from_dir = &from_dir[..from_dir.len() - 1];
    let to: Vec<&str> = to.trim_end_matches(".ts").split('/').collect();
    let common = from_dir.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from_dir.len() - common];
    if parts.is_empty() {
        parts.push(".");
    }
    parts.extend(&to[common..]);
    parts.join("/")
}

/// The rule, components and limit of a violation of a rule over the whole graph,
/// as `check` reports it.
fn graph_keys(report: &serde_json::Value) -> BTreeSet<String> {
    report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| ["cycle", "reach", "metric", "diagram"].contains(&v["rule"].as_str().unwrap()))
        .map(|v| {
            let limit = if v["rule"] == "metric" {
                v["target"].as_str().unwrap()
            } else {
                ""
            };
            format!("{} {} {} {limit}", v["rule"], v["from"], v["to"])
        })
        .collect()
}

/// Makes a scratch project.
type MakeProject = fn() -> (TempDir, PathBuf);

/// Proposes `plan` in a fresh project from `make`, then writes its imports and
/// runs `check`: the decision on each import and the graph violations the
/// imports add must be what `propose` said before the code existed. A target
/// outside `src/` is an external module.
fn assert_propose_agrees_with_check(make: MakeProject, plan: &[(&str, &str)]) {
    let (_tmp, root) = make();
    let external = |to: &str| !to.starts_with("src/");
    let list: Vec<serde_json::Value> = plan
        .iter()
        .map(|(from, to)| {
            if external(to) {
                serde_json::json!({"from": from, "external": to})
            } else {
                serde_json::json!({"from": from, "to": to})
            }
        })
        .collect();
    let proposed = propose(
        &root,
        &Proposed::Plan(serde_json::to_string(&list).unwrap()),
        false,
    )
    .unwrap()
    .json;
    let before = json(&check_with(&root, |o| o.no_baseline = true));
    let mut at = Vec::new();
    for (from, to) in plan {
        let path = root.join(from);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if external(to) {
            text.push_str(&format!("import {to}Module from \"{to}\";\n"));
        } else {
            let name = to.rsplit('/').next().unwrap().trim_end_matches(".ts");
            text.push_str(&format!(
                "import {{ {name} }} from \"{}\";\n",
                specifier(from, to)
            ));
        }
        at.push((from.to_string(), text.lines().count()));
        fs::write(&path, text).unwrap();
    }
    let after = json(&check_with(&root, |o| o.no_baseline = true));
    for (i, (file, line)) in at.iter().enumerate() {
        let rules: Vec<&str> = after["violations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["file"] == file.as_str() && v["line"] == *line)
            .filter(|v| {
                !["cycle", "metric", "diagram", "reach"].contains(&v["rule"].as_str().unwrap())
            })
            .map(|v| v["rule"].as_str().unwrap())
            .collect();
        let decided: Vec<&str> = proposed["proposals"][i]["rule"]
            .as_str()
            .into_iter()
            .collect();
        assert_eq!(decided, rules, "decision on {plan:?} #{i}");
    }
    let new: BTreeSet<String> = graph_keys(&after)
        .difference(&graph_keys(&before))
        .cloned()
        .collect();
    let consequences: BTreeSet<String> = proposed["consequences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            format!(
                "{} {} {} {}",
                c["rule"],
                c["from"],
                serde_json::Value::from(c["to"].as_str().unwrap_or_default()),
                c["limit"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(consequences, new, "consequences of {plan:?}");
    assert_eq!(proposed["passed"], after["passed"], "verdict on {plan:?}");
}

#[test]
fn propose_agrees_with_check_once_the_imports_are_written() {
    let plans: &[(MakeProject, &[(&str, &str)])] = &[
        (
            shop,
            &[("src/checkout/pay.ts", "src/billing/internal/ledger.ts")],
        ),
        (
            shop,
            &[("src/checkout/pay.ts", "src/billing/api/charge.ts")],
        ),
        (
            shop,
            &[
                ("src/shipping/ship.ts", "src/notify/mail.ts"),
                ("src/notify/mail.ts", "src/shipping/ship.ts"),
            ],
        ),
        (
            shop,
            &[
                ("src/shipping/ship.ts", "src/notify/mail.ts"),
                ("src/notify/mail.ts", "src/billing/api/charge.ts"),
            ],
        ),
        (
            shop,
            &[
                ("src/checkout/pay.ts", "src/billing/api/charge.ts"),
                ("src/checkout/fresh.ts", "src/notify/mail.ts"),
            ],
        ),
        // A missing arrow and a crossed `max_instability`.
        (
            drawn_shop,
            &[("src/notify/mail.ts", "src/shipping/ship.ts")],
        ),
        // `exclude` hides the source: no rule judges it, `strict` or not.
        (
            strict_shop,
            &[("src/scripts/seed.ts", "src/checkout/cart.ts")],
        ),
        // External modules: one `may_use_external` lists, one it does not.
        (
            mailing_shop,
            &[
                ("src/notify/mail.ts", "nodemailer"),
                ("src/notify/mail.ts", "lodash"),
            ],
        ),
        // A new file of no component under `strict`.
        (
            strict_shop,
            &[("src/tools/seed.ts", "src/checkout/cart.ts")],
        ),
    ];
    for (make, plan) in plans {
        assert_propose_agrees_with_check(*make, plan);
    }
}

#[test]
fn propose_names_every_proposal_that_makes_a_cycle() {
    let (_tmp, root) = shop();
    // 1 and 2 close checkout -> notify -> checkout on their own, inside the
    // group that 0 closes with the existing billing -> checkout.
    let plan = r#"[{"from": "src/checkout/pay.ts", "to": "src/billing/api/charge.ts"},
            {"from": "src/checkout/pay.ts", "to": "src/notify/mail.ts"},
            {"from": "src/notify/mail.ts", "to": "src/checkout/cart.ts"}]"#;
    let answer = propose(&root, &Proposed::Plan(plan.into()), false).unwrap();
    assert!(!answer.passed);
    let report = &answer.json;
    let cycle = report["consequences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["rule"] == "cycle")
        .unwrap();
    assert_eq!(
        cycle["proposals"],
        serde_json::json!([0, 1, 2]),
        "{report:#}"
    );
}

#[test]
fn propose_does_not_judge_a_source_exclude_hides() {
    let (_tmp, root) = strict_shop();
    let answer = propose(
        &root,
        &one(&root, "src/scripts/seed.ts", "src/checkout/cart.ts"),
        false,
    )
    .unwrap();
    assert!(answer.passed, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("src/scripts/seed.ts -> src/checkout/cart.ts: excluded\n"),
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("`exclude` hides src/scripts/seed.ts"),
        "{}",
        answer.text
    );
}

/// `check` never reads a `target_only` file as a source, so no rule judges
/// what it would import, and a policy example from it cannot be judged.
#[test]
fn propose_does_not_judge_a_source_target_only_names() {
    let (_tmp, root) = project(&[(
        "easement.yml",
        "version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
rules:
  web: { must_not_depend_on: [storage] }
target_only: [src/web/gen]
examples:
  - { from: src/web/gen/Api.kt, to: src/storage/Db.kt, expect: rejected }
",
    )]);
    let answer = propose(
        &root,
        &one(&root, "src/web/gen/Api.kt", "src/storage/Db.kt"),
        true,
    )
    .unwrap();
    assert!(answer.passed, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("src/web/gen/Api.kt -> src/storage/Db.kt: target_only\n"),
        "{}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains("`target_only` names src/web/gen/Api.kt, so no rule judges its dependencies"),
        "{}",
        answer.text
    );
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let example = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["rule"] == "example")
        .unwrap_or_else(|| panic!("{report}"));
    assert!(
        example["message"]
            .as_str()
            .unwrap()
            .contains("cannot be judged: `target_only` names src/web/gen/Api.kt"),
        "{report}"
    );
}

#[test]
fn propose_without_the_graph_judges_decisions_alone_and_says_so() {
    let (_tmp, root) = shop();
    let answer = propose(
        &root,
        &one(&root, "src/checkout/pay.ts", "src/billing/api/charge.ts"),
        true,
    )
    .unwrap();
    assert!(answer.passed, "{}", answer.text);
    assert_eq!(answer.json["consequences"], serde_json::Value::Null);
    assert_eq!(answer.json["narrowed"][0]["kind"], "no_graph");
    let answer = propose(
        &root,
        &Proposed::One {
            source: "src/checkout/pay.ts".into(),
            target: None,
            external: Some("lodash".into()),
        },
        true,
    )
    .unwrap();
    assert!(answer.passed, "{}", answer.text);
    assert!(
        answer.text.contains(
            "src/checkout/pay.ts -> external lodash: allowed (checkout): no rule restricts it"
        ),
        "{}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains("Graph consequences not judged (`--no-graph`)"),
        "{}",
        answer.text
    );
}

#[test]
fn propose_rejects_an_unowned_source_under_strict() {
    let (_tmp, root) = shop();
    let seed = one(&root, "src/scripts/seed.ts", "src/checkout/cart.ts");
    let answer = propose(&root, &seed, false).unwrap();
    assert!(answer.passed, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("src/scripts/seed.ts -> src/checkout/cart.ts: unowned\n"),
        "{}",
        answer.text
    );
    fs::write(root.join("easement.yml"), format!("{SHOP}strict: true\n")).unwrap();
    let answer = propose(&root, &seed, false).unwrap();
    assert!(!answer.passed);
    assert!(
        answer
            .text
            .contains("src/scripts/seed.ts -> src/checkout/cart.ts: unowned [orphan]"),
        "{}",
        answer.text
    );
}

#[test]
fn propose_refuses_a_malformed_plan() {
    let (_tmp, root) = shop();
    for (plan, says) in [
        ("{", "a plan is a JSON list"),
        ("[]", "the plan proposes nothing"),
        (
            r#"[{"from": "a.ts"}]"#,
            "exactly one of `to` and `external`",
        ),
        (
            r#"[{"from": "a.ts", "to": "b.ts", "external": "x"}]"#,
            "exactly one of `to` and `external`",
        ),
    ] {
        let refused = error(propose(&root, &Proposed::Plan(plan.into()), false));
        assert!(refused.contains(says), "{plan}: {refused}");
    }
}

/// A file of a Go package, new or not, is judged as the package: what `check`
/// says once the file imports another package.
#[test]
fn propose_judges_a_go_file_as_its_package() {
    let (_tmp, root) = fixture("go");
    let answer = propose(
        &root,
        &one(
            &root,
            "internal/handlers/new.go",
            "internal/repository/other.go",
        ),
        false,
    )
    .unwrap();
    assert!(!answer.passed);
    let report = &answer.json;
    assert_eq!(report["proposals"][0]["component"], "handler");
    assert_eq!(report["proposals"][0]["target_component"], "repository");
    assert_eq!(report["proposals"][0]["rule"], "dependency");
    fs::write(
        root.join("internal/handlers/new.go"),
        "package handlers\n\nimport \"example.com/shop/internal/repository\"\n\nvar all = repository.All\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("internal/handlers/new.go:3: handler -> repository is not allowed"),
        "{}",
        text(&checked)
    );
}
