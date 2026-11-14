//! What an import that more than one file declares says about itself: the
//! evidence gap it leaves, the resolution a violation carries, and the
//! candidates a dependency lists.

use std::path::PathBuf;

use easement::commands;
use tempfile::TempDir;

use crate::common::{fixture, project};
use crate::run::{cache, check, gap_messages, json, leaves_out, parse, pinned, text};

/// The file that imports the name.
const IMPORT: &str = "app/src/main/kotlin/com/acme/App.kt";

/// The source of a Kotlin file that imports the name.
const APP: &str =
    "package com.acme\n\nimport com.acme.thing.Thing\n\nclass App {\n    val thing = Thing()\n}\n";

/// The source of a Kotlin file that declares the name.
const THING: &str = "package com.acme.thing\n\nclass Thing\n";

/// A Kotlin project where `app` imports `com.acme.thing.Thing`, and each pair
/// of `copies` is the component that holds a copy of it and the file that
/// declares it.
fn project_of(copies: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let mut components = vec!["app"];
    for (component, _) in copies {
        if !components.contains(component) {
            components.push(component);
        }
    }
    let policy = format!(
        "version: 1\n\ncomponents:\n{}\nrules:\n  app: {{ must_not_depend_on: [lib] }}\n",
        components
            .iter()
            .map(|component| format!("  {component}: {{ in: {component} }}\n"))
            .collect::<String>()
    );
    let mut files: Vec<(String, String)> = vec![
        ("easement.yml".into(), policy),
        (
            IMPORT.into(),
            "package com.acme\n\nimport com.acme.thing.Thing\n\nclass App {\n    val thing = Thing()\n}\n"
                .into(),
        ),
    ];
    for (_, path) in copies {
        files.push((
            (*path).into(),
            "package com.acme.thing\n\nclass Thing\n".into(),
        ));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    project(&files)
}

/// Two files that declare one name leave a gap: the import of it may mean
/// either, so the graph may be short.
#[test]
fn a_name_declared_in_two_components_is_an_evidence_gap() {
    let (_tmp, root) = fixture("kotlin_duplicate");
    let report = json(&check(&root));
    let gaps: Vec<_> = report["evidence_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|gap| gap["kind"] == "declarations")
        .collect();
    assert_eq!(gaps.len(), 1, "{report}");
    assert!(leaves_out(&report), "{report}");
    pinned(|| {
        insta::assert_json_snapshot!("declarations_gap", gaps[0]);
    });
}

/// A dependency that two components may answer for says how certain its
/// target is: the target is not proven.
#[test]
fn a_violation_over_two_components_says_the_target_is_ambiguous() {
    let (_tmp, root) = project_of(&[
        ("lib", "lib/src/main/kotlin/com/acme/thing/Thing.kt"),
        ("other", "other/src/main/kotlin/com/acme/thing/Thing.kt"),
    ]);
    let report = json(&check(&root));
    let violation = &report["violations"][0];
    assert_eq!(violation["rule"], "dependency");
    assert_eq!(violation["from"], "app");
    assert_eq!(violation["to"], "lib");
    assert_eq!(violation["resolution"], "ambiguous");
    // The copy of the component of the importing file, or the first by path.
    assert_eq!(
        violation["target"],
        "lib/src/main/kotlin/com/acme/thing/Thing.kt"
    );
}

/// Two declarations in one component leave the component known and the exact
/// file unknown.
#[test]
fn two_declarations_in_one_component_are_the_same_scope() {
    let (_tmp, root) = project_of(&[
        ("lib", "lib/src/main/kotlin/com/acme/thing/Thing.kt"),
        ("lib", "lib/src/main/kotlin/com/acme/other/Thing.kt"),
    ]);
    let report = json(&check(&root));
    let violation = &report["violations"][0];
    assert_eq!(violation["to"], "lib");
    assert_eq!(violation["resolution"], "same_scope");
}

/// A target that one file declares is proven, so the violation says nothing
/// of how certain it is.
#[test]
fn a_proven_target_carries_no_resolution() {
    let (_tmp, root) = project_of(&[("lib", "lib/src/main/kotlin/com/acme/thing/Thing.kt")]);
    let report = json(&check(&root));
    let violation = &report["violations"][0];
    assert_eq!(violation["rule"], "dependency");
    assert!(violation.get("resolution").is_none(), "{violation}");
    assert!(
        report["evidence_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|gap| gap["kind"] != "declarations"),
        "{report}"
    );
}

/// The dependency lists every file that may declare the name, so a reader
/// knows what else the import may mean.
#[test]
fn a_dependency_lists_the_declarations_the_import_may_mean() {
    let (_tmp, root) = project_of(&[
        ("lib", "lib/src/main/kotlin/com/acme/thing/Thing.kt"),
        ("other", "other/src/main/kotlin/com/acme/thing/Thing.kt"),
    ]);
    let why = commands::why(&root.join("easement.yml"), "app", "lib", &mut cache()).unwrap();
    let why = serde_json::to_value(&why).unwrap();
    let decision = &why["dependencies"][0]["decisions"][0];
    assert_eq!(decision["resolution"], "ambiguous");
    assert_eq!(
        decision["candidates"],
        serde_json::json!([
            "lib/src/main/kotlin/com/acme/thing/Thing.kt",
            "other/src/main/kotlin/com/acme/thing/Thing.kt"
        ])
    );
}

/// The policy of a project: the components, the rule of each, the paths the
/// policy hides and whether a cycle between components fails.
fn policy(components: &[&str], rules: &str, exclude: &[&str], no_cycles: bool) -> String {
    format!(
        "version: 1\n\ncomponents:\n{}\nrules:\n{}{}{}\n",
        components
            .iter()
            .map(|component| format!("  {component}: {{ in: {component} }}\n"))
            .collect::<String>(),
        rules,
        if exclude.is_empty() {
            String::new()
        } else {
            format!(
                "exclude: [{}]\n",
                exclude
                    .iter()
                    .map(|pattern| format!("\"{pattern}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
        if no_cycles { "no_cycles: true\n" } else { "" },
    )
}

/// A Kotlin project with the policy `policy` and the files `files`.
fn project_with(policy: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let mut all: Vec<(&str, &str)> = vec![("easement.yml", policy)];
    all.extend(files.iter().copied());
    project(&all)
}

/// `exclude` does not rank the candidates: the first by path is taken even when
/// it is hidden, so the import fails as `excluded` rather than being taken for
/// the copy the policy allows.
#[test]
fn an_excluded_candidate_is_not_replaced_by_another() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib"],
            "  app: { must_not_depend_on: [lib] }\n",
            &["legacy/**"],
            false,
        ),
        &[
            (IMPORT, APP),
            ("lib/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("legacy/src/main/kotlin/com/acme/thing/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    let violations = report["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "{report}");
    let violation = &violations[0];
    assert_eq!(violation["rule"], "excluded");
    assert_eq!(violation["from"], "app");
    assert_eq!(violation["to"], "");
    assert_eq!(
        violation["target"],
        "legacy/src/main/kotlin/com/acme/thing/Thing.kt"
    );
    // The doubt stays: two files declare the name.
    let gaps = gap_messages(&report, "declarations");
    assert_eq!(gaps.len(), 1, "{report}");
    assert!(
        gaps[0]
            .as_str()
            .unwrap()
            .starts_with("2 places declare `com.acme.thing.Thing`"),
        "{gaps:?}"
    );
}

/// The copy in the importing file's component is taken before the first by
/// path, so an import is judged by the copy of its own component.
#[test]
fn a_candidate_of_the_importing_component_is_preferred() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib"],
            "  app: { may_depend_on: [] }\n",
            &[],
            false,
        ),
        &[
            (IMPORT, APP),
            ("app/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("lib/src/main/kotlin/com/acme/thing/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    assert!(
        report["violations"].as_array().unwrap().is_empty(),
        "{report}"
    );
    let gaps = gap_messages(&report, "declarations");
    assert_eq!(gaps.len(), 1, "{report}");
}

/// A wildcard import means every copy of a package, so several directories of
/// one package are no duplicate declaration. Its target is still not proven:
/// copies in one component tell `same_scope`.
#[test]
fn a_wildcard_import_over_two_directories_is_no_gap() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib"],
            "  app: { must_not_depend_on: [lib] }\n",
            &[],
            false,
        ),
        &[
            (
                "app/src/main/kotlin/com/acme/App.kt",
                "package com.acme\n\nimport com.acme.thing.*\n\nclass App\n",
            ),
            (
                "lib/src/main/kotlin/com/acme/thing/A.kt",
                "package com.acme.thing\n\nclass A\n",
            ),
            (
                "lib/src/test/kotlin/com/acme/thing/B.kt",
                "package com.acme.thing\n\nclass B\n",
            ),
        ],
    );
    let why = commands::why(&root.join("easement.yml"), "app", "lib", &mut cache()).unwrap();
    let why = serde_json::to_value(&why).unwrap();
    let decision = &why["dependencies"][0]["decisions"][0];
    assert_eq!(
        decision["candidates"],
        serde_json::json!([
            "lib/src/main/kotlin/com/acme/thing",
            "lib/src/test/kotlin/com/acme/thing"
        ]),
        "{why}"
    );
    let report = json(&check(&root));
    assert_eq!(
        report["violations"][0]["resolution"], "same_scope",
        "{report}"
    );
    assert!(gap_messages(&report, "declarations").is_empty(), "{report}");
}

/// A function of one name in several files is an overload or an extension, not
/// a duplicate declaration: every copy stays and no gap is raised. Copies in
/// one component tell `same_scope`.
#[test]
fn a_function_of_one_name_in_two_files_is_no_gap() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib"],
            "  app: { must_not_depend_on: [lib] }\n",
            &[],
            false,
        ),
        &[
            (
                "app/src/main/kotlin/com/acme/App.kt",
                "package com.acme\n\nimport com.acme.thing.helper\n\nclass App {\n    val value = helper()\n}\n",
            ),
            (
                "lib/src/main/kotlin/com/acme/thing/One.kt",
                "package com.acme.thing\n\nfun helper() = 1\n",
            ),
            (
                "lib/src/main/kotlin/com/acme/thing/Two.kt",
                "package com.acme.thing\n\nfun helper() = 2\n",
            ),
        ],
    );
    let report = json(&check(&root));
    assert_eq!(
        report["violations"][0]["resolution"], "same_scope",
        "{report}"
    );
    assert!(gap_messages(&report, "declarations").is_empty(), "{report}");
    let why = commands::why(&root.join("easement.yml"), "app", "lib", &mut cache()).unwrap();
    let why = serde_json::to_value(&why).unwrap();
    let decision = &why["dependencies"][0]["decisions"][0];
    assert_eq!(
        decision["candidates"],
        serde_json::json!([
            "lib/src/main/kotlin/com/acme/thing/One.kt",
            "lib/src/main/kotlin/com/acme/thing/Two.kt"
        ]),
        "{why}"
    );
}

/// A property of one name in several files is no duplicate declaration either,
/// and copies in one component tell `same_scope`.
#[test]
fn a_property_of_one_name_in_two_files_is_no_gap() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib"],
            "  app: { must_not_depend_on: [lib] }\n",
            &[],
            false,
        ),
        &[
            (
                "app/src/main/kotlin/com/acme/App.kt",
                "package com.acme\n\nimport com.acme.thing.LIMIT\n\nclass App {\n    val value = LIMIT\n}\n",
            ),
            (
                "lib/src/main/kotlin/com/acme/thing/One.kt",
                "package com.acme.thing\n\nconst val LIMIT = 1\n",
            ),
            (
                "lib/src/main/kotlin/com/acme/thing/Two.kt",
                "package com.acme.thing\n\nconst val LIMIT = 2\n",
            ),
        ],
    );
    let report = json(&check(&root));
    assert_eq!(
        report["violations"][0]["resolution"], "same_scope",
        "{report}"
    );
    assert!(gap_messages(&report, "declarations").is_empty(), "{report}");
}

/// An import that means every copy and finds them in different components is no
/// duplicate declaration, but its target is not proven: it tells `ambiguous`.
#[test]
fn a_function_of_one_name_in_two_components_is_ambiguous_but_no_gap() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib", "other"],
            "  app: { must_not_depend_on: [lib] }\n",
            &[],
            false,
        ),
        &[
            (
                "app/src/main/kotlin/com/acme/App.kt",
                "package com.acme\n\nimport com.acme.thing.helper\n\nclass App {\n    val value = helper()\n}\n",
            ),
            (
                "lib/src/main/kotlin/com/acme/thing/One.kt",
                "package com.acme.thing\n\nfun helper() = 1\n",
            ),
            (
                "other/src/main/kotlin/com/acme/thing/Two.kt",
                "package com.acme.thing\n\nfun helper() = 2\n",
            ),
        ],
    );
    let report = json(&check(&root));
    assert_eq!(
        report["violations"][0]["resolution"], "ambiguous",
        "{report}"
    );
    assert!(gap_messages(&report, "declarations").is_empty(), "{report}");
}

/// `impact` of a copy that `decide` did not pick still names the importer: the
/// file graph keeps an edge to every candidate.
#[test]
fn impact_of_an_alternative_copy_names_the_importer() {
    let (_tmp, root) = project_of(&[
        ("lib", "lib/src/main/kotlin/com/acme/thing/Thing.kt"),
        ("other", "other/src/main/kotlin/com/acme/thing/Thing.kt"),
    ]);
    let found = commands::impact(
        &root.join("easement.yml"),
        &[root.join("other/src/main/kotlin/com/acme/thing/Thing.kt")],
        None,
        &mut cache(),
    )
    .unwrap();
    let found = parse(&found.json().unwrap());
    let dependents: Vec<&str> = found["dependents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["file"].as_str().unwrap())
        .collect();
    assert!(dependents.contains(&IMPORT), "{found}");
    // The graph keeps every copy, so the gap hides no dependency from `impact`.
    assert!(
        found["evidence_gaps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|gap| gap["kind"] != "declarations"),
        "{found}"
    );
}

/// The text report says the same as the JSON gap: the rule the import follows
/// and how to close it.
#[test]
fn the_text_report_names_the_declarations_gap() {
    let (_tmp, root) = fixture("kotlin_duplicate");
    let report = text(&check(&root));
    assert!(
        report.contains("an import of it is taken for the copy in the importing file's component"),
        "{report}"
    );
    pinned(|| {
        insta::assert_snapshot!("declarations_gap_text", report);
    });
}

/// A copy in no component is a candidate like another: the first by path is
/// taken, and the dependency of a component's copy is judged as usual.
#[test]
fn a_candidate_outside_the_components_is_no_special_case() {
    let (_tmp, root) = project_with(
        &policy(
            &["app", "lib"],
            "  app: { must_not_depend_on: [lib] }\n",
            &[],
            false,
        ),
        &[
            (IMPORT, APP),
            ("lib/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("shared/src/main/kotlin/com/acme/thing/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    let violations = report["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "{report}");
    let violation = &violations[0];
    assert_eq!(violation["rule"], "dependency");
    assert_eq!(violation["to"], "lib");
    assert_eq!(
        violation["target"],
        "lib/src/main/kotlin/com/acme/thing/Thing.kt"
    );
    assert_eq!(violation["resolution"], "ambiguous");
}

/// When the policy hides every candidate, the dependency fails as `excluded`:
/// the enforcement of the policy stays.
#[test]
fn a_dependency_of_only_hidden_candidates_fails_as_excluded() {
    let (_tmp, root) = project_with(
        &policy(
            &["app"],
            "  app: { may_depend_on: [] }\n",
            &["legacy/**"],
            false,
        ),
        &[
            (IMPORT, APP),
            ("legacy/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("legacy/src/main/kotlin/com/acme/other/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    let violations = report["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1, "{report}");
    let violation = &violations[0];
    assert_eq!(violation["rule"], "excluded");
    assert_eq!(violation["from"], "app");
    assert_eq!(violation["to"], "");
    assert_eq!(
        violation["target"],
        "legacy/src/main/kotlin/com/acme/other/Thing.kt"
    );
}

/// When every candidate belongs to no component, no rule judges the
/// dependency: the doubt is left in the report.
#[test]
fn a_dependency_of_only_unowned_candidates_is_judged_by_no_rule() {
    let (_tmp, root) = project_with(
        &policy(&["app"], "  app: { may_depend_on: [] }\n", &[], false),
        &[
            (IMPORT, APP),
            ("shared/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("shared/src/main/kotlin/com/acme/other/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    assert!(
        report["violations"].as_array().unwrap().is_empty(),
        "{report}"
    );
    // The doubt stays: two files declare the name, and no rule judged the
    // import of it.
    let gaps = gap_messages(&report, "declarations");
    assert_eq!(gaps.len(), 1, "{report}");
    assert!(
        gaps[0]
            .as_str()
            .unwrap()
            .starts_with("2 places declare `com.acme.thing.Thing`"),
        "{gaps:?}"
    );
}

/// A cycle between components that only the alternative candidate closes is
/// reported: the graph keeps an edge to every candidate of a component.
#[test]
fn a_cycle_through_an_alternative_candidate_is_reported() {
    let (_tmp, root) = project_with(
        &policy(
            &["a", "b"],
            "  a: { may_depend_on: [b] }\n  b: { may_depend_on: [a] }\n",
            &[],
            true,
        ),
        &[
            (
                "a/src/main/kotlin/com/acme/Use.kt",
                "package com.acme\n\nimport com.acme.thing.Thing\n\nclass UseA {\n    val thing = Thing()\n}\n",
            ),
            (
                "b/src/main/kotlin/com/acme/Use.kt",
                "package com.acme\n\nimport com.acme.thing.Thing\n\nclass UseB {\n    val thing = Thing()\n}\n",
            ),
            ("a/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("b/src/main/kotlin/com/acme/thing/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    let cycles: Vec<_> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|violation| violation["rule"] == "cycle")
        .collect();
    assert_eq!(cycles.len(), 1, "{report}");
    assert_eq!(
        cycles[0]["message"],
        "component dependency cycle: a -> b -> a"
    );
}

/// A candidate outside every component adds no edge to the graph of the
/// components: no cycle comes of it.
#[test]
fn a_candidate_outside_the_components_adds_no_edge() {
    let (_tmp, root) = project_with(
        &policy(
            &["a", "b"],
            "  a: { may_depend_on: [b] }\n  b: { may_depend_on: [a] }\n",
            &[],
            true,
        ),
        &[
            (
                "a/src/main/kotlin/com/acme/Use.kt",
                "package com.acme\n\nimport com.acme.thing.Thing\n\nclass UseA {\n    val thing = Thing()\n}\n",
            ),
            (
                "b/src/main/kotlin/com/acme/Use.kt",
                "package com.acme\n\nimport com.acme.thing.Thing\n\nclass UseB {\n    val thing = Thing()\n}\n",
            ),
            ("a/src/main/kotlin/com/acme/thing/Thing.kt", THING),
            ("shared/src/main/kotlin/com/acme/thing/Thing.kt", THING),
        ],
    );
    let report = json(&check(&root));
    assert!(
        report["violations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|violation| violation["rule"] != "cycle"),
        "{report}"
    );
}
