//! What the commands that answer a question say: `impact`, `why`, `explain`,
//! `graph --focus` and `suggest`, the note each gives when its indexes came from
//! the cache, and what a violation adds to help fix it: the owner to ask, the
//! layer that stops it, what `$std` stands for.

use std::fs;
use std::path::Path;

use easement::commands::{self, CheckOptions, Outcome};
use easement::{CacheUse, GraphFormat};
use tempfile::TempDir;

use crate::common::{commit_all, fixture, project, stub_packages};
use crate::reach::{go_package_reach, reach_project, slices_project};
use crate::run::{cache, cache_in, check, config, error, json, parse, pinned, text};

/// What `impact` says of `paths`, relative to `root`, as text.
fn impact_text(root: &Path, paths: &[&str]) -> String {
    let paths: Vec<_> = paths.iter().map(|p| root.join(p)).collect();
    commands::impact(&config(root), &paths, None, &mut cache())
        .unwrap()
        .text()
}

/// What `why` says of `from -> to` in the project at `root`, as text.
fn why_text(root: &Path, from: &str, to: &str) -> String {
    commands::why(&config(root), from, to, &mut cache())
        .unwrap()
        .text()
}

/// What `why` says of `from -> to` in the project at `root`, as data.
fn why_json(root: &Path, from: &str, to: &str) -> serde_json::Value {
    serde_json::to_value(commands::why(&config(root), from, to, &mut cache()).unwrap()).unwrap()
}

/// What `explain` says of `path`, relative to `root`, as data.
fn explain_json(root: &Path, path: &str) -> serde_json::Value {
    serde_json::to_value(commands::explain(&config(root), &root.join(path)).unwrap()).unwrap()
}

/// What `suggest` says of the project at `root`, as text.
fn suggest_text(root: &Path) -> String {
    let found = commands::suggest(&config(root), &mut cache()).unwrap();
    found.text()
}

#[test]
fn impact_lists_what_depends_on_a_change() {
    let (_tmp, root) = reach_project(&[]);
    pinned(|| {
        insta::assert_snapshot!("impact_text", impact_text(&root, &["src/http/Client.kt"]));
        let impacted =
            commands::impact(&config(&root), &[root.join("src/util")], None, &mut cache()).unwrap();
        insta::assert_json_snapshot!("impact_json", parse(&impacted.json().unwrap()));
    });

    commit_all(&root);
    fs::write(
        root.join("src/util/Retry.kt"),
        "package util\nimport http.Client\nclass Retry2\n",
    )
    .unwrap();
    let said = commands::impact(&config(&root), &[], Some("main"), &mut cache())
        .unwrap()
        .text();
    assert!(said.contains("Changed: 1 file(s) in util"), "{said}");
    assert!(
        said.contains("  domain:\n    src/domain/Order.kt:2 -> src/util/Retry.kt"),
        "{said}"
    );
    let refused = error(commands::impact(&config(&root), &[], None, &mut cache()));
    assert!(refused.contains("--diff"), "{refused}");
}

/// What `impact` of `paths`, relative to `root`, says of each dependent: its file,
/// the `file:line` of each hop of its chain, and the changed file the chain ends in.
fn impact_chains(root: &Path, paths: &[&str]) -> Vec<(String, Vec<String>, String)> {
    let paths: Vec<_> = paths.iter().map(|p| root.join(p)).collect();
    let impacted = commands::impact(&config(root), &paths, None, &mut cache()).unwrap();
    let report = parse(&impacted.json().unwrap());
    report["dependents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dependent| {
            let hops = dependent["chain"]["hops"]
                .as_array()
                .unwrap()
                .iter()
                .map(|hop| format!("{}:{}", hop["file"].as_str().unwrap(), hop["line"]))
                .collect();
            let end = dependent["end"].as_str().unwrap().to_string();
            (dependent["file"].as_str().unwrap().to_string(), hops, end)
        })
        .collect()
}

/// Each dependent carries the shortest chain of imports from it to a changed file,
/// and the nearest come first: a direct importer has one hop.
#[test]
fn impact_gives_each_dependent_its_chain_to_the_change() {
    let (_tmp, root) = reach_project(&[]);
    let owned = |file: &str, hops: &[&str], end: &str| {
        (
            file.to_string(),
            hops.iter().map(|hop| hop.to_string()).collect::<Vec<_>>(),
            end.to_string(),
        )
    };
    assert_eq!(
        impact_chains(&root, &["src/http/Client.kt"]),
        [
            owned(
                "src/util/Retry.kt",
                &["src/util/Retry.kt:2"],
                "src/http/Client.kt"
            ),
            owned(
                "src/domain/Order.kt",
                &["src/domain/Order.kt:2", "src/util/Retry.kt:2"],
                "src/http/Client.kt"
            ),
        ]
    );
}

/// Of two changed files, a dependent's chain ends in the nearer one, even when the
/// farther one sorts first.
#[test]
fn impact_chains_a_dependent_to_the_nearest_change() {
    let (_tmp, root) = reach_project(&[
        ("src/http/Auth.kt", "package http\nclass Auth\n"),
        (
            "src/util/Guard.kt",
            "package util\nimport http.Auth\nclass Guard\n",
        ),
        (
            "src/util/Cache.kt",
            "package util\nimport util.Guard\nimport http.Client\nclass Cache\n",
        ),
    ]);
    let chains = impact_chains(&root, &["src/http/Auth.kt", "src/http/Client.kt"]);
    let cache = chains
        .iter()
        .find(|(file, _, _)| file == "src/util/Cache.kt")
        .unwrap();
    assert_eq!(cache.1, ["src/util/Cache.kt:3"], "{chains:?}");
    assert_eq!(cache.2, "src/http/Client.kt", "{chains:?}");
}

/// Of two equally short chains, the one the search finds first is given, on every
/// run: one hop from the change, through the importer first in path order.
#[test]
fn impact_breaks_a_tie_between_chains_by_path() {
    let (_tmp, root) = reach_project(&[
        (
            "src/util/A.kt",
            "package util\nimport http.Client\nclass A\n",
        ),
        (
            "src/util/B.kt",
            "package util\nimport http.Client\nclass B\n",
        ),
        (
            "src/domain/D.kt",
            "package domain\nimport util.B\nimport util.A\nclass D\n",
        ),
    ]);
    let chains = impact_chains(&root, &["src/http/Client.kt"]);
    let d = chains
        .iter()
        .find(|(file, _, _)| file == "src/domain/D.kt")
        .unwrap();
    assert_eq!(d.1, ["src/domain/D.kt:3", "src/util/A.kt:2"], "{chains:?}");
}

/// `explain` and `why` say the same in text and JSON; the JSON names where
/// the policy writes each grant and prohibition.
#[test]
fn explain_and_why_answer_in_text_and_json() {
    let (_tmp, root) = fixture("reach");
    let config = config(&root);
    pinned(|| {
        insta::assert_snapshot!(
            "explain_text",
            commands::explain(&config, &root.join("src/domain/Order.kt"))
                .unwrap()
                .text()
                .unwrap()
        );
        insta::assert_json_snapshot!("explain_json", explain_json(&root, "src/domain/Order.kt"));
        insta::assert_snapshot!("why_text", why_text(&root, "domain", "http"));
        insta::assert_json_snapshot!("why_json", why_json(&root, "domain", "http"));
    });

    // A direct import breaks `dependency`; a chain breaks `reach`, as `check` says.
    assert_eq!(
        why_json(&root, "domain", "http")["decision"]["rules"],
        serde_json::json!(["dependency", "reach"])
    );

    // The surfaces of a component, and who may import it.
    let http = explain_json(&root, "src/http/fake/FakeClient.kt");
    assert_eq!(
        http["allow_importers"],
        serde_json::json!(["shared"]),
        "{http}"
    );
    assert_eq!(
        http["surfaces"][0]["provenance"],
        serde_json::json!({ "field": "components.http.surfaces.fakes", "input": "easement.yml" }),
        "{http}"
    );
    // Each direct dependency with its own decision.
    let direct = why_json(&root, "shared", "http");
    assert_eq!(
        direct["dependencies"][0]["decisions"],
        serde_json::json!([{
            "target": "src/http/api/Client.kt",
            "to": "http",
            "allowed": true,
            "grant": "may_depend_on",
            "because": "retries wrap the transport",
            "provenance": { "field": "rules.shared.may_depend_on[0]", "input": "easement.yml" },
        }]),
        "{direct}"
    );
    // A path no component owns: an error in text, a status in JSON.
    fs::write(root.join("stray.kt"), "class Stray\n").unwrap();
    let stray = error(commands::explain(&config, &root.join("stray.kt")).and_then(|e| e.text()));
    assert!(stray.contains("belongs to no component"), "{stray}");
    assert_eq!(
        explain_json(&root, "stray.kt"),
        serde_json::json!({
            "path": "stray.kt",
            "ownership": { "status": "unowned", "test": false, "target_only": false },
        })
    );
}

#[test]
fn why_shows_the_imports_or_the_chain_between_two_components() {
    let (_tmp, root) = reach_project(&[]);
    let said = why_text(&root, "util", "http");
    assert!(
        said.contains("util -> http is allowed (may_depend_on)"),
        "{said}"
    );
    assert!(
        said.contains("1 dependency(ies):\n  src/util/Retry.kt:2 depends on src/http/Client.kt\n"),
        "{said}"
    );
    let said = why_text(&root, "domain", "http");
    assert!(
        said.contains(
            "domain -> http is forbidden (must_not_reach): the domain knows no transport"
        ),
        "{said}"
    );
    assert!(
        said.contains("domain depends on nothing of http directly; it reaches http through util:"),
        "{said}"
    );
    assert!(
        said.contains("  src/domain/Order.kt:2 -> src/util/Retry.kt:2 -> src/http/Client.kt"),
        "{said}"
    );
    let said = why_text(&root, "http", "domain");
    assert!(said.contains("http does not depend on domain"), "{said}");
    let refused = error(commands::why(&config(&root), "http", "ghost", &mut cache()));
    assert!(refused.contains("unknown component `ghost`"), "{refused}");
    let refused = error(commands::why(
        &config(&root),
        "http",
        "dmoain",
        &mut cache(),
    ));
    assert!(
        refused.contains("unknown component `dmoain`; did you mean `domain`?"),
        "{refused}"
    );
}

#[test]
fn why_follows_the_other_files_of_a_go_package() {
    let (_tmp, root) = go_package_reach("");
    let said = why_text(&root, "domain", "http");
    assert!(said.contains("internal/util/retry.go"), "{said}");
}

#[test]
fn suggest_never_offers_to_allow_a_forbidden_import() {
    let (_tmp, root) = reach_project(&[(
        "src/domain/Order.kt",
        "package domain\nimport http.Client\nclass Order\n",
    )]);
    let said = suggest_text(&root);
    assert!(
        said.contains(
            "Depended on but forbidden by the config (fix the dependencies):\n  domain: http (1 dependency(ies))",
        ),
        "{said}"
    );
    assert!(!said.contains("add to may_depend_on"), "{said}");
}

#[test]
fn graph_focus_keeps_a_component_and_its_neighbours() {
    let (_tmp, root) = slices_project(&[]);
    let graph = |focus: &str| {
        commands::graph(
            &config(&root),
            GraphFormat::Mermaid,
            Some(focus),
            &mut cache(),
        )
    };
    let out = graph("data").unwrap().diagram;
    assert!(out.contains("\"ui\""), "{out}");
    assert!(out.contains("\"data\""), "{out}");
    assert!(!out.contains("\"app\""), "{out}");
    let refused = error(graph("ghost"));
    assert!(refused.contains("unknown component `ghost`"), "{refused}");
    let refused = error(graph("dat"));
    assert!(
        refused.contains("unknown component `dat`; did you mean `data`?"),
        "{refused}"
    );
}

/// Every command that reads indexes says when they came from the cache, like `check`.
#[test]
fn every_command_says_when_its_indexes_came_from_the_cache() {
    let (_tmp, root) = fixture("go");
    let config = config(&root);
    let dir = TempDir::new().unwrap();
    // A cache of its own, read afresh by each command as by each run of the binary.
    let own = || cache_in(dir.path(), CacheUse::On);
    // The first run indexes; every one after it reads the cache.
    let checked = commands::check(&CheckOptions::new(&config), &mut own()).unwrap();
    assert_eq!(checked.outcome(), Outcome::Failed);
    let impacted = commands::impact(&config, &[root.join("internal")], None, &mut own()).unwrap();
    let impact = parse(&impacted.json().unwrap());
    assert!(
        impact["cached"].as_str().unwrap().contains("--no-cache"),
        "{impact}"
    );
    assert!(impacted.text().contains("Cached: "), "{}", impacted.text());
    let why = commands::why(&config, "handler", "repository", &mut own())
        .unwrap()
        .text();
    assert!(why.contains("Cached: "), "{why}");
    let found = commands::suggest(&config, &mut own()).unwrap();
    assert!(
        found.text().contains("came from the cache"),
        "{}",
        found.text()
    );
    let patched = commands::suggest_patch(&config, &mut own()).unwrap();
    assert!(
        patched
            .notes
            .iter()
            .any(|note| note.contains("came from the cache")),
        "{:?}",
        patched.notes
    );
    let drawn = commands::graph(&config, GraphFormat::Mermaid, None, &mut own()).unwrap();
    assert!(
        !drawn.diagram.contains("came from the cache"),
        "{}",
        drawn.diagram
    );
    assert!(
        drawn
            .notes
            .iter()
            .any(|note| note.contains("came from the cache")),
        "{:?}",
        drawn.notes
    );
    let recorded = commands::baseline(&config, false, &mut own()).unwrap();
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("came from the cache"),
        "{}",
        recorded.text()
    );
}

#[test]
fn impact_and_why_say_when_an_answer_may_be_short() {
    let (_tmp, root) = fixture("uncovered");
    fs::write(
        root.join("easement.yml"),
        "version: 1\ncomponents:\n  app: { in: app }\n  tools: { in: tools }\n",
    )
    .unwrap();
    // A cache of its own, so the answer never says it came from the cache.
    let dir = TempDir::new().unwrap();
    let impacted = commands::impact(
        &config(&root),
        &[root.join("app")],
        None,
        &mut cache_in(dir.path(), CacheUse::On),
    )
    .unwrap();
    pinned(|| {
        insta::assert_json_snapshot!("impact_partial_json", parse(&impacted.json().unwrap()));
    });
    let said = why_text(&root, "tools", "app");
    assert!(said.contains("tools does not depend on app"), "{said}");
    assert!(
        said.contains("source files were not covered by an indexer"),
        "{said}"
    );

    // A deleted file: what imported it is gone from the code.
    let (_tmp, root) = reach_project(&[]);
    commit_all(&root);
    fs::remove_file(root.join("src/http/Client.kt")).unwrap();
    let said = commands::impact(&config(&root), &[], Some("main"), &mut cache())
        .unwrap()
        .text();
    assert!(said.contains("Changed: 0 file(s)\n"), "{said}");
    assert!(said.contains("Deleted: src/http/Client.kt"), "{said}");
    assert!(
        said.contains(
            "Evidence gaps (what depends on the changes through this code is missing):\n  1 deleted file(s)",
        ),
        "{said}"
    );
}

#[test]
fn why_reports_what_check_would_say_about_each_import() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b, allow_importers: [c] }\n  c: { in: src/c }\nrules:\n  a: { may_depend_on: [b] }\n",
        ),
        ("src/a/A.kt", "package a\nimport b.*\nclass A\n"),
        ("src/b/B.kt", "package b\nclass B\n"),
        ("src/b/C.kt", "package b\nclass C\n"),
        ("src/c/C.kt", "package c\nclass C\n"),
    ]);
    let said = why_text(&root, "a", "b");
    assert!(
        said.contains("a -> b is allowed (may_depend_on)\n  but b is open only to c"),
        "{said}"
    );
    assert!(
        said.contains(
            "1 dependency(ies):\n  src/a/A.kt:2 depends on src/b/B.kt, src/b/C.kt [violates: dependency]",
        ),
        "{said}"
    );
    let said = why_text(&root, "a", "a");
    assert!(
        said.contains("a -> a is allowed (same component)"),
        "{said}"
    );
}

#[test]
fn why_names_the_surface_an_allowed_dependency_must_go_through() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: src/web }\n  billing: { in: src/billing, public: src/billing/api }\nrules:\n  web: { may_depend_on: [billing] }\n",
        ),
        (
            "src/web/Page.kt",
            "package web\nimport billing.internal.Ledger\nclass Page\n",
        ),
        (
            "src/billing/api/Billing.kt",
            "package billing.api\nclass Billing\n",
        ),
        (
            "src/billing/internal/Ledger.kt",
            "package billing.internal\nclass Ledger\n",
        ),
    ]);
    let said = why_text(&root, "web", "billing");
    assert!(
        said.contains(
            "web -> billing is allowed (may_depend_on)\n  but only through the public surface of billing: src/billing/api\n",
        ),
        "{said}"
    );
    assert!(
        said.contains(
            "src/web/Page.kt:2 depends on src/billing/internal/Ledger.kt [violates: privacy]",
        ),
        "{said}"
    );
}

#[test]
fn std_stands_for_the_standard_library_of_the_language() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  core: { in: src/core }\n  ui: { in: src/ui }\nrules:\n  core: { may_use_external: [$std] }\n  ui: { forbid_external: [{ name: $std, because: the UI goes through core }] }\n",
        ),
        (
            "src/core/A.kt",
            "package core\nimport java.util.UUID\nimport kotlin.math.max\nimport okhttp3.Client\nclass A\n",
        ),
        (
            "src/ui/B.kt",
            "package ui\nimport java.time.Instant\nclass B\n",
        ),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains(
            "src/core/A.kt:4: core -> external okhttp3.Client is not in may_use_external",
        ),
        "{report}"
    );
    assert!(!report.contains("src/core/A.kt:2"), "{report}");
    assert!(!report.contains("src/core/A.kt:3"), "{report}");
    assert!(
        report.contains("src/ui/B.kt:2: ui -> external java.time.Instant is forbidden"),
        "{report}"
    );
    assert!(
        report.contains("because: the UI goes through core"),
        "{report}"
    );
}

#[test]
fn a_violation_names_the_owner_of_the_component_to_ask() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: src/a, owner: '@acme/payments' }\n  b: { in: src/b }\n",
        ),
        ("src/a/A.kt", "package a\nimport b.B\nclass A\n"),
        ("src/b/B.kt", "package b\nclass B\n"),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains(
            "a -> b is not allowed (written as b.B) [dependency]\n  owner: @acme/payments"
        ),
        "{}",
        text(&checked)
    );
    let report = json(&checked);
    assert_eq!(report["violations"][0]["from"], "a");
    assert_eq!(report["owners"]["a"], "@acme/payments");
    let explained = commands::explain(&config(&root), &root.join("src/a/A.kt"))
        .unwrap()
        .text()
        .unwrap();
    assert!(explained.contains("owner: @acme/payments"), "{explained}");
}

#[test]
fn std_says_when_it_cannot_tell_a_language() {
    // Swift is known: Foundation is its standard library.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  ui: { in: Sources/UI }\nrules:\n  ui: { forbid_external: [$std] }\n",
        ),
        (
            "Sources/UI/View.swift",
            "import Foundation\nstruct View {}\n",
        ),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("ui -> external Foundation is forbidden"),
        "{}",
        text(&checked)
    );
    // TypeScript is not: an npm package and a Node builtin look alike to it.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  ui: { in: src/ui }\nrules:\n  ui: { may_use_external: [$std, lodash] }\n",
        ),
        (
            "src/ui/a.ts",
            "import lodash from \"lodash\";\nexport const a = lodash;\n",
        ),
    ]);
    stub_packages(&root, &["lodash"]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert!(
        report["evidence_gaps"][0]["message"]
            .as_str()
            .unwrap()
            .contains("`$std` cannot tell the standard library of .ts files"),
        "{report}"
    );
}

#[test]
fn a_cycle_names_the_owners_of_its_components() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\nno_cycles: true\ncomponents:\n  a: { in: src/a, owner: '@acme/a' }\n  b: { in: src/b, owner: '@acme/b' }\nrules:\n  a: { may_depend_on: [b] }\n  b: { may_depend_on: [a] }\n",
        ),
        ("src/a/A.kt", "package a\nimport b.B\nclass A\n"),
        ("src/b/B.kt", "package b\nimport a.A\nclass B\n"),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("[cycle]\n  owner: @acme/a, @acme/b"),
        "{}",
        text(&checked)
    );
}

#[test]
fn a_layer_violation_says_which_layer_stops_it() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  api: { in: src/api, owner: '@acme/api' }\n  jobs: { in: src/jobs }\n  service: { in: src/service }\n  repo: { in: src/repo }\nlayers:\n  - [api, jobs]\n  - { name: service, closed: true }\n  - repo\n",
        ),
        (
            "src/api/A.kt",
            "package api\nimport repo.R\nimport jobs.J\nimport service.S\nclass A\n",
        ),
        ("src/jobs/J.kt", "package jobs\nclass J\n"),
        (
            "src/service/S.kt",
            "package service\nimport repo.R\nclass S\n",
        ),
        ("src/repo/R.kt", "package repo\nclass R\n"),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(report.contains("src/api/A.kt:2: api -> repo is not allowed: the closed layer service is between them (written as repo.R)"), "{report}");
    assert!(report.contains("src/api/A.kt:3: api -> jobs is not allowed: they are siblings in one layer (written as jobs.J)"), "{report}");
    let said = suggest_text(&root);
    assert!(said.contains("Depended on but forbidden by the config (fix the dependencies):\n  api: jobs (1 dependency(ies)), repo (1 dependency(ies))"), "{said}");
    let said = why_text(&root, "api", "repo");
    assert!(
        said.contains("api -> repo is not allowed: the closed layer service is between them"),
        "{said}"
    );
    assert!(said.contains("owner of api: @acme/api"), "{said}");
    let github = checked.report().github();
    assert!(github.contains("(owner: @acme/api)"), "{github}");
}
