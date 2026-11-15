//! Globs over component names (`'ee-*'`) and named sets (`'@drivers'`) in the
//! lists of components and the keys of `rules`: a policy that writes them
//! judges as its written-out form does; one that covers nothing fails as
//! `unmatched`; `explain`, `why`, `review` and `suggest --patch` name the glob
//! or the set a component comes through.

use std::fs;
use std::path::{Path, PathBuf};

use easement::{Cache, CacheUse, commands};
use serde_json::json;
use tempfile::TempDir;

use crate::common::{commit_all, git, project};
use crate::run::{cache, cache_in, check, config, json, pinned};

/// The policy with globs and sets.
const SPELLED: &str = "version: 1
sets:
  drivers: [api-site, api-external, runner]
components:
  api-site: { in: src/api/site }
  api-external: { in: src/api/external }
  runner: { in: src/runner }
  usecase: { in: src/usecase, allow_importers: ['@drivers', wiring] }
  wiring: { in: src/wiring }
  ee-licensekey: { in: src/ee/licensekey, allow_importers: [wiring, 'ee-*'] }
  ee-sso: { in: src/ee/sso }
layers: [wiring, ['@drivers'], usecase]
rules:
  'api-*':
    may_depend_on: [usecase, ee-licensekey]
    because: drivers call use cases
  runner: { may_depend_on: [usecase] }
  'ee-*': { may_depend_on: ['ee-*'] }
  usecase: { must_not_depend_on: [{ name: '@drivers', because: use cases know no driver }] }
";

/// The same policy written out.
const WRITTEN_OUT: &str = "version: 1
components:
  api-site: { in: src/api/site }
  api-external: { in: src/api/external }
  runner: { in: src/runner }
  usecase: { in: src/usecase, allow_importers: [api-site, api-external, runner, wiring] }
  wiring: { in: src/wiring }
  ee-licensekey: { in: src/ee/licensekey, allow_importers: [wiring, ee-licensekey, ee-sso] }
  ee-sso: { in: src/ee/sso }
layers: [wiring, [api-site, api-external, runner], usecase]
rules:
  api-site:
    may_depend_on: [usecase, ee-licensekey]
    because: drivers call use cases
  api-external:
    may_depend_on: [usecase, ee-licensekey]
    because: drivers call use cases
  runner: { may_depend_on: [usecase] }
  ee-licensekey: { may_depend_on: [ee-licensekey, ee-sso] }
  ee-sso: { may_depend_on: [ee-licensekey, ee-sso] }
  usecase:
    must_not_depend_on:
      - { name: api-site, because: use cases know no driver }
      - { name: api-external, because: use cases know no driver }
      - { name: runner, because: use cases know no driver }
";

/// A project of drivers, use cases and enterprise components under `policy`:
/// some of its imports the policy allows, some it rejects for each reason.
fn drivers(policy: &str) -> (TempDir, PathBuf) {
    project(&[
        ("easement.yml", policy),
        (
            "src/api/site/page.ts",
            "import { run } from \"../../usecase/case\";\nimport { key } from \"../../ee/licensekey/key\";\nexport const page = [run, key];\n",
        ),
        (
            "src/api/external/hook.ts",
            "import { job } from \"../../runner/run\";\nexport const hook = job;\n",
        ),
        (
            "src/runner/run.ts",
            "import { run } from \"../usecase/case\";\nexport const job = run;\n",
        ),
        (
            "src/usecase/case.ts",
            "import { job } from \"../runner/run\";\nexport const run = () => job;\n",
        ),
        (
            "src/wiring/main.ts",
            "import { run } from \"../usecase/case\";\nimport { key } from \"../ee/licensekey/key\";\nexport const main = [run, key];\n",
        ),
        ("src/ee/licensekey/key.ts", "export const key = 1;\n"),
        (
            "src/ee/sso/sso.ts",
            "import { key } from \"../licensekey/key\";\nexport const sso = key;\n",
        ),
    ])
}

/// The violations of a check of `root`, without where the policy writes each
/// clause: that is how it is spelled, not what it decides.
fn verdicts(root: &Path) -> Vec<serde_json::Value> {
    let mut found = json(&check(root))["violations"].as_array().unwrap().clone();
    for violation in &mut found {
        violation.as_object_mut().unwrap().remove("provenance");
    }
    found
}

#[test]
fn a_policy_of_globs_and_sets_judges_as_its_written_out_form() {
    let (_tmp, root) = drivers(SPELLED);
    let spelled = verdicts(&root);
    fs::write(config(&root), WRITTEN_OUT).unwrap();
    let written_out = verdicts(&root);
    assert_eq!(spelled, written_out);
    // Each kind of decision is there: a reason, an importer list and a prohibition.
    let rules: Vec<(&str, &str)> = spelled
        .iter()
        .map(|v| (v["file"].as_str().unwrap(), v["to"].as_str().unwrap()))
        .collect();
    assert_eq!(
        rules,
        [
            ("src/api/external/hook.ts", "runner"),
            ("src/api/site/page.ts", "ee-licensekey"),
            ("src/usecase/case.ts", "runner"),
            ("src/wiring/main.ts", "ee-licensekey"),
        ],
        "{spelled:#?}"
    );
}

#[test]
fn a_glob_or_a_set_that_covers_nothing_fails_as_unmatched_at_its_line() {
    let policy = SPELLED
        .replace(
            "  drivers: [api-site, api-external, runner]\n",
            "  drivers: [api-site, api-external, runner]\n  idle: [runner]\n  gone: ['cli-*']\n",
        )
        .replace(
            "  runner: { may_depend_on: [usecase] }\n",
            "  runner: { may_depend_on: [usecase, '@gone'] }\n  'job-*': { may_depend_on: [usecase] }\n  wiring: { may_depend_on: ['web-*'] }\n",
        )
        .replace(
            "    may_depend_on: [usecase, ee-licensekey]\n",
            "    may_depend_on: [usecase, ee-licensekey, 'zz-*']\n",
        );
    let (_tmp, root) = drivers(&policy);
    let report = json(&check(&root));
    let unmatched: Vec<(u64, &str, &str)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["rule"] == "unmatched")
        .map(|v| {
            (
                v["line"].as_u64().unwrap(),
                v["target"].as_str().unwrap(),
                v["message"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        unmatched,
        [
            (
                4,
                "idle",
                "the set `idle` is named by no list or rule key, so it changes nothing; use `@idle` or drop the set"
            ),
            (
                5,
                "cli-*",
                "`cli-*` in sets.gone matches no component; fix the glob or drop it"
            ),
            (
                17,
                "zz-*",
                "`zz-*` in rules.'api-*'.may_depend_on matches no component; fix the glob or drop it"
            ),
            (
                20,
                "job-*",
                "the rule key `job-*` matches no component, so its rule judges nothing; fix the glob or drop the rule"
            ),
            (
                21,
                "web-*",
                "`web-*` in rules.wiring.may_depend_on matches no component; fix the glob or drop it"
            ),
        ],
        "{report:#}"
    );
}

/// A cache of its own that indexes anew, so that what an answer says of the
/// cache does not depend on which test indexed the same files first.
fn fresh() -> (TempDir, Cache) {
    let dir = TempDir::new().unwrap();
    let cache = cache_in(dir.path(), CacheUse::Off);
    (dir, cache)
}

/// What `explain` says of `path` in the project at `root`, as data.
fn explain_json(root: &Path, path: &str) -> serde_json::Value {
    serde_json::to_value(commands::explain(&config(root), &root.join(path)).unwrap()).unwrap()
}

#[test]
fn explain_and_why_name_the_glob_or_the_set_a_clause_comes_through() {
    let (_tmp, root) = drivers(SPELLED);
    let explained = explain_json(&root, "src/api/site/page.ts");
    assert_eq!(
        explained["rule"]["provenance"],
        json!({ "field": "rules.'api-*'", "input": "easement.yml", "through": ["api-*"] })
    );
    let usecase = explained["grants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["to"] == "usecase")
        .unwrap();
    assert_eq!(
        usecase["provenance"],
        json!({
            "field": "rules.'api-*'.may_depend_on[0]",
            "input": "easement.yml",
            "through": ["api-*"]
        })
    );
    let explained = explain_json(&root, "src/usecase/case.ts");
    let runner = explained["prohibitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["to"] == "runner")
        .unwrap();
    assert_eq!(
        runner["provenance"],
        json!({
            "field": "rules.usecase.must_not_depend_on[0]",
            "input": "easement.yml",
            "through": ["@drivers"]
        })
    );
    let (_dir, mut fresh) = fresh();
    let why = commands::why(&config(&root), "usecase", "runner", &mut fresh).unwrap();
    assert_eq!(
        serde_json::to_value(&why).unwrap()["decision"]["provenance"]["through"],
        json!(["@drivers"])
    );
    pinned(|| {
        insta::assert_snapshot!(
            "explain_through_text",
            commands::explain(&config(&root), &root.join("src/api/site/page.ts"))
                .unwrap()
                .text()
                .unwrap()
        );
        insta::assert_snapshot!("why_through_text", why.text());
    });
}

/// A component the head policy adds that a glob of the merge-base policy covers:
/// the review names the glob it comes through, where it now may import and where
/// its rule comes from.
#[test]
fn review_names_the_glob_a_new_component_comes_through() {
    let (_tmp, root) = drivers(SPELLED);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(
        config(&root),
        SPELLED.replace(
            "  ee-sso: { in: src/ee/sso }\n",
            "  ee-sso: { in: src/ee/sso }\n  ee-audit: { in: src/ee/audit }\n",
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("src/ee/audit")).unwrap();
    fs::write(
        root.join("src/ee/audit/log.ts"),
        "import { key } from \"../licensekey/key\";\nexport const log = key;\n",
    )
    .unwrap();
    let review = commands::review(&config(&root), "main", &mut cache()).unwrap();
    let report = crate::run::parse(&review.json());
    let importers = report["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["component"] == "ee-licensekey")
        .unwrap();
    assert_eq!(importers["key"], "allow_importers");
    assert_eq!(importers["added"], json!(["ee-audit"]));
    assert_eq!(importers["through"], json!({ "ee-audit": "ee-*" }));
    let permitted = report["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["from"] == "ee-audit" && p["to"] == "ee-licensekey")
        .unwrap();
    assert_eq!(permitted["through"], json!(["ee-*"]), "{report:#}");
    let said = review.text();
    assert!(
        said.contains("  ee-licensekey.allow_importers: added `ee-audit` (through `ee-*`)\n"),
        "{said}"
    );
    assert!(
        said.contains(
            "  ee-audit -> ee-licensekey newly permitted (may_depend_on through `ee-*`)\n"
        ),
        "{said}"
    );
}

/// `suggest --patch` edits what names a component and leaves a glob or a set
/// alone, naming it once with the components and allowances it holds.
#[test]
fn suggest_patch_leaves_globs_and_sets_to_remove_by_hand() {
    let policy = SPELLED
        .replace(
            "  runner: { may_depend_on: [usecase] }\n",
            "  runner: { may_depend_on: [usecase, wiring] }\n  wiring: { may_depend_on: ['@drivers', usecase] }\n",
        )
        .replace(
            "  'ee-*': { may_depend_on: ['ee-*'] }\n",
            "  'ee-*': { may_depend_on: [ee-licensekey] }\n",
        );
    let (_tmp, root) = drivers(&policy);
    let (_dir, mut fresh) = fresh();
    let patched = commands::suggest_patch(&config(&root), &mut fresh).unwrap();
    assert!(
        patched
            .diff
            .contains("-  runner: { may_depend_on: [usecase, wiring] }\n+  runner: { may_depend_on: [usecase] }\n"),
        "{}",
        patched.diff
    );
    assert_eq!(patched.diff.matches("\n-  ").count(), 1, "{}", patched.diff);
    assert_eq!(
        patched.notes,
        [
            "rules.'api-*'.may_depend_on: no import of api-external uses ee-licensekey, usecase, but the patch does not edit what comes through `api-*`; remove it by hand if no component it covers needs it",
            "rules.'ee-*'.may_depend_on: no import of ee-licensekey uses ee-licensekey, but the patch does not edit what comes through `ee-*`; remove it by hand if no component it covers needs it",
            "rules.wiring.may_depend_on: no import of wiring uses api-external, api-site, runner, but the patch does not edit what comes through `@drivers`; remove it by hand if no component it covers needs it",
        ]
    );
}

/// Writing a policy out as globs and sets changes who is allowed nothing, so a
/// review of that change lists no permission, surface or constraint: it compares
/// the expanded lists, whatever their order.
#[test]
fn review_of_a_policy_respelled_as_globs_and_sets_lists_no_change() {
    let surface = |allowed: &str, importers: &str| {
        format!(
            "allow_importers: {allowed}, surfaces: {{ api: {{ in: src/usecase/case.ts, importers: {importers} }} }}"
        )
    };
    let written_out = WRITTEN_OUT
        .replace(
            "allow_importers: [wiring, ee-licensekey, ee-sso]",
            "allow_importers: [ee-sso, wiring, ee-licensekey]",
        )
        .replace(
            "allow_importers: [api-site, api-external, runner, wiring]",
            &surface(
                "[api-site, api-external, runner, wiring]",
                "[runner, api-site, api-external]",
            ),
        );
    let spelled = SPELLED.replace(
        "allow_importers: ['@drivers', wiring]",
        &surface("['@drivers', wiring]", "['@drivers']"),
    );
    let (_tmp, root) = drivers(&written_out);
    commit_all(&root);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    fs::write(config(&root), spelled).unwrap();
    let review = commands::review(&config(&root), "main", &mut cache()).unwrap();
    let report = crate::run::parse(&review.json());
    for section in ["permissions", "surfaces", "constraints"] {
        assert_eq!(report[section], json!([]), "{section}: {report:#}");
    }
}
