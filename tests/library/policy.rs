//! What the policy declares beyond who may depend on whom: the names of types
//! (`defines`), the diagram, the externals and manifests, layers, forbidden
//! externals and symbols, metrics and `strict`, and the suppressions of them.

use std::fs;

use easement::commands::{self, Outcome};

use crate::common::{fixture, project, stub_packages};
use crate::run::{
    cache, check, config, error, failed_json, gap_messages, json, report_of, text, try_check_with,
};

#[test]
fn a_type_named_for_another_component_is_a_naming_violation() {
    let policy = "version: 1\ncomponents:\n  web: { in: src/web }\n  repository: { in: src/repository, defines: ['*Repository'], because: storage goes through repositories }\n";
    let (_tmp, root) = project(&[
        ("easement.yml", policy),
        (
            "src/repository/orders.ts",
            "export class OrderRepository {}\n",
        ),
        (
            "src/web/page.ts",
            "export const page = 1;\n\nexport class UserRepository {}\n",
        ),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains(
            "src/web/page.ts:3: UserRepository is defined in web, but types named like *Repository belong in repository [naming]",
        ),
        "{said}"
    );
    assert!(
        said.contains("because: storage goes through repositories"),
        "{said}"
    );
    assert!(!said.contains("orders.ts"), "{said}");
    let (_tmp, root) = project(&[
        ("go.mod", "module ex.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: web }\n  repository: { in: repository, defines: ['*Repository'] }\n",
        ),
        (
            "repository/r.go",
            "package repository\n\ntype OrderRepository struct{}\n",
        ),
        ("web/h.go", "package web\n\ntype UserRepository struct{}\n"),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains("web/h.go:3: UserRepository is defined in web"),
        "{said}"
    );
    // Kotlin and Swift types are not seen: a clean verdict says so.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: src/web }\n  repository: { in: src/repository, defines: ['*Repository'] }\n",
        ),
        ("src/web/A.kt", "package web\nclass UserRepository\n"),
        ("src/repository/B.kt", "package repository\nclass B\n"),
    ]);
    let said = report_of(&root, Outcome::Passed);
    assert!(
        said.contains("`defines` cannot see the types of .kt files"),
        "{said}"
    );
}

#[test]
fn a_dependency_the_diagram_does_not_draw_fails() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ndiagram: ARCHITECTURE.md\ncomponents:\n  web: { in: src/web }\n  service: { in: src/service }\n  repository: { in: src/repository }\nrules:\n  web: { may_depend_on: [service, repository] }\n  service: { may_depend_on: [repository] }\n",
        ),
        (
            "ARCHITECTURE.md",
            "# Architecture\n\n```mermaid\nflowchart LR\n  ui[web] --> service --> repository\n  service --> db[(Postgres)]\n```\n",
        ),
        (
            "src/web/Page.kt",
            "package web\nimport service.S\nimport repository.R\nclass Page\n",
        ),
        ("src/service/S.kt", "package service\nclass S\n"),
        ("src/repository/R.kt", "package repository\nclass R\n"),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains(
            "src/web/Page.kt:3: web -> repository is not in the diagram ARCHITECTURE.md (1 dependency(ies)); draw the arrow, or drop the dependency [diagram]",
        ),
        "{said}"
    );
    // `service -> repository` is drawn, but no import stands for it.
    let found = commands::suggest(&config(&root), &mut cache()).unwrap();
    let said = found.text();
    assert!(
        said.contains(
            "Arrows of the diagram that no dependency stands for (drop them, or the dependency is still to come):\n  service -> repository",
        ),
        "{said}"
    );
    fs::write(root.join("ARCHITECTURE.md"), "no graph\n").unwrap();
    let said = error(try_check_with(&root, |_| {}));
    assert!(said.contains("no Mermaid `graph` or `flowchart`"), "{said}");
}

#[test]
fn an_external_module_the_manifest_does_not_declare_fails() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\nmanifests: true\ncomponents:\n  web: { in: src/web }\n",
        ),
        (
            "package.json",
            "{\"name\": \"app\", \"dependencies\": {\"react\": \"1\"}}\n",
        ),
        (
            "src/web/a.ts",
            "import react from \"react\";\nimport lodash from \"lodash/fp\";\nexport const a = [react, lodash];\n",
        ),
    ]);
    stub_packages(&root, &["react", "lodash/fp"]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains(
            "src/web/a.ts:2: lodash is not declared in package.json; add it to the dependencies, or drop the dependency [undeclared]",
        ),
        "{said}"
    );
    assert!(!said.contains("src/web/a.ts:1:"), "{said}");
}

#[test]
fn a_package_whose_types_sit_under_a_marker_package_json_is_judged_by_its_name() {
    // `{"type": "module"}` in a subdirectory of a package (zod, hono, @simplewebauthn/browser)
    // hid the package from scip-typescript before 0.6.1. Now the index names it, and the
    // directory is part of the module, as `dist/` is for any package.
    let marker = "{\"type\": \"module\"}\n";
    let stub = "declare const stub: any;\nexport default stub;\n";
    let nested = |name: &'static str| {
        [
            (
                format!("node_modules/{name}/package.json"),
                format!(
                    "{{\"name\": \"{name}\", \"version\": \"1.0.0\", \"types\": \"esm/index.d.ts\"}}\n"
                ),
            ),
            (
                format!("node_modules/{name}/esm/package.json"),
                marker.into(),
            ),
            (format!("node_modules/{name}/esm/index.d.ts"), stub.into()),
        ]
    };
    let mut files: Vec<(String, String)> = vec![
        (
            "easement.yml".into(),
            "version: 1\nmanifests: true\ncomponents:\n  web: { in: src/web }\nrules:\n  web: { forbid_external: [nested-ok] }\n".into(),
        ),
        (
            "package.json".into(),
            "{\"name\": \"app\", \"dependencies\": {\"zodlike\": \"1\", \"nested-ok\": \"1\"}}\n"
                .into(),
        ),
        (
            "node_modules/zodlike/package.json".into(),
            "{\"name\": \"zodlike\", \"version\": \"1.0.0\", \"types\": \"index.d.ts\"}\n".into(),
        ),
        (
            "node_modules/zodlike/index.d.ts".into(),
            "export * as z from \"./v4/external\";\n".into(),
        ),
        ("node_modules/zodlike/v4/package.json".into(), marker.into()),
        (
            "node_modules/zodlike/v4/external.d.ts".into(),
            "export declare function string(): string;\n".into(),
        ),
        (
            "src/web/helper.ts".into(),
            "export const helper = 1;\n".into(),
        ),
        (
            "src/web/a.ts".into(),
            "import { helper } from \"./helper\";\nimport { z } from \"zodlike\";\nimport a from \"nested-a\";\nimport b from \"nested-b\";\nimport ok from \"nested-ok\";\nexport const all = [helper, z.string(), a, b, ok];\n".into(),
        ),
    ];
    for name in ["nested-a", "nested-b", "nested-ok"] {
        files.extend(nested(name));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let (_tmp, root) = project(&files);
    let report = failed_json(&root);
    let mut found: Vec<(String, u64, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["rule"].as_str().unwrap().into(),
                v["line"].as_u64().unwrap(),
                v["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("external".to_string(), 5, "nested-ok/esm".to_string()),
            ("undeclared".to_string(), 3, "nested-a".to_string()),
            ("undeclared".to_string(), 4, "nested-b".to_string()),
        ]
    );
}

#[test]
fn an_alias_into_a_project_with_a_nameless_package_json_is_no_package() {
    // Without `name`, the project's own files are unnamed too; `@/gen/api` resolves
    // through `paths` to an ignored file, which the index defines and the walk drops.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\nmanifests: true\ncomponents:\n  app: { in: src }\n",
        ),
        (
            "package.json",
            "{\"private\": true, \"dependencies\": {}}\n",
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler","baseUrl":".","paths":{"@/*":["src/*"]}},"include":["src"]}"#,
        ),
        (".gitignore", "src/gen/\n"),
        ("src/gen/api.ts", "export const api = 1;\n"),
        (
            "src/a.ts",
            "import { api } from \"@/gen/api\";\nexport const all = [api];\n",
        ),
    ]);
    let said = report_of(&root, Outcome::Passed);
    assert!(!said.contains("[undeclared]"), "{said}");
}

#[test]
fn forbid_symbol_judges_a_name_of_a_package_under_a_marker_package_json() {
    // scip-typescript before 0.6.1 left such a name unnamed, and easement said it could not
    // judge it (`a_name_of_an_unnamed_package_...` in src/scip/mod.rs); now the index names it.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: src/web }\n  api: { in: src/api }\nrules:\n  web: { forbid_symbol: [nested.Thing] }\n",
        ),
        (
            "package.json",
            "{\"name\": \"app\", \"dependencies\": {\"nested\": \"1\"}}\n",
        ),
        (
            "node_modules/nested/package.json",
            "{\"name\": \"nested\", \"version\": \"1.0.0\", \"types\": \"esm/index.d.ts\"}\n",
        ),
        (
            "node_modules/nested/esm/package.json",
            "{\"type\": \"module\"}\n",
        ),
        (
            "node_modules/nested/esm/index.d.ts",
            "export declare class Thing {}\n",
        ),
        (
            "src/web/a.ts",
            "import { Thing } from \"nested\";\nexport const t = new Thing();\n",
        ),
        // No rule on names covers `api`, so nothing was left unjudged there.
        (
            "src/api/b.ts",
            "import { Thing } from \"nested\";\nexport const t = new Thing();\n",
        ),
    ]);
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let files: Vec<&str> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["file"].as_str().unwrap())
        .collect();
    // `api` has no rule on names; `web` is judged, and nothing is left unjudged.
    assert!(
        !files.is_empty() && files.iter().all(|f| *f == "src/web/a.ts"),
        "{files:?}"
    );
    assert!(gap_messages(&report, "symbols").is_empty(), "{report}");
    let said = text(&checked);
    assert!(said.contains("web must not use nested.Thing"), "{said}");
    assert!(!said.contains("cannot tell the npm package"), "{said}");
}

#[test]
fn layers_forbidden_externals_metrics_and_strict() {
    let (_tmp, root) = fixture("policy");
    let report = failed_json(&root);
    let found: Vec<(String, String, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["rule"].as_str().unwrap().into(),
                v["file"].as_str().unwrap().into(),
                v["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    let has = |rule: &str, file: &str, target: &str| {
        assert!(
            found.contains(&(rule.into(), file.into(), target.into())),
            "missing {rule} {file} {target} in {found:?}"
        );
    };
    has("dependency", "src/web/page.ts", "src/data/db.ts");
    has("dependency", "src/core/run.ts", "src/web/page.ts");
    has("external", "src/web/page.ts", "moment/locale/ru");
    has("external", "src/core/run.ts", "axios");
    has("metric", "src/core/run.ts", "max_fan_out");
    has("orphan", "src/stray.ts", "src/stray.ts");
    assert_eq!(found.len(), 6, "{found:?}");
    // Layers allow web -> core, core -> data, and common allows shared.
    assert!(!found.iter().any(|(_, _, t)| t == "src/core/run.ts"));
    assert!(!found.iter().any(|(_, _, t)| t == "src/shared/fmt.ts"));
    assert!(!found.iter().any(|(_, _, t)| t == "lodash/fp"));
}

#[test]
fn external_violations_can_be_suppressed() {
    let (_tmp, root) = fixture("policy");
    let page = root.join("src/web/page.ts");
    let source = fs::read_to_string(&page).unwrap();
    fs::write(
        &page,
        source.replace(
            "import moment",
            "// easement:disable-next-line external -- legacy locale\nimport moment",
        ),
    )
    .unwrap();
    let said = report_of(&root, Outcome::Failed);
    // It is no violation: it is listed only under the directive that excused it.
    let (violations, suppressions) = said.split_once("Inline suppressions:\n").unwrap();
    assert!(!violations.contains("moment"), "{said}");
    assert!(
        suppressions.starts_with("  src/web/page.ts:3 [external] legacy locale\n"),
        "{said}"
    );
    assert!(suppressions.contains("moment"), "{said}");
    assert!(said.contains("1 suppressed"), "{said}");
}

#[test]
fn a_directive_in_a_string_or_a_doc_example_is_not_a_suppression() {
    let (_tmp, root) = fixture("policy");
    let page = root.join("src/web/page.ts");
    let source = fs::read_to_string(&page).unwrap();
    // One literal per line: easement checks this file too, and cannot tell a
    // line inside a string that spans lines from a comment.
    let syntax = r#"export const syntax = "// easement:disable-next-line external -- why";"#;
    let example = "/** // easement:disable-line privacy -- an example */";
    fs::write(&page, format!("{source}\n{syntax}\n{example}\n")).unwrap();
    let said = report_of(&root, Outcome::Failed);
    assert!(!said.contains("suppression"), "{said}");
}

/// A key next to `<<` replaces the merged list: one that drops a shared ban
/// stops the command before it judges anything, as a duplicate key does.
#[test]
fn a_rule_that_drops_a_shared_ban_next_to_a_merge_stops_the_check() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: src/web }\nx-bans: &bans\n  forbid_external: [moment]\nrules:\n  web: { <<: *bans, forbid_external: [left-pad] }\n",
        ),
        ("src/web/page.ts", "export const page = 1;\n"),
    ]);
    let refused = error(try_check_with(&root, |_| {}));
    let input = config(&root);
    assert!(
        refused.starts_with(&format!("invalid config {}: ", input.display())),
        "{refused}"
    );
    assert!(
        refused.contains("`rules.web.forbid_external` written next to `<<` drops moment"),
        "{refused}"
    );
    assert!(refused.contains("split the anchor"), "{refused}");
}
