//! The imports a file writes against what its index resolved: an import is
//! resolved only when the index takes it to a source target, a file of the project
//! or an external package. Otherwise its dependency is read from its text, or it
//! is an `unresolved_imports` evidence gap that says why.

use std::path::{Path, PathBuf};

use easement::commands::Checked;
use easement::{CacheUse, EvidenceGap, GapKind};
use tempfile::TempDir;

use crate::common::{fixture, write_all};
use crate::run::{check, check_in, json, pinned, text};

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
        places(&checked, "the index holds no `require()`"),
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
