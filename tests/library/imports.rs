//! The imports a file writes against what its index resolved: an import is
//! resolved only when the index takes it to a source target, a file of the project
//! or an external package. Otherwise its dependency is read from its text, or it
//! is an `unresolved_imports` evidence gap that says why.

use std::path::{Path, PathBuf};

use easement::commands::{Checked, Outcome};
use easement::{CacheUse, EvidenceGap, GapKind};
use tempfile::TempDir;

use crate::common::{fixture, project, write_all};
use crate::run::{
    by_message, cannot_index_here, check, check_in, check_with, config, found, json, pinned, text,
};

/// The `ts_imports` fixture, with the packages it imports installed: Vite's
/// client types, which declare `"*.css"`, `"*.svg"` and `"*?raw"` by wildcard,
/// and `some-pkg`.
fn ts_imports() -> (TempDir, PathBuf) {
    let (tmp, root) = fixture("ts_imports");
    install(&root);
    (tmp, root)
}

fn install(root: &Path) {
    write_all(
        root,
        &[
            (
                "node_modules/vite/package.json",
                r#"{"name":"vite","version":"5.0.0"}"#,
            ),
            (
                "node_modules/vite/client.d.ts",
                "declare module \"*.css\" { const css: string; export default css; }\n\
                 declare module \"*.svg\" { const src: string; export default src; }\n\
                 declare module \"*?raw\" { const src: string; export default src; }\n",
            ),
            (
                "node_modules/some-pkg/package.json",
                r#"{"name":"some-pkg","version":"1.0.0","types":"index.d.ts"}"#,
            ),
            (
                "node_modules/some-pkg/index.d.ts",
                "export declare const x: number;\n",
            ),
        ],
    );
}

/// The `unresolved_imports` gaps of a check.
fn unresolved(checked: &Checked) -> Vec<&EvidenceGap> {
    checked
        .coverage
        .evidence_gaps
        .iter()
        .filter(|gap| gap.kind == GapKind::UnresolvedImports)
        .collect()
}

/// `(line, specifier)` of every place of the gap whose message starts with `start`.
fn places(checked: &Checked, start: &str) -> Vec<(usize, String)> {
    let gaps: Vec<&EvidenceGap> = unresolved(checked)
        .into_iter()
        .filter(|gap| gap.message.starts_with(start))
        .collect();
    let [gap] = gaps.as_slice() else {
        panic!("one gap that starts `{start}`: {:?}", unresolved(checked));
    };
    gap.places
        .iter()
        .map(|place| (place.line, place.specifier.clone().unwrap_or_default()))
        .collect()
}

fn at(places: &[(usize, &str)]) -> Vec<(usize, String)> {
    places
        .iter()
        .map(|(line, specifier)| (*line, specifier.to_string()))
        .collect()
}

/// A wildcard declaration is no module: an import it catches is a dependency on
/// the file its path names, or on the package it spells; never on the file that
/// declares the wildcard, nor on an external `vite/client.d.ts/"*.css"`.
#[test]
fn an_import_a_wildcard_catches_depends_on_the_file_or_package_it_spells() {
    let (_tmp, root) = ts_imports();
    let report = json(&check(&root));
    let mut targets: Vec<(String, u64, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["target"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    targets.sort();
    let main = "src/app/main.ts";
    let expected: Vec<(String, u64, String)> = [
        ("src/app/legacy.js", 1, "src/lib/util.ts"),
        (main, 1, "src/lib/util.ts"),
        (main, 3, "src/assets/app.css"),
        (main, 4, "src/assets/logo.svg"),
        (main, 5, "src/lib/raw.ts"),
        (main, 6, "src/assets/Comp.svelte"),
        (main, 9, "src/assets/data.json"),
        (main, 10, "some-pkg"),
    ]
    .into_iter()
    .map(|(file, line, target)| (file.to_string(), line, target.to_string()))
    .collect();
    assert_eq!(targets, expected, "{report:#}");
}

/// Every import the index left without a source target that its text does not
/// name a file or a package by is a gap, one per cause.
#[test]
fn an_import_the_index_takes_nowhere_is_a_gap_that_names_its_cause() {
    let (_tmp, root) = ts_imports();
    let checked = check(&root);
    assert_eq!(
        places(&checked, "the indexer did not resolve"),
        at(&[(8, "../lib/missing")])
    );
    assert_eq!(
        places(&checked, "the index ties"),
        at(&[(11, "#lib/x.css"), (12, "$lib/x.css")])
    );
    assert_eq!(
        places(&checked, "the index holds nothing of"),
        at(&[(7, "../lib/polyfill")])
    );
    assert_eq!(
        places(
            &checked,
            "the index holds no module of 1 call(s) that import one"
        ),
        at(&[(14, "../lib/util")])
    );
    assert_eq!(
        places(&checked, "the target of"),
        at(&[
            (16, "`../lib/${name}`"),
            (17, "\"../lib/\" + name"),
            (18, "../lib/*.ts")
        ])
    );
    assert_eq!(unresolved(&checked).len(), 5, "{:?}", unresolved(&checked));
    // A declaration file describes code that lives elsewhere: it is not compared.
    assert!(
        unresolved(&checked)
            .iter()
            .all(|gap| gap.files == ["src/app/main.ts"]),
        "{:?}",
        unresolved(&checked)
    );
}

/// What each cause says, and how to close it, in the text and the JSON.
#[test]
fn each_cause_of_an_unresolved_import_is_pinned() {
    let (_tmp, root) = ts_imports();
    // Indexed afresh, so that no other run decides whether the output says it is cached.
    let own = TempDir::new().unwrap();
    let checked = check_in(&root, own.path(), CacheUse::Off).unwrap();
    let report = json(&checked);
    let said = text(&checked);
    let start = said.find("Evidence gaps").expect("the text lists the gaps");
    pinned(|| {
        insta::assert_json_snapshot!("unresolved_import_causes_json", report["evidence_gaps"]);
        insta::assert_snapshot!("unresolved_import_causes_text", &said[start..]);
    });
}

/// An import of a file the walk leaves out (one .gitignore hides), which the index
/// resolves, is a gap that says so, whether its path is relative or goes through
/// `paths`; an import of a file that does not exist still did not resolve. Tracking
/// the file closes the gap.
#[test]
fn an_import_of_a_file_the_walk_leaves_out_is_a_gap_that_says_so() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\n",
        ),
        ("package.json", "{\"name\": \"app\", \"private\": true}\n"),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler","baseUrl":".","paths":{"@/*":["src/*"]}},"include":["src"]}"#,
        ),
        (".gitignore", "src/gen/\n"),
        ("src/gen/api.ts", "export const api = 1;\n"),
        (
            "src/app/a.ts",
            "import { api } from \"../gen/api\";\nimport { api as again } from \"@/gen/api\";\nimport { gone } from \"../lib/missing\";\nexport const all = [api, again, gone];\n",
        ),
    ]);
    // Indexed afresh: the cases differ only in what the walk leaves out.
    let fresh = || {
        let own = TempDir::new().unwrap();
        check_in(&root, own.path(), CacheUse::Off).unwrap()
    };
    let checked = fresh();
    assert_eq!(
        places(&checked, "the index takes"),
        at(&[(1, "../gen/api"), (2, "@/gen/api")])
    );
    assert_eq!(
        places(&checked, "the indexer did not resolve"),
        at(&[(3, "../lib/missing")])
    );
    let report = json(&checked);
    pinned(|| {
        insta::assert_json_snapshot!("unwalked_import_json", report["evidence_gaps"]);
    });
    std::fs::remove_file(root.join(".gitignore")).unwrap();
    let checked = fresh();
    assert_eq!(
        unresolved(&checked)
            .iter()
            .map(|gap| gap.places.len())
            .sum::<usize>(),
        1,
        "{:?}",
        unresolved(&checked)
    );
}

/// Every form of a Python import the index resolves is no gap; the module of a
/// relative import it loses is read from its text when that names exactly one
/// file (`from .. import a`, in a package without `__init__.py`); every other
/// import it loses is a gap, one per cause, and `--require-evidence` fails on them.
#[test]
fn a_python_import_the_index_loses_is_read_from_its_text_or_is_a_gap() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = fixture("python_imports");
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    assert_eq!(
        places(&checked, "the indexer did not resolve"),
        at(&[
            (3, "lib.missing"),
            (4, ".nowhere"),
            (5, ".gone"),
            (6, "not_installed_anywhere"),
        ])
    );
    assert_eq!(
        places(&checked, "the index holds no module of"),
        // `.sibling` names `src/app/sibling.py` only relative to the package the
        // call is given, so it is no text to read a file from.
        at(&[(8, "lib.util"), (10, ".sibling")])
    );
    assert_eq!(places(&checked, "the target of"), at(&[(9, "m.__name__")]));
    assert_eq!(unresolved(&checked).len(), 3, "{:?}", unresolved(&checked));
    // Every form `main.py` writes resolves.
    assert!(
        unresolved(&checked)
            .iter()
            .all(|gap| gap.files == ["src/app/lost.py"]),
        "{:?}",
        unresolved(&checked)
    );
    // `b.py` depends on `a.py`, which its import names by its text alone.
    let policy = std::fs::read_to_string(config(&root)).unwrap();
    std::fs::write(
        config(&root),
        policy.replace("may_depend_on: [ns]", "may_depend_on: []"),
    )
    .unwrap();
    let report = json(&check(&root));
    assert!(
        report["violations"].as_array().unwrap().iter().any(|v| {
            v["file"] == "src/ns/inner/b.py" && v["line"] == 1 && v["target"] == "src/ns/a.py"
        }),
        "{report:#}"
    );
}

/// What each cause of an unresolved Python import says, and how to close it.
#[test]
fn each_cause_of_an_unresolved_python_import_is_pinned() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = fixture("python_imports");
    let own = TempDir::new().unwrap();
    let checked = check_in(&root, own.path(), CacheUse::Off).unwrap();
    let report = json(&checked);
    let said = text(&checked);
    let start = said.find("Evidence gaps").expect("the text lists the gaps");
    pinned(|| {
        insta::assert_json_snapshot!(
            "unresolved_python_import_causes_json",
            report["evidence_gaps"]
        );
        insta::assert_snapshot!("unresolved_python_import_causes_text", &said[start..]);
    });
}

/// A comment before the argument of a Python import call is no part of it either:
/// a literal call stays a gap of its own cause, named by its module, and a computed
/// one is named by its argument, which an `accepted_gaps` entry accepts it by.
#[test]
fn a_comment_before_the_argument_of_a_python_import_call_is_no_part_of_it() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\n  lib: { in: src/lib }\naccepted_gaps:\n  - { kind: unresolved_imports, file: src/app/main.py, specifier: m.__name__, because: the module is known at run time }\n",
        ),
        ("src/app/__init__.py", ""),
        (
            "src/app/main.py",
            "import importlib\nimport lib.util as m\na = importlib.import_module(  # c\n    \"lib.util\")\nb = importlib.import_module(  # c\n    m.__name__)\n",
        ),
        ("src/lib/__init__.py", ""),
        ("src/lib/util.py", "X = 1\n"),
    ]);
    let checked = check_with(&root, |o| o.require_evidence = true);
    let report = json(&checked);
    assert_eq!(
        places(&checked, "the index holds no module of"),
        at(&[(4, "lib.util")]),
        "{report:#}"
    );
    assert_eq!(
        report["accepted_gaps"][0]["places"],
        serde_json::json!([
            { "file": "src/app/main.py", "line": 6, "specifier": "m.__name__" },
        ]),
        "{report:#}"
    );
    assert_eq!(unresolved(&checked).len(), 1, "{:?}", unresolved(&checked));
}

/// The Python projects whose imports all resolve stay without a gap.
#[test]
fn a_python_project_whose_imports_resolve_has_no_unresolved_import() {
    if cannot_index_here(&["python"]) {
        return;
    }
    for name in ["python", "implementations_python"] {
        let (_tmp, root) = fixture(name);
        let checked = check(&root);
        assert!(
            unresolved(&checked).is_empty(),
            "{name}: {:?}",
            unresolved(&checked)
        );
    }
}

/// A project whose `app` reaches into the installed `flag-icons` by a glob of its
/// files (`src/app/glob.ts`) and by a literal path to one (`src/app/url.ts`), with
/// `rules` for `app` and `dependencies` for its `package.json`.
fn flags(rules: &str, dependencies: &str) -> (TempDir, PathBuf) {
    project(&[
        (
            "easement.yml",
            &format!(
                "version: 1\nmanifests: true\ncomponents:\n  app: {{ in: src/app }}\nrules:\n  app: {rules}\n"
            ),
        ),
        (
            "package.json",
            &format!(
                "{{\"name\": \"app\", \"private\": true, \"dependencies\": {{{dependencies}}}}}\n"
            ),
        ),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler"},"include":["src"]}"#,
        ),
        (
            "node_modules/flag-icons/package.json",
            r#"{"name":"flag-icons","version":"7.0.0"}"#,
        ),
        ("node_modules/flag-icons/flags/4x3/de.svg", "<svg/>\n"),
        (
            "src/app/glob.ts",
            "export const flags = import.meta.glob(\"/node_modules/flag-icons/flags/4x3/*.svg\", { eager: true });\n",
        ),
        (
            "src/app/url.ts",
            "import de from \"/node_modules/flag-icons/flags/4x3/de.svg?url\";\nexport const flag = de;\n",
        ),
    ])
}

/// `(file, line, message)` of the violations of both files of [`flags`], each
/// with `message` at its first line.
fn in_both_files(message: &str) -> Vec<(String, u64, String)> {
    ["src/app/glob.ts", "src/app/url.ts"]
        .map(|file| (file.to_string(), 1, message.to_string()))
        .to_vec()
}

/// A glob, or a literal path, whose static prefix runs into one installed package
/// is a use of that package, judged as a literal import of it is: by
/// `forbid_external`, by `may_use_external` and by `manifests`, with no gap left.
#[test]
fn a_glob_into_an_installed_package_is_a_use_of_it() {
    let (_tmp, root) = flags(
        "{ forbid_external: [flag-icons] }",
        "\"flag-icons\": \"7.0.0\"",
    );
    let report = json(&check(&root));
    let forbidden = "app -> external flag-icons is forbidden";
    assert_eq!(by_message(&report), in_both_files(forbidden), "{report:#}");

    let (_tmp, root) = flags(
        "{ may_use_external: [flag-icons] }",
        "\"flag-icons\": \"7.0.0\"",
    );
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::Passed, "{:#}", json(&checked));
    assert!(
        unresolved(&checked).is_empty(),
        "{:?}",
        unresolved(&checked)
    );

    let (_tmp, root) = flags("{ may_use_external: [flag-icons] }", "");
    let report = json(&check(&root));
    let undeclared = "flag-icons is not declared in package.json; add it to the dependencies, or drop the dependency";
    assert_eq!(by_message(&report), in_both_files(undeclared), "{report:#}");
}

/// A glob whose static prefix stops short of a whole package, or that is bare,
/// which an alias may stand for, stays a gap.
#[test]
fn a_glob_short_of_one_installed_package_stays_a_gap() {
    let (_tmp, root) = flags(
        "{ may_use_external: [flag-icons] }",
        "\"flag-icons\": \"7.0.0\"",
    );
    write_all(
        &root,
        &[(
            "src/app/short.ts",
            "export const any = import.meta.glob(\"/node_modules/flag-*/x.svg\");\nexport const bare = import.meta.glob(\"flag-icons/flags/*.svg\");\n",
        )],
    );
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    assert_eq!(
        places(&checked, "the target of"),
        at(&[
            (1, "/node_modules/flag-*/x.svg"),
            (2, "flag-icons/flags/*.svg")
        ])
    );
}

/// A comment before the argument of an import call is no part of it: a literal
/// import with one is a dependency, as without it, whether the index resolves it or
/// it is read from its text (the index ties it to a wildcard declaration); a
/// computed one is named by its argument, which an `accepted_gaps` entry accepts it
/// by.
#[test]
fn a_comment_before_the_argument_of_an_import_call_is_no_part_of_it() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\n  lib: { in: src/lib }\nrules:\n  app: { may_depend_on: [] }\naccepted_gaps:\n  - { kind: unresolved_imports, file: src/app/a.ts, specifier: '`${base}runtime.js`', because: the base is known at run time }\n",
        ),
        ("package.json", "{\"name\": \"app\", \"private\": true}\n"),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler"},"include":["src"]}"#,
        ),
        (
            "src/lib/shims.d.ts",
            "declare module \"*.svg\" { const src: string; export default src; }\n",
        ),
        ("src/lib/logo.svg", "<svg/>\n"),
        ("src/lib/b.ts", "export const b = 1;\n"),
        (
            "src/app/a.ts",
            "export const logo = import(/* webpackChunkName: \"logo\" */ \"../lib/logo.svg\");\nexport const b = import(/* webpackChunkName: \"b\" */ \"../lib/b\");\nexport const load = (base: string) => import(/* @vite-ignore */ `${base}runtime.js`);\n",
        ),
    ]);
    let checked = check_with(&root, |o| o.require_evidence = true);
    let report = json(&checked);
    assert_eq!(
        found(&report),
        [1, 2].map(|line| ("src/app/a.ts".to_string(), line, "app -> lib".to_string())),
        "{report:#}"
    );
    assert_eq!(
        report["accepted_gaps"][0]["places"],
        serde_json::json!([
            { "file": "src/app/a.ts", "line": 3, "specifier": "`${base}runtime.js`" },
        ]),
        "{report:#}"
    );
    assert!(
        unresolved(&checked).is_empty(),
        "{:?}",
        unresolved(&checked)
    );
}
