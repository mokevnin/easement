//! The top-level `forbid_external`: external modules no checked file may use,
//! with the components (`allow`) and the paths (`allow_in`) that may all the same.

use std::fs;

use easement::commands::{self, Outcome};

use crate::common::{commit_all, fixture, git, project, stub_packages};
use crate::run::{cache, check, config, json, parse, pinned, text};

#[test]
fn a_top_level_forbidden_module_may_be_used_only_by_the_components_and_paths_it_allows() {
    let (_tmp, root) = fixture("externals_global");
    let checked = check(&root);
    let report = text(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    for said in [
        // Two components and a file of none: only `wiring` may.
        "src/domain/Order.kt:3: domain -> external org.koin.core.component.KoinComponent is forbidden (only wiring may)",
        "src/web/Handler.kt:3: web -> external org.koin.core.Koin is forbidden (only wiring may)",
        "scripts/Seed.kt:3: unowned files -> external org.koin.core.Koin is forbidden (only wiring may)",
        // One file of a component, not the rest of it.
        "src/utils/Strings.kt:3: utils -> external org.threeten.bp.format.DateTimeFormatter is forbidden (only src/utils/Dates.kt may)",
        "src/web/Handler.kt:4: web -> external org.threeten.bp.LocalDate is forbidden (only src/utils/Dates.kt may)",
        // A component's own list adds to the top-level one.
        "src/domain/Order.kt:4: domain -> external okhttp3.OkHttpClient is forbidden",
    ] {
        assert!(report.contains(said), "{said}\n{report}");
    }
    assert!(
        report.contains("because: no domain module or handler sees the DI container"),
        "{report}"
    );
    for granted in ["src/wiring/Module.kt", "src/utils/Dates.kt"] {
        assert!(
            !report.contains(&format!("{granted}:")),
            "{granted}: {report}"
        );
    }
    pinned(|| insta::assert_snapshot!("forbid_external_top_level_text", report));
    // A violation points at the item that forbids it; one of a file of no component
    // comes from no component.
    let violations = json(&checked)["violations"].clone();
    let at = |file: &str, line: u64| {
        violations
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["file"] == file && v["line"] == line)
            .unwrap()
            .clone()
    };
    let unowned = at("scripts/Seed.kt", 3);
    assert_eq!(unowned["from"], "", "{unowned}");
    assert_eq!(unowned["rule"], "external", "{unowned}");
    assert_eq!(
        unowned["provenance"],
        serde_json::json!({ "field": "forbid_external[0]", "input": "easement.yml" })
    );
    assert_eq!(
        at("src/utils/Strings.kt", 3)["provenance"]["field"],
        "forbid_external[1]"
    );
}

/// A component per letter, `a` to `z` and `wiring`, each with a file; with
/// `rules`, `github.com/samber/do` forbidden in the rule of every letter.
fn letters(policy_rest: &str) -> String {
    let mut policy = "version: 1\ncomponents:\n  wiring: { in: src/wiring }\n".to_string();
    for letter in 'a'..='z' {
        policy.push_str(&format!("  {letter}: {{ in: src/{letter} }}\n"));
    }
    policy.push_str(policy_rest);
    policy
}

fn in_every_rule() -> String {
    let mut rules = "rules:\n".to_string();
    for letter in 'a'..='z' {
        rules.push_str(&format!(
            "  {letter}: {{ forbid_external: [{{ name: github.com/samber/do, because: wiring only }}] }}\n"
        ));
    }
    letters(&rules)
}

#[test]
fn review_reads_bans_moved_from_every_rule_into_one_top_level_item_as_that_item_alone() {
    let mut files = vec![("easement.yml".to_string(), in_every_rule())];
    for letter in ('a'..='z').chain(['w']) {
        let dir = if letter == 'w' {
            "wiring".to_string()
        } else {
            letter.to_string()
        };
        files.push((
            format!("src/{dir}/m.ts"),
            "export const m = 1;\n".to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let (_tmp, root) = project(&borrowed);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    let top = |allow: &str| {
        letters(&format!(
            "forbid_external:\n  - {{ name: github.com/samber/do, allow: [{allow}], because: wiring only }}\n"
        ))
    };
    fs::write(config(&root), top("wiring")).unwrap();
    let review = commands::review(&config(&root), "main", &mut cache()).unwrap();
    assert_eq!(
        parse(&review.json())["constraints"],
        serde_json::json!([{
            "component": null,
            "key": "forbid_external",
            "item": "github.com/samber/do",
            "edit": "added",
            "change": "tightened",
            "because": "wiring only",
            "provenance": { "field": "forbid_external[0]", "input": "easement.yml" },
        }])
    );
    // A component let in loosens, and names it.
    fs::write(config(&root), top("wiring, q")).unwrap();
    let review = commands::review(&config(&root), "main", &mut cache()).unwrap();
    let said = review.text();
    assert!(
        said.contains("forbid_external: `github.com/samber/do` may now be used by `q` (loosened)"),
        "{said}"
    );
    assert!(
        said.contains("1 constraint(s) loosened, 1 tightened"),
        "{said}"
    );
}

#[test]
fn explain_names_the_top_level_forbidden_modules_of_a_path_and_what_lets_it_use_them() {
    let (_tmp, root) = fixture("externals_global");
    let explained = |path: &str| commands::explain(&config(&root), &root.join(path)).unwrap();
    let dates = explained("src/utils/Dates.kt");
    let said = dates.text().unwrap();
    for line in [
        "  forbid_external forbids: org.koin (only wiring may: no domain module or handler sees the DI container)\n",
        "  may use, although forbid_external forbids it: org.threeten.bp (allow_in `src/utils/Dates.kt`: dates go through src/utils/Dates.kt)\n",
    ] {
        assert!(said.contains(line), "{line}{said}");
    }
    let wiring = explained("src/wiring/Module.kt");
    let bans = serde_json::to_value(&wiring).unwrap()["forbid_external"].clone();
    assert_eq!(bans[0]["allow"], "wiring", "{bans}");
    assert_eq!(bans[0]["allowed"], true, "{bans}");
    assert_eq!(
        bans[0]["provenance"],
        serde_json::json!({ "field": "forbid_external[0].allow", "input": "easement.yml" })
    );
    assert_eq!(bans[1]["allowed"], false, "{bans}");
    // A component whose `may_use_external` leaves the module out still may not use it.
    let policy = fs::read_to_string(config(&root)).unwrap().replace(
        "web: { may_depend_on: [domain, utils] }",
        "web: { may_depend_on: [domain, utils] }\n  utils: { may_use_external: [kotlin] }",
    );
    fs::write(config(&root), &policy).unwrap();
    let narrowed = serde_json::to_value(explained("src/utils/Dates.kt")).unwrap();
    assert_eq!(
        narrowed["forbid_external"][1]["allowed"], false,
        "{narrowed}"
    );
    assert!(
        !explained("src/utils/Dates.kt")
            .text()
            .unwrap()
            .contains("may use, although forbid_external")
    );
    fs::write(
        config(&root),
        policy.replace(
            "may_use_external: [kotlin]",
            "may_use_external: [kotlin, org.threeten]",
        ),
    )
    .unwrap();
    let covered = serde_json::to_value(explained("src/utils/Dates.kt")).unwrap();
    assert_eq!(covered["forbid_external"][1]["allowed"], true, "{covered}");
    // A file of no component: the top-level lists alone bind it.
    let unowned = explained("scripts/Seed.kt").text().unwrap();
    assert!(
        unowned.starts_with(
            "scripts/Seed.kt\n  component: none, so only the top-level lists judge it; of the external modules:\n"
        ),
        "{unowned}"
    );
    pinned(|| {
        insta::assert_snapshot!("explain_external_bans_text", said);
        insta::assert_json_snapshot!("explain_external_bans_json", dates);
        insta::assert_snapshot!("explain_external_bans_unowned_text", unowned);
    });
}

#[test]
fn a_path_allowed_a_forbidden_module_that_matches_no_file_is_unmatched() {
    let (_tmp, root) = fixture("externals_global");
    let policy = fs::read_to_string(config(&root)).unwrap().replace(
        "allow_in: [src/utils/Dates.kt]",
        "allow_in: [src/utils/Date.kt]",
    );
    fs::write(config(&root), policy).unwrap();
    let report = text(&check(&root));
    assert!(
        report.contains(
            "easement.yml:10: `forbid_external` `org.threeten.bp` is allowed in `src/utils/Date.kt`, which matches no file; fix the pattern or drop it (files that `exclude` or .gitignore hide do not count) [unmatched]"
        ),
        "{report}"
    );
}

#[test]
fn policy_examples_of_external_modules_get_the_decision_of_the_top_level_list() {
    let (_tmp, root) = fixture("externals_global");
    let mut policy = fs::read_to_string(config(&root)).unwrap();
    policy.push_str(
        "examples:
  - { from: src/web/New.kt, external: org.koin.core.Koin, expect: rejected, rule: external }
  - { from: src/wiring/New.kt, external: org.koin.core.Koin, expect: allowed }
  - { from: src/utils/Dates.kt, external: org.threeten.bp.LocalDate, expect: allowed }
  - { from: src/utils/New.kt, external: org.threeten.bp.LocalDate, expect: rejected }
  - { from: scripts/New.kt, external: org.koin.core.Koin, expect: rejected, rule: external }
",
    );
    fs::write(config(&root), &policy).unwrap();
    let report = text(&check(&root));
    assert!(!report.contains("[example]"), "{report}");
    // Flipped, each fails.
    fs::write(
        config(&root),
        policy.replace("expect: rejected, rule: external", "expect: allowed"),
    )
    .unwrap();
    let report = text(&check(&root));
    assert!(
        report.contains("example src/web/New.kt -> external org.koin.core.Koin: expects allowed, but the policy now decides rejected [external]: web -> external org.koin.core.Koin is forbidden (only wiring may)"),
        "{report}"
    );
}

/// `$std` at the top level binds the files of no component too: where it cannot
/// tell the standard library, the report says so for them as well.
#[test]
fn a_top_level_std_ban_over_a_file_of_no_component_it_cannot_judge_is_a_gap() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  ui: { in: src/ui }\nforbid_external: [{ name: $std, allow: [ui] }]\n",
        ),
        ("src/ui/a.ts", "export const a = 1;\n"),
        (
            "scripts/seed.ts",
            "import lodash from \"lodash\";\nexport const seed = lodash;\n",
        ),
    ]);
    stub_packages(&root, &["lodash"]);
    let checked = check(&root);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Passed, "{report}");
    assert!(
        report["evidence_gaps"][0]["message"]
            .as_str()
            .unwrap()
            .contains("`$std` cannot tell the standard library of .ts files"),
        "{report}"
    );
}

/// `$std` at the top level is the standard library of each importing file, and
/// `allow` lifts it for a component as for any module.
#[test]
fn a_top_level_std_ban_holds_for_every_file_but_those_it_allows() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  core: { in: src/core }\n  ui: { in: src/ui }\nforbid_external: [{ name: $std, allow: [core], because: the platform goes through core }]\n",
        ),
        (
            "src/core/A.kt",
            "package core\nimport java.util.UUID\nclass A\n",
        ),
        (
            "src/ui/B.kt",
            "package ui\nimport java.time.Instant\nimport okhttp3.Client\nclass B\n",
        ),
    ]);
    let checked = check(&root);
    let report = text(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    assert!(
        report.contains(
            "src/ui/B.kt:2: ui -> external java.time.Instant is forbidden (only core may)"
        ),
        "{report}"
    );
    // Not the standard library, and a component let in.
    assert!(!report.contains("src/ui/B.kt:3"), "{report}");
    assert!(!report.contains("src/core/A.kt"), "{report}");
}

/// A proposal from a file of no component is judged by the top-level list: a
/// module it forbids is rejected, with no component named; any other is unowned.
#[test]
fn propose_judges_an_external_module_of_a_file_of_no_component_by_the_top_level_list() {
    let (_tmp, root) = fixture("externals_global");
    let proposed = |module: &str| commands::Proposed::One {
        source: root.join("scripts/New.kt"),
        target: None,
        external: Some(module.to_string()),
    };
    let answer = commands::propose(
        &config(&root),
        &proposed("org.koin.core.Koin"),
        true,
        &mut cache(),
    )
    .unwrap();
    let judged = parse(&answer.json());
    assert!(!answer.passed(), "{judged}");
    let found = &judged["proposals"][0];
    assert_eq!(found["decision"], "rejected", "{judged}");
    assert_eq!(found["rule"], "external", "{judged}");
    assert_eq!(found["component"], serde_json::Value::Null, "{judged}");
    assert_eq!(
        found["message"],
        "unowned files -> external org.koin.core.Koin is forbidden (only wiring may)"
    );
    let other = commands::propose(
        &config(&root),
        &proposed("okhttp3.Client"),
        true,
        &mut cache(),
    )
    .unwrap();
    assert_eq!(parse(&other.json())["proposals"][0]["decision"], "unowned");
}
