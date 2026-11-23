//! `review`: what a change to the policy since the merge-base with a base
//! revision permits, forbids and accepts over the current code, the files it
//! moves, opens or closes, and the gaps of both scans it rests on.

use std::fs;
use std::path::{Path, PathBuf};

use easement::commands::{self, Outcome};
use tempfile::TempDir;

use crate::common::{WEB_AND_STORAGE, commit_all, fixture, git, project, web_and_storage};
use crate::run::{cache, check, config, parse, pinned};

/// The review of the change to the policy of the project at `root` since
/// `main` as text, as `review --base main` prints it.
fn review_text(root: &Path) -> String {
    commands::review(&config(root), "main", &mut cache())
        .unwrap()
        .text()
}

/// The same review as data, as `review --base main --format json` prints it.
fn review_json(root: &Path) -> serde_json::Value {
    parse(
        &commands::review(&config(root), "main", &mut cache())
            .unwrap()
            .json(),
    )
}

/// Two components that do not import each other, committed on `main` and
/// checked out on `feature`.
fn web_and_unused_storage(policy: &str) -> (TempDir, PathBuf) {
    let (tmp, root) = project(&[
        ("easement.yml", policy),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/storage/db.ts", "export const save = 1;\n"),
        ("src/storage/internal/pool.ts", "export const pool = 1;\n"),
    ]);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    (tmp, root)
}

const UNUSED_STORAGE: &str = "version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
rules:
  web: { may_depend_on: [] }
";

/// A grant that no import uses yet produces no violation, yet the review
/// names it with its reason.
#[test]
fn review_names_a_permission_no_code_uses() {
    let (_tmp, root) = web_and_unused_storage(UNUSED_STORAGE);
    fs::write(
        config(&root),
        UNUSED_STORAGE.replace(
            "web: { may_depend_on: [] }",
            "web: { may_depend_on: [{ name: storage, because: pages save drafts }] }",
        ),
    )
    .unwrap();
    assert_eq!(check(&root).outcome(), Outcome::Passed);
    let said = review_text(&root);
    assert!(
        said.contains("web -> storage newly permitted (may_depend_on)"),
        "{said}"
    );
    assert!(said.contains("because: pages save drafts"), "{said}");
    let report = review_json(&root);
    assert_eq!(
        report["permissions"],
        serde_json::json!([{
            "from": "web",
            "to": "storage",
            "change": "permitted",
            "clause": "may_depend_on",
            "because": "pages save drafts",
        }]),
        "{report}"
    );

    // A plain item gives the reason of its rule.
    fs::write(
        config(&root),
        UNUSED_STORAGE.replace(
            "web: { may_depend_on: [] }",
            "web: { may_depend_on: [storage], because: pages render what storage holds }",
        ),
    )
    .unwrap();
    let said = review_text(&root);
    assert!(
        said.contains(
            "web -> storage newly permitted (may_depend_on)\n    because: pages render what storage holds\n",
        ),
        "{said}"
    );
}

/// What the change to the policy makes of the code as it is: violations it
/// introduces and those it removes, a rule moved to `warn` told apart.
#[test]
fn review_reports_the_policy_delta_on_the_current_code() {
    let legacy = WEB_AND_STORAGE.replace("common: [storage]\n", "");
    let (_tmp, root) = web_and_storage(WEB_AND_STORAGE);
    fs::write(
        config(&root),
        WEB_AND_STORAGE.replace(
            "web: { may_depend_on: [] }",
            "web: { must_not_depend_on: [{ name: storage, because: pages go through the api }] }",
        ),
    )
    .unwrap();
    let report = review_json(&root);
    let introduced: Vec<(&str, u64, &str)> = report["delta"]["introduced"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap(),
                v["line"].as_u64().unwrap(),
                v["because"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        introduced,
        [("src/web/page.ts", 1, "pages go through the api")],
        "{report}"
    );
    assert_eq!(
        report["delta"]["removed"],
        serde_json::json!([]),
        "{report}"
    );
    assert_eq!(report["rescanned"], serde_json::Value::Null, "{report}");
    let said = review_text(&root);
    assert!(
        said.contains("web -> storage newly rejected (must_not_depend_on)"),
        "{said}"
    );
    assert!(
        said.contains("Policy delta on the current code:\n  introduced (1):\n    src/web/page.ts:1: web -> storage is forbidden"),
        "{said}"
    );

    // Moved to `warn`, a violation fails no more: it leaves the delta as a warning.
    git(&root, &["checkout", "-q", "main"]);
    fs::write(config(&root), &legacy).unwrap();
    git(&root, &["commit", "-q", "-am", "legacy"]);
    git(&root, &["checkout", "-q", "-B", "feature"]);
    fs::write(config(&root), format!("{legacy}warn: [dependency]\n")).unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["delta"]["removed"],
        serde_json::json!([]),
        "{report}"
    );
    assert_eq!(
        report["delta"]["warned"][0]["file"], "src/web/page.ts",
        "{report}"
    );
    assert_eq!(
        report["debt"]["warn_added"],
        serde_json::json!(["dependency"]),
        "{report}"
    );
}

/// When the change alters what is scanned, the current code is scanned again
/// under the old policy, and the review says so.
#[test]
fn review_scans_again_when_the_policy_changes_what_is_scanned() {
    let legacy = WEB_AND_STORAGE.replace("common: [storage]\n", "");
    let (_tmp, root) = web_and_storage(&legacy);
    fs::write(config(&root), format!("{legacy}exclude: [src/web]\n")).unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["rescanned"],
        serde_json::json!(["exclude"]),
        "{report}"
    );
    assert_eq!(
        report["ownership"]["excluded"],
        serde_json::json!(["src/web/page.ts"]),
        "{report}"
    );
    assert_eq!(
        report["ownership"]["moved"],
        serde_json::json!([]),
        "{report}"
    );
    assert_eq!(
        report["delta"]["removed"][0]["file"], "src/web/page.ts",
        "{report}"
    );
    assert_eq!(
        report["debt"]["exclude_added"],
        serde_json::json!(["src/web"]),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains(
            "Scanned twice: the change alters what is scanned (`exclude`), so the current code was scanned again under the policy at the merge-base",
        ),
        "{said}"
    );
    assert!(
        said.contains("  newly excluded (1):\n    src/web/page.ts\n"),
        "{said}"
    );
}

/// Files whose component changes are listed with where they were and are.
#[test]
fn review_lists_files_that_change_component() {
    let (_tmp, root) = web_and_unused_storage(UNUSED_STORAGE);
    fs::write(
        config(&root),
        UNUSED_STORAGE
            .replace(
                "  storage: { in: src/storage }",
                "  storage: { in: src/storage/db.ts }\n  pool: { in: src/storage/internal }",
            )
            .replace("  web: { in: src/web }\n", "")
            .replace("rules:\n  web: { may_depend_on: [] }\n", ""),
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["ownership"]["moved"],
        serde_json::json!([
            { "file": "src/storage/internal/pool.ts", "from": "storage", "to": "pool" },
            { "file": "src/web/page.ts", "from": "web", "to": null },
        ]),
        "{report}"
    );
    assert_eq!(
        report["components"],
        serde_json::json!({ "added": ["pool"], "removed": ["web"] }),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains("  storage -> pool (1):\n    src/storage/internal/pool.ts\n"),
        "{said}"
    );
    assert!(
        said.contains("  web -> no component (1):\n    src/web/page.ts\n"),
        "{said}"
    );
    assert!(
        said.contains("Components added: pool\nComponents removed: web\n"),
        "{said}"
    );
}

/// What the old policy accepted is no gap of the delta, what it did not accept
/// is, and an entry taken back is listed under the debt.
#[test]
fn review_leaves_out_what_the_old_policy_accepted() {
    let (_tmp, root) = fixture("uncovered");
    let policy = fs::read_to_string(config(&root)).unwrap();
    fs::write(
        config(&root),
        format!(
            "{policy}accepted_gaps:\n  - {{ kind: uncovered, files: tools/gen.go, because: generated }}\n  - {{ kind: uncovered, files: tools/other.go, because: a helper }}\n"
        ),
    )
    .unwrap();
    commit_all(&root);
    fs::write(
        config(&root),
        format!(
            "{policy}accepted_gaps:\n  - {{ kind: uncovered, files: tools/**, because: build helpers }}\n"
        ),
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(report["evidence_gaps"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["debt"]["accepted_gaps_removed"],
        serde_json::json!([
            { "kind": "uncovered", "files": "tools/gen.go", "because": "generated" },
            { "kind": "uncovered", "files": "tools/other.go", "because": "a helper" },
        ]),
        "{report}"
    );

    // Taken back now, the file the old policy accepted is a gap again.
    fs::write(
        config(&root),
        format!(
            "{policy}accepted_gaps:\n  - {{ kind: uncovered, files: tools/gen.go, because: generated }}\n"
        ),
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["evidence_gaps"][0]["files"],
        serde_json::json!(["tools/other.go"]),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains("accepted_gaps removed: uncovered `tools/other.go` (because: a helper)"),
        "{said}"
    );
}

/// The delta rests on both scans: code either could not read is an evidence
/// gap, and code the new policy accepts unread is an accepted gap.
#[test]
fn review_reports_the_gaps_of_both_policies() {
    let (_tmp, root) = fixture("uncovered");
    commit_all(&root);
    let policy = fs::read_to_string(config(&root)).unwrap();
    // Excluded now, the files only the old scan read stay a gap of the delta.
    fs::write(config(&root), format!("{policy}exclude: [tools]\n")).unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["rescanned"],
        serde_json::json!(["exclude"]),
        "{report}"
    );
    let gaps = report["evidence_gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(
        gaps[0]["files"],
        serde_json::json!(["tools/gen.go", "tools/other.go"]),
        "{report}"
    );

    fs::write(
        config(&root),
        format!(
            "{policy}accepted_gaps: [{{ kind: uncovered, files: tools/gen.go, because: generated }}]\n"
        ),
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["accepted_gaps"][0]["files"],
        serde_json::json!(["tools/gen.go"]),
        "{report}"
    );
    // The old policy did not accept it, so the delta read nothing of it.
    assert_eq!(
        report["evidence_gaps"][0]["files"],
        serde_json::json!(["tools/gen.go", "tools/other.go"]),
        "{report}"
    );
    let said = review_text(&root);
    assert!(said.contains("Accepted gaps (`accepted_gaps`"), "{said}");
    assert!(
        said.contains("accepted_gaps added: uncovered `tools/gen.go` (because: generated)"),
        "{said}"
    );
    assert!(
        said.contains("Evidence gaps (the delta and the file lists say nothing about this code):"),
        "{said}"
    );
}

/// A policy input outside the repository has no past: the review cannot see
/// how it changed, and says so.
#[test]
fn review_names_a_policy_input_whose_past_is_unknown() {
    let (tmp, root) = project(&[
        (
            "shared.yml",
            "components:\n  storage: { in: repo/src/storage }\n",
        ),
        (
            "repo/easement.yml",
            "extends: ../shared.yml\nversion: 1\ncomponents:\n  web: { in: src/web }\n  storage: { in: src/storage }\n",
        ),
        ("repo/src/web/page.ts", "export const page = 1;\n"),
    ]);
    let root = root.join("repo");
    fs::rename(tmp.path().join("tsconfig.json"), root.join("tsconfig.json")).unwrap();
    commit_all(&root);
    let report = review_json(&root);
    let unknown = report["inputs"]["past_unknown"].as_array().unwrap();
    assert!(
        unknown.len() == 1 && unknown[0].as_str().unwrap().ends_with("shared.yml"),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains("shared.yml is outside the repository, so its content at the merge-base is unknown and how it changed is not reviewed"),
        "{said}"
    );
}

/// A wider surface is stated as the patterns it adds, with the files of
/// today that it opens.
#[test]
fn review_lists_the_files_a_wider_surface_opens() {
    let policy = UNUSED_STORAGE.replace(
        "storage: { in: src/storage }",
        "storage: { in: src/storage, public: src/storage/db.ts }",
    );
    let (_tmp, root) = web_and_unused_storage(&policy);
    fs::write(
        config(&root),
        policy.replace(
            "public: src/storage/db.ts",
            "public: [src/storage/db.ts, src/storage/internal/**]",
        ),
    )
    .unwrap();
    let said = review_text(&root);
    assert!(
        said.contains(
            "  storage.public: added `src/storage/internal/**`\n    opens 1 file(s):\n      src/storage/internal/pool.ts\n",
        ),
        "{said}"
    );
    let report = review_json(&root);
    assert_eq!(
        report["surfaces"],
        serde_json::json!([{
            "component": "storage",
            "key": "public",
            "before": ["src/storage/db.ts"],
            "after": ["src/storage/db.ts", "src/storage/internal/**"],
            "added": ["src/storage/internal/**"],
            "removed": [],
            "opens": ["src/storage/internal/pool.ts"],
            "closes": [],
        }]),
        "{report}"
    );
}

/// Naming a surface on an open component closes every file outside it to
/// all, and opens what it holds only to its importers.
#[test]
fn review_lists_the_files_a_named_surface_closes() {
    let (_tmp, root) = web_and_unused_storage(UNUSED_STORAGE);
    fs::write(
        config(&root),
        UNUSED_STORAGE.replace(
            "storage: { in: src/storage }",
            "storage: { in: src/storage, surfaces: { pool: { in: src/storage/internal, importers: [web] } } }",
        ),
    )
    .unwrap();
    let said = review_text(&root);
    assert!(
        said.contains(
            "  storage.surfaces: set to `pool: src/storage/internal (open to web)` (was unset: no named surface)\n    closes 2 file(s):\n      src/storage/db.ts\n      src/storage/internal/pool.ts\n",
        ),
        "{said}"
    );
    let report = review_json(&root);
    assert_eq!(
        report["surfaces"],
        serde_json::json!([{
            "component": "storage",
            "key": "surfaces",
            "before": null,
            "after": ["pool: src/storage/internal (open to web)"],
            "added": ["pool: src/storage/internal (open to web)"],
            "removed": [],
            "opens": [],
            "closes": ["src/storage/db.ts", "src/storage/internal/pool.ts"],
        }]),
        "{report}"
    );
}

/// A baseline of legalized `web -> storage` imports from the given files.
fn baseline_of(files: impl IntoIterator<Item = String>) -> String {
    let mut out = String::from("violations:\n");
    for file in files {
        out.push_str(&format!(
            "- rule: dependency\n  file: {file}\n  target: src/storage/db.ts\n  from: web\n  to: storage\n"
        ));
    }
    out
}

/// Debt accepted in the baseline is listed entry by entry, however much of it.
#[test]
fn review_lists_every_baseline_entry_added_or_removed() {
    let (_tmp, root) = project(&[
        ("easement.yml", UNUSED_STORAGE),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/storage/db.ts", "export const save = 1;\n"),
        (
            "easement.baseline.yml",
            &baseline_of(["src/web/old.ts".to_string()]),
        ),
    ]);
    commit_all(&root);
    let added: Vec<String> = (0..10).map(|i| format!("src/web/new{i}.ts")).collect();
    fs::write(
        root.join("easement.baseline.yml"),
        baseline_of(added.clone()),
    )
    .unwrap();

    let text = review_text(&root);
    assert!(text.contains("Baseline entries added (10):"), "{text}");
    for file in &added {
        assert!(
            text.contains(&format!(
                "{file}: web -> storage (src/storage/db.ts) [dependency]"
            )),
            "{text}"
        );
    }
    assert!(text.contains("Baseline entries removed (1):"), "{text}");
    let report = review_json(&root);
    let debt = &report["debt"];
    assert_eq!(
        debt["baseline_added"].as_array().unwrap().len(),
        10,
        "{report}"
    );
    assert_eq!(
        debt["baseline_removed"],
        serde_json::json!([{
            "rule": "dependency",
            "file": "src/web/old.ts",
            "target": "src/storage/db.ts",
            "from": "web",
            "to": "storage",
        }]),
        "{report}"
    );
}

/// Every constraint the change loosens or tightens is listed, a name no code
/// uses among them, each with its reason and where the policy writes it: a
/// removed one as the merge-base wrote it, in the file it extended then.
#[test]
fn review_lists_every_constraint_a_change_loosens_or_tightens() {
    let shared = "forbid_symbol:\n  - { name: tags.DeleteIn, because: tags are kept forever }\n";
    let policy = "extends: policy/shared.yml
version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
no_cycles: [web]
rules:
  web: { max_fan_out: 1, because: pages stay small }
  storage: { may_use_external: [react] }
";
    let (_tmp, root) = project(&[
        ("easement.yml", policy),
        ("policy/shared.yml", shared),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/storage/db.ts", "export const save = 1;\n"),
    ]);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("policy/shared.yml"), "exclude: []\n").unwrap();
    fs::write(
        config(&root),
        policy
            .replace("no_cycles: [web]", "no_cycles: true")
            .replace(
                "max_fan_out: 1,",
                "max_fan_out: 2, forbid_external: [lodash], may_use_external: [react],",
            )
            .replace("[react] }", "[react, lodash] }"),
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["constraints"],
        serde_json::json!([
            {
                "component": null,
                "key": "forbid_symbol",
                "item": "tags.DeleteIn",
                "edit": "removed",
                "change": "loosened",
                "because": "tags are kept forever",
                "provenance": { "field": "forbid_symbol[0]", "input": "policy/shared.yml" },
            },
            {
                "component": null,
                "key": "no_cycles",
                "item": "storage",
                "edit": "added",
                "change": "tightened",
                "provenance": { "field": "no_cycles", "input": "easement.yml" },
            },
            {
                "component": "storage",
                "key": "may_use_external",
                "item": "lodash",
                "edit": "added",
                "change": "loosened",
                "provenance": { "field": "rules.storage.may_use_external[1]", "input": "easement.yml" },
            },
            {
                "component": "web",
                "key": "forbid_external",
                "item": "lodash",
                "edit": "added",
                "change": "tightened",
                "because": "pages stay small",
                "provenance": { "field": "rules.web.forbid_external[0]", "input": "easement.yml" },
            },
            {
                "component": "web",
                "key": "may_use_external",
                "item": null,
                "before": null,
                "after": ["react"],
                "change": "tightened",
                "because": "pages stay small",
                "provenance": { "field": "rules.web.may_use_external", "input": "easement.yml" },
            },
            {
                "component": "web",
                "key": "max_fan_out",
                "item": null,
                "before": 1,
                "after": 2,
                "change": "loosened",
                "because": "pages stay small",
                "provenance": { "field": "rules.web.max_fan_out", "input": "easement.yml" },
            },
        ]),
        "{report}"
    );
    let said = review_text(&root);
    for line in [
        "\nConstraints:\n  forbid_symbol: removed `tags.DeleteIn` (loosened)\n    because: tags are kept forever\n",
        "  web.max_fan_out: 2 (was 1) (loosened)\n",
        "  storage.may_use_external: added `lodash` (loosened)\n",
        "3 constraint(s) loosened, 3 tightened",
    ] {
        assert!(said.contains(line), "{line}\n---\n{said}");
    }
}

/// Three components, none of which imports another.
const APP_WEB_WIRING: &str = "version: 1
components:
  app: { in: src/app }
  web: { in: src/web }
  wiring: { in: src/wiring }
";

const SETTINGS: &str = "settings come through the getenv Run is given";

/// The review of moving `os.Getenv` from the rules of `app` and `web` into a
/// top-level item that allows `allow`, as data and as text.
fn review_of_moving_getenv(allow: &str) -> (serde_json::Value, String) {
    let own = format!("{{ forbid_symbol: [{{ name: os.Getenv, because: {SETTINGS} }}] }}");
    let in_rules = format!("{APP_WEB_WIRING}rules:\n  app: {own}\n  web: {own}\n");
    let (_tmp, root) = project(&[
        ("easement.yml", in_rules.as_str()),
        ("src/app/run.ts", "export const run = 1;\n"),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/wiring/main.ts", "export const main = 1;\n"),
    ]);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(
        config(&root),
        format!(
            "{APP_WEB_WIRING}forbid_symbol:\n  - {{ name: os.Getenv, allow: [{allow}], because: {SETTINGS} }}\n"
        ),
    )
    .unwrap();
    (
        review_json(&root)["constraints"].clone(),
        review_text(&root),
    )
}

/// The top-level item forbids the name to every component and to the files of
/// no component, so it tightens; `app` and `web` stay forbidden it, so the
/// items their rules drop loosen nothing.
#[test]
fn review_reads_a_forbidden_name_moved_to_the_top_level_as_tightened_alone() {
    let (constraints, said) = review_of_moving_getenv("wiring");
    assert_eq!(
        constraints,
        serde_json::json!([{
            "component": null,
            "key": "forbid_symbol",
            "item": "os.Getenv",
            "edit": "added",
            "change": "tightened",
            "because": SETTINGS,
            "provenance": { "field": "forbid_symbol[0]", "input": "easement.yml" },
        }]),
    );
    assert!(
        said.contains("0 constraint(s) loosened, 1 tightened"),
        "{said}"
    );
}

/// An `allow` that lets in a component its own rule used to forbid the name
/// loosens, and names that component.
#[test]
fn review_names_the_component_a_moved_forbidden_name_lets_in() {
    let (constraints, said) = review_of_moving_getenv("wiring, web");
    // The section alone: the rest names the merge-base and the cache.
    let section = said
        .split("\n\n")
        .find(|part| part.starts_with("Constraints:"))
        .unwrap_or_default();
    pinned(|| {
        insta::assert_json_snapshot!("review_allow_lets_in_json", constraints);
        insta::assert_snapshot!("review_allow_lets_in_text", section);
    });
}

/// A path added to `allow_in` loosens and one dropped from it tightens, each
/// naming its patterns.
#[test]
fn review_names_the_paths_a_forbidden_name_is_let_into_or_out_of() {
    let item = |paths: &str| {
        format!(
            "{APP_WEB_WIRING}forbid_symbol:\n  - {{ name: os.Getenv, allow: [wiring], allow_in: [{paths}], because: {SETTINGS} }}\n"
        )
    };
    let (_tmp, root) = project(&[
        ("easement.yml", item("src/app").as_str()),
        ("src/app/run.ts", "export const run = 1;\n"),
        ("src/web/admin/page.ts", "export const page = 1;\n"),
        ("src/wiring/main.ts", "export const main = 1;\n"),
    ]);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(config(&root), item("src/web/admin")).unwrap();
    let said = review_text(&root);
    let section = said
        .split("\n\n")
        .find(|part| part.starts_with("Constraints:"))
        .unwrap_or_default();
    assert!(
        said.contains("1 constraint(s) loosened, 1 tightened"),
        "{said}"
    );
    pinned(|| {
        insta::assert_json_snapshot!(
            "review_allow_in_paths_json",
            review_json(&root)["constraints"]
        );
        insta::assert_snapshot!("review_allow_in_paths_text", section);
    });
}

/// The policy of `web` importing `storage`, with the diagram that draws it.
const DRAWN_POLICY: &str = "version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
rules:
  web: { may_depend_on: [storage] }
diagram: docs/arch.md
";

/// `web` imports `storage`, and the diagram on `main` has no arrow for it.
fn undrawn_dependency() -> (TempDir, PathBuf) {
    let (tmp, root) = project(&[
        ("easement.yml", DRAWN_POLICY),
        (
            "docs/arch.md",
            "```mermaid\ngraph TD\n  web\n  storage\n```\n",
        ),
        (
            "src/web/page.ts",
            "import { save } from \"../storage/db\";\nexport const page = save;\n",
        ),
        ("src/storage/db.ts", "export const save = 1;\n"),
    ]);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    (tmp, root)
}

/// An arrow drawn loosens the diagram; `target_only` changes which files are
/// judged, so it is debt, not a constraint.
#[test]
fn review_lists_a_diagram_arrow_added_and_target_only_as_debt() {
    let (_tmp, root) = undrawn_dependency();
    fs::write(
        root.join("docs/arch.md"),
        "```mermaid\ngraph TD\n  web --> storage\n```\n",
    )
    .unwrap();
    fs::write(
        config(&root),
        format!("{DRAWN_POLICY}target_only: ['**/*.gen.ts']\n"),
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(
        report["constraints"],
        serde_json::json!([{
            "component": null,
            "key": "diagram",
            "item": "web -> storage",
            "edit": "added",
            "change": "loosened",
            "provenance": { "field": "diagram", "input": "docs/arch.md" },
        }]),
        "{report}"
    );
    assert_eq!(
        report["debt"]["target_only_added"],
        serde_json::json!(["**/*.gen.ts"]),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains("  diagram: added arrow `web -> storage` (loosened)\n"),
        "{said}"
    );
    assert!(
        said.contains("  target_only added: `**/*.gen.ts`\n"),
        "{said}"
    );
}

/// A diagram the policy cannot read is named, with no arrow guessed, and the
/// delta leaves its rule out rather than call its violations removed.
#[test]
fn review_says_when_a_diagram_cannot_be_read() {
    let (_tmp, root) = undrawn_dependency();
    fs::write(root.join("docs/arch.md"), "no graph here\n").unwrap();
    let report = review_json(&root);
    assert_eq!(report["constraints"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["delta"]["removed"],
        serde_json::json!([]),
        "{report}"
    );
    let unread = &report["unread_diagrams"];
    assert_eq!(unread[0]["policy"], "head", "{report}");
    assert!(
        unread[0]["reason"].as_str().unwrap().contains("no Mermaid"),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains("Not compared: the diagram of the policy in the working tree cannot be read"),
        "{said}"
    );
    assert!(said.contains("  no constraint changed\n"), "{said}");
}

/// A diagram the merge-base could not read is named as that policy's, and
/// what the readable one now says of the code is not called introduced.
#[test]
fn review_says_when_the_diagram_at_the_merge_base_cannot_be_read() {
    let (_tmp, root) = project(&[
        ("easement.yml", DRAWN_POLICY),
        ("docs/arch.md", "no graph yet\n"),
        (
            "src/web/page.ts",
            "import { save } from \"../storage/db\";\nexport const page = save;\n",
        ),
        ("src/storage/db.ts", "export const save = 1;\n"),
    ]);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    // Readable now, and without the arrow: `check` fails it as `diagram`.
    fs::write(
        root.join("docs/arch.md"),
        "```mermaid\ngraph TD\n  web\n  storage\n```\n",
    )
    .unwrap();
    let report = review_json(&root);
    assert_eq!(report["unread_diagrams"][0]["policy"], "base", "{report}");
    assert_eq!(report["constraints"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["delta"]["introduced"],
        serde_json::json!([]),
        "{report}"
    );
    let said = review_text(&root);
    assert!(
        said.contains("Not compared: the diagram of the policy at the merge-base cannot be read"),
        "{said}"
    );
}
