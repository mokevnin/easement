//! `check --diff`: a verdict narrowed to the lines changed since a base
//! revision, and what a change to the policy since then causes, judged in full
//! when the delta cannot be told.

use std::fs;

use easement::CacheUse;
use easement::commands::{self, CheckOptions, Outcome};
use tempfile::TempDir;

use crate::common::{
    WEB_AND_STORAGE, commit_all, fixture, git, on_feature, project, stub_packages, web_and_storage,
};
use crate::run::{
    by_rule, cache_in, check, check_with, config, error, json, leaves_out, pinned, text,
    try_check_with,
};

/// Sets the options of a run of `check --diff main`.
pub(crate) fn diff_main(options: &mut CheckOptions) {
    options.diff = Some("main".into());
}

/// A diff run narrows the verdict by request; it reads no less of the project,
/// so it has no evidence gap.
#[test]
fn a_diff_run_is_narrowed_without_evidence_gaps() {
    let (_tmp, root) = fixture("go");
    commit_all(&root);
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert_eq!(
        report["narrowed"],
        serde_json::json!([{ "kind": "diff", "limited_to": "main" }])
    );
    assert_eq!(report["evidence_gaps"], serde_json::json!([]));
    assert_eq!(report["accepted_gaps"], serde_json::json!([]));
    assert!(leaves_out(&report), "{report}");
}

#[test]
fn diff_mode_judges_only_changed_lines() {
    let (_tmp, root) = fixture("ts");
    on_feature(&root);

    // Legacy violations exist on main, but this change touches nothing.
    assert_eq!(check(&root).outcome(), Outcome::Failed);
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(
        text(&checked).contains("Checking lines changed since main"),
        "{}",
        text(&checked)
    );

    // A committed new violation in an existing file.
    let checkout = root.join("src/app/checkout.ts");
    let source = fs::read_to_string(&checkout).unwrap();
    fs::write(
        &checkout,
        format!("import * as ui from \"../ui/page\";\n{source}"),
    )
    .unwrap();
    git(&root, &["commit", "-q", "-am", "leak"]);
    // An uncommitted, untracked file with another one.
    fs::write(
        root.join("src/domain/leak.ts"),
        "import { save } from \"../infra/db\";\n",
    )
    .unwrap();

    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    // A verdict over the changed lines says nothing about the rest of the project.
    assert_eq!(report["narrowed"][0]["kind"], "diff", "{report}");
    assert_eq!(report["narrowed"][0]["limited_to"], "main", "{report}");
    let found: Vec<(String, u64)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().into(),
                v["line"].as_u64().unwrap(),
            )
        })
        .collect();
    // checkout.ts -> infra moved to line 4 but is untouched, so it is not reported.
    assert_eq!(
        found,
        vec![
            ("src/app/checkout.ts".to_string(), 1),
            ("src/domain/leak.ts".to_string(), 1),
        ]
    );
    assert_eq!(report["diff"]["base"], "main");
    let new: Vec<&str> = report["stats"]["new_dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert_eq!(new, ["app -> ui", "domain -> infra"]);
    assert!(
        text(&checked).contains(
            "New dependencies between components in this change:\n  app -> ui\n  domain -> infra\n",
        ),
        "{}",
        text(&checked)
    );
    let github = checked.report().github();
    assert!(
        github.contains(
            "::notice title=easement graph::New component dependencies in this change: app -> ui, domain -> infra",
        ),
        "{github}"
    );

    let said = error(try_check_with(&root, |o| {
        o.diff = Some("no-such-branch".into());
    }));
    assert!(said.contains("merge-base"), "{said}");
}

/// A file moved into another component is new there: every import it keeps is
/// judged, although git only renamed it.
#[test]
fn diff_mode_judges_a_moved_file_in_full() {
    let (_tmp, root) = fixture("ts");
    on_feature(&root);
    git(&root, &["mv", "src/ui/page.tsx", "src/domain/page.tsx"]);
    git(&root, &["commit", "-q", "-m", "move"]);

    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed, "{}", text(&checked));
    let report = json(&checked);
    let found: Vec<(&str, u64)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["file"].as_str().unwrap(), v["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(found, [("src/domain/page.tsx", 1)], "{report}");
}

/// A change to the policy is part of the change: what it newly rejects fails
/// `--diff` although no import line changed.
#[test]
fn diff_mode_judges_what_a_change_to_the_policy_causes() {
    let (_tmp, root) = web_and_storage(WEB_AND_STORAGE);
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);

    // An unrelated rule changes: the unchanged imports stay out of the report.
    fs::write(
        config(&root),
        WEB_AND_STORAGE.replace("storage is the bottom", "nothing below storage"),
    )
    .unwrap();
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);

    // A prohibition makes the unchanged import of storage fail.
    fs::write(
        config(&root),
        WEB_AND_STORAGE.replace(
            "web: { may_depend_on: [] }",
            "web: { must_not_depend_on: [storage] }",
        ),
    )
    .unwrap();
    // A cache of its own, so that no other run decides whether it is cached:
    // the first run indexes, the second reads the cache.
    let own = TempDir::new().unwrap();
    let mut options = CheckOptions::new(config(&root));
    diff_main(&mut options);
    let run = || commands::check(&options, &mut cache_in(own.path(), CacheUse::On)).unwrap();
    let checked = run();
    assert_eq!(checked.outcome(), Outcome::Failed);
    let mut report = json(&checked);
    // The report agents read is pinned; the merge-base differs every run.
    let merge_base = report["diff"]["merge_base"].as_str().unwrap().to_string();
    report["diff"]["merge_base"] = "[merge-base]".into();
    let cached = run();
    assert_eq!(cached.outcome(), Outcome::Failed);
    pinned(|| {
        insta::assert_json_snapshot!("check_diff_policy_json", report);
        insta::assert_snapshot!(
            "check_diff_policy_text",
            text(&cached).replace(&merge_base[..12], "[merge-base]")
        );
    });
    let found: Vec<(&str, u64)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["file"].as_str().unwrap(), v["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(found, [("src/web/page.ts", 1)], "{report}");
    assert_eq!(report["diff"]["in_full"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["diff"]["policy_changed"],
        serde_json::json!(["easement.yml"]),
        "{report}"
    );
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let said = text(&checked);
    assert!(
        said.contains("Checking lines changed since main (merge-base "),
        "{said}"
    );
    assert!(
        said.contains("and what the change to the policy causes"),
        "{said}"
    );

    // Committed, the change to the policy is judged the same.
    git(&root, &["commit", "-q", "-am", "forbid storage"]);
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("src/web/page.ts:1:"),
        "{}",
        text(&checked)
    );
}

/// A rule taken out of `warn` fails what it only warned about before: a
/// warning under the old policy is no violation it already had.
#[test]
fn diff_mode_judges_a_rule_taken_out_of_warn() {
    let legacy = WEB_AND_STORAGE.replace("common: [storage]\n", "");
    let (_tmp, root) = web_and_storage(&format!("{legacy}warn: [dependency]\n"));
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);
    fs::write(config(&root), &legacy).unwrap();
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("src/web/page.ts:1:"),
        "{}",
        text(&checked)
    );
}

/// When the scan of the merge-base policy may differ from this one, the delta
/// cannot be told: the whole project is judged, and the report says why.
#[test]
fn diff_mode_judges_in_full_when_the_policy_changes_what_is_scanned() {
    let legacy = WEB_AND_STORAGE.replace("common: [storage]\n", "");
    let (_tmp, root) = web_and_storage(&legacy);
    assert_eq!(check(&root).outcome(), Outcome::Failed);
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);

    fs::write(config(&root), format!("{legacy}exclude: [docs]\n")).unwrap();
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    assert_eq!(
        report["violations"][0]["file"], "src/web/page.ts",
        "{report}"
    );
    let in_full = &report["diff"]["in_full"];
    assert_eq!(in_full.as_array().unwrap().len(), 1, "{report}");
    assert!(in_full[0].get("keys").is_none(), "{report}");
    assert!(
        in_full[0]["reason"]
            .as_str()
            .unwrap()
            .contains("changes what is scanned (`exclude`)"),
        "{report}"
    );
    // A verdict over the whole project is not narrowed.
    assert_eq!(report["narrowed"], serde_json::json!([]), "{report}");
    let said = text(&checked);
    assert!(
        said.contains("Checking the whole project, not only the lines changed since main"),
        "{said}"
    );
    assert!(said.contains("(`exclude`)"), "{said}");
}

/// A file outside the repository has no past: what changing it caused is
/// unknown, so the whole project is judged.
#[test]
fn diff_mode_judges_in_full_when_the_policy_extends_a_file_outside_the_repository() {
    let (tmp, root) = project(&[
        (
            "shared.yml",
            "components:\n  storage: { in: repo/src/storage }\n",
        ),
        (
            "repo/easement.yml",
            "extends: ../shared.yml\nversion: 1\ncomponents:\n  web: { in: src/web }\n  storage: { in: src/storage }\n",
        ),
        (
            "repo/src/web/page.ts",
            "import { save } from \"../storage/db\";\nexport const page = save;\n",
        ),
        ("repo/src/storage/db.ts", "export const save = 1;\n"),
    ]);
    let root = root.join("repo");
    fs::rename(tmp.path().join("tsconfig.json"), root.join("tsconfig.json")).unwrap();
    commit_all(&root);
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    assert_eq!(
        report["violations"][0]["file"], "src/web/page.ts",
        "{report}"
    );
    assert!(
        report["diff"]["in_full"][0]["reason"]
            .as_str()
            .unwrap()
            .ends_with(
                "shared.yml is outside the repository, so its content at the merge-base is unknown"
            ),
        "{report}"
    );
    let changed = report["diff"]["policy_changed"].as_array().unwrap();
    assert!(
        changed.len() == 1 && changed[0].as_str().unwrap().ends_with("shared.yml"),
        "{report}"
    );
}

/// The `manifests` rule reads a manifest, not a line of it: a dependency
/// removed from one fails the unchanged imports of it.
#[test]
fn diff_mode_judges_the_manifests_rule_in_full_when_a_manifest_changed() {
    // One dependency a line, so that removing one deletes a line and adds none.
    let manifest =
        |deps: &str| format!("{{\n  \"name\": \"app\",\n  \"dependencies\": {{\n{deps}  }}\n}}\n");
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\nmanifests: true\ncomponents:\n  web: { in: src/web }\nentry: [src/web/a.ts]\n",
        ),
        (
            "package.json",
            &manifest("    \"zod\": \"1\",\n    \"react\": \"1\"\n"),
        ),
        (".gitignore", "node_modules\n"),
        (
            "src/web/a.ts",
            "import { z } from \"zod\";\nimport react from \"react\";\nexport const a = [z, react];\n",
        ),
    ]);
    stub_packages(&root, &["zod", "react"]);
    commit_all(&root);
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);

    fs::write(
        root.join("package.json"),
        manifest("    \"react\": \"1\"\n"),
    )
    .unwrap();
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let found: Vec<(&str, u64, &str)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap(),
                v["line"].as_u64().unwrap(),
                v["rule"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(found, [("src/web/a.ts", 1, "undeclared")], "{report}");
    let keys: Vec<&serde_json::Value> = report["diff"]["in_full"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| &part["keys"])
        .collect();
    assert_eq!(
        keys,
        [
            &serde_json::json!(["manifests", "packages"]),
            &serde_json::json!(["unused"])
        ],
        "{report}"
    );
    // The rest is still judged by the change.
    assert_eq!(report["narrowed"][0]["kind"], "diff", "{report}");
    assert!(
        text(&checked).contains(
            "Judged in full: package.json changed, so the `manifests` and `packages` rules were judged in full",
        ),
        "{}",
        text(&checked)
    );
}

/// The `unused` files of a report, by path.
fn unused_files(report: &serde_json::Value) -> Vec<String> {
    by_rule(report)
        .into_iter()
        .filter(|(_, _, rule)| rule == "unused")
        .map(|(file, _, _)| file)
        .collect()
}

/// `unused` takes the files a framework a manifest lists loads as entries: a
/// manifest that drops the framework leaves them unreachable, although none of
/// their lines changed, so the rule is judged in full.
#[test]
fn diff_mode_judges_unused_in_full_when_a_manifest_drops_a_framework() {
    let (_tmp, root) = fixture("sveltekit");
    commit_all(&root);
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"web\",\n  \"private\": true,\n  \"type\": \"module\"\n}\n",
    )
    .unwrap();

    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let found = unused_files(&report);
    for loaded in [
        "src/routes/+page.ts",
        "src/hooks.server.ts",
        "src/params/integer.ts",
    ] {
        assert!(found.iter().any(|f| f == loaded), "{loaded}: {report}");
    }
    // Judged in full: what the whole project reports, the old `src/lib/x.ts` too.
    assert_eq!(found, unused_files(&json(&check(&root))), "{report}");
}

/// What a package runs or exports is an entry too: a manifest that drops its
/// `main`, `bin` and `exports` leaves their files unreachable, and `in_full` says why, with the
/// `manifests` and `packages` rules off. Other rules stay judged by the change.
#[test]
fn diff_mode_judges_unused_in_full_when_a_manifest_changes_what_its_package_runs() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: src/web }\n  storage: { in: src/storage }\nrules:\n  storage: { must_not_depend_on: [web] }\nentry: [src/main.ts]\n",
        ),
        (
            "package.json",
            "{\n  \"name\": \"m\",\n  \"main\": \"src/web/lib.ts\",\n  \"bin\": { \"m\": \"src/web/cli.ts\" },\n  \"exports\": { \"./x\": \"./src/web/x.ts\" }\n}\n",
        ),
        (
            "src/main.ts",
            "import { save } from \"./storage/db\";\nconsole.log(save);\n",
        ),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/web/cli.ts", "console.log(1);\n"),
        ("src/web/lib.ts", "export const lib = 1;\n"),
        ("src/web/x.ts", "export const x = 1;\n"),
        // An old violation of another rule, on a line the change leaves alone.
        (
            "src/storage/db.ts",
            "import { page } from \"../web/page\";\nexport const save = page;\n",
        ),
    ]);
    commit_all(&root);
    let old = json(&check(&root));
    assert_eq!(by_rule(&old).len(), 1, "{old}");
    assert_eq!(check_with(&root, diff_main).outcome(), Outcome::Passed);

    fs::write(root.join("package.json"), "{\n  \"name\": \"m\"\n}\n").unwrap();
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    assert_eq!(
        by_rule(&report),
        ["src/web/cli.ts", "src/web/lib.ts", "src/web/x.ts"].map(|file| (
            file.to_string(),
            1,
            "unused".to_string()
        )),
        "{report}"
    );
    assert_eq!(
        report["diff"]["in_full"],
        serde_json::json!([{
            "keys": ["unused"],
            "reason": "package.json changed, so the `unused` rule was judged in full: it takes entries from manifests, so an edit to one can leave a file unreachable without touching it",
        }]),
        "{report}"
    );
    assert!(
        text(&checked).contains(
            "Judged in full: package.json changed, so the `unused` rule was judged in full"
        ),
        "{}",
        text(&checked)
    );
}

/// No changed line points at a file left dead by removing its last import, and
/// no manifest changed: `--diff` does not report it, the full check does.
#[test]
fn diff_mode_misses_a_file_left_dead_by_removing_its_last_import() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\nentry: [src/main.ts]\n",
        ),
        (
            "src/main.ts",
            "import { a } from \"./app/a\";\nconsole.log(a);\n",
        ),
        ("src/app/a.ts", "export const a = 1;\n"),
    ]);
    commit_all(&root);
    fs::write(root.join("src/main.ts"), "console.log(1);\n").unwrap();

    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    assert_eq!(json(&checked)["diff"]["in_full"], serde_json::json!([]));
    assert_eq!(
        by_rule(&json(&check(&root))),
        [("src/app/a.ts".to_string(), 1, "unused".to_string())]
    );
}
