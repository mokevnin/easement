//! A name one component re-exports from another (#103): `gw` may depend on `chunk`
//! and `chunk` on `blob`, but `gw` not on `blob`. A name of `blob` that `gw` writes
//! through `chunk` is a dependency on `blob`; the lines of `chunk` that refer to
//! `blob` are evidence of it too, so a change to the re-export alone is judged.

use std::fs;
use std::path::PathBuf;

use easement::CacheUse;
use easement::commands::{self, CheckOptions, Outcome};
use tempfile::TempDir;

use crate::common::{fixture, git, on_feature, project, write_all};
use crate::diff::diff_main;
use crate::languages::THREE;
use crate::run::{
    by_rule, cache, cache_in, cannot_index_here, check, check_with, composer_install, config,
    found, json, pinned, text,
};

/// A project of the components `blob`, `chunk` and `gw` in the directories under
/// `dir`, of `files`, committed on `main` and checked out on `feature`.
pub(crate) fn three(dir: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let policy = THREE
        .replace("BLOB", &format!("{dir}blob"))
        .replace("CHUNK", &format!("{dir}chunk"))
        .replace("GW", &format!("{dir}gw"));
    let mut all = vec![("easement.yml", policy.as_str())];
    all.extend_from_slice(files);
    let (tmp, root) = project(&all);
    on_feature(&root);
    (tmp, root)
}

/// A Cargo workspace of `blob`, `chunk` and `gw`, with the sources of `chunk` and
/// `gw` as given.
pub(crate) fn rust(chunk: &str, gw: &str) -> (TempDir, PathBuf) {
    let manifest = |name: &str, deps: &str| {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
        )
    };
    three(
        "",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"gw\", \"chunk\", \"blob\"]\nresolver = \"2\"\n",
            ),
            ("blob/Cargo.toml", &manifest("blob", "")),
            (
                "blob/src/lib.rs",
                "pub struct Block {\n    pub len: usize,\n}\n",
            ),
            (
                "chunk/Cargo.toml",
                &manifest("chunk", "blob = { path = \"../blob\" }\n"),
            ),
            ("chunk/src/lib.rs", chunk),
            (
                "gw/Cargo.toml",
                &manifest("gw", "chunk = { path = \"../chunk\" }\n"),
            ),
            ("gw/src/lib.rs", gw),
        ],
    )
}

/// Rewrites `file` of `root` to `text` and commits it on the branch checked out.
pub(crate) fn change(root: &std::path::Path, file: &str, text: &str) {
    fs::write(root.join(file), text).unwrap();
    git(root, &["commit", "-q", "-am", "change"]);
}

/// `chunk` uses `blob::Block` in `make()` above the line that turns its own `Block`
/// into a re-export, so the first line of `chunk` that refers to `blob` is not the
/// one that changed.
#[test]
fn a_rust_re_export_changed_alone_is_judged_in_a_diff_run_and_named() {
    let (_tmp, root) = rust(
        "pub fn make() -> blob::Block {\n    blob::Block { len: 1 }\n}\n\npub struct Block;\n",
        "pub fn named(_: chunk::Block) {}\n",
    );
    change(
        &root,
        "chunk/src/lib.rs",
        "pub fn make() -> blob::Block {\n    blob::Block { len: 1 }\n}\n\npub use blob::Block;\n",
    );
    let checked = check_with(&root, diff_main);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    assert_eq!(
        found(&report),
        [("gw/src/lib.rs".to_string(), 1, "gw -> blob".to_string())],
        "{report}"
    );
    let violation = &report["violations"][0];
    assert_eq!(violation["reexported_by"], serde_json::json!(["chunk"]));
    assert!(
        violation["evidence"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "file": "chunk/src/lib.rs", "line": 5 })),
        "{violation}"
    );
    assert!(
        text(&checked).contains(
            "gw/src/lib.rs:1: gw -> blob is not allowed (written as blob/src/lib.rs, re-exported by chunk)"
        ),
        "{}",
        text(&checked)
    );
}

/// The one violation of a diff run in which only the re-exporter changed: at
/// `file:line` of `gw`, naming `chunk`, with the line of `chunk` that changed
/// among its evidence.
fn judged_through_chunk(root: &std::path::Path, file: &str, line: u64, changed: (&str, u64)) {
    let checked = check_with(root, diff_main);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    assert_eq!(
        found(&report),
        [(file.to_string(), line, "gw -> blob".to_string())],
        "{report}"
    );
    let violation = &report["violations"][0];
    assert_eq!(
        violation["reexported_by"],
        serde_json::json!(["chunk"]),
        "{violation}"
    );
    assert!(
        violation["evidence"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "file": changed.0, "line": changed.1 })),
        "{violation}"
    );
}

/// A glob re-export names only the module it re-exports, not the name.
#[test]
fn a_typescript_glob_re_export_changed_alone_is_judged_in_a_diff_run() {
    let (_tmp, root) = three(
        "src/",
        &[
            ("src/blob/index.ts", "export class Block {\n  len = 1;\n}\n"),
            (
                "src/chunk/index.ts",
                "import { Block as B } from \"../blob\";\n\nexport const make = (): B => new B();\n\nexport class Block {}\n",
            ),
            (
                "src/gw/named.ts",
                "import { Block } from \"../chunk\";\n\nexport const named = (b: Block) => b;\n",
            ),
        ],
    );
    change(
        &root,
        "src/chunk/index.ts",
        "import { Block as B } from \"../blob\";\n\nexport const make = (): B => new B();\n\nexport * from \"../blob\";\n",
    );
    judged_through_chunk(&root, "src/gw/named.ts", 1, ("src/chunk/index.ts", 5));
}

#[test]
fn a_python_name_an_init_re_exports_changed_alone_is_judged_in_a_diff_run() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = three(
        "app/",
        &[
            ("app/__init__.py", ""),
            ("app/blob/__init__.py", "class Block:\n    len = 1\n"),
            (
                "app/chunk/__init__.py",
                "from app.blob import Block as B\n\n\ndef make():\n    return B()\n\n\nclass Block:\n    pass\n",
            ),
            (
                "app/gw/__init__.py",
                "from app.chunk import Block\n\n\ndef named(b: Block):\n    return b\n",
            ),
        ],
    );
    change(
        &root,
        "app/chunk/__init__.py",
        "from app.blob import Block as B\n\n\ndef make():\n    return B()\n\n\nfrom app.blob import Block\n",
    );
    judged_through_chunk(&root, "app/gw/__init__.py", 1, ("app/chunk/__init__.py", 8));
}

/// TypeScript files of `gw` that use `Block` of `blob`: `named.ts` through the
/// barrel of `chunk`, `direct.ts` from `blob` itself.
fn typescript_both() -> (TempDir, PathBuf) {
    three(
        "src/",
        &[
            ("src/blob/index.ts", "export class Block {\n  len = 1;\n}\n"),
            (
                "src/chunk/index.ts",
                "export { Block } from \"../blob\";\nexport const own = 1;\n",
            ),
            (
                "src/gw/named.ts",
                "import { Block } from \"../chunk\";\n\nexport const named = (b: Block) => b;\n",
            ),
            (
                "src/gw/direct.ts",
                "import { Block } from \"../blob\";\n\nexport const direct = (b: Block) => b;\n",
            ),
        ],
    )
}

/// The full check reports each use once; only the one through `chunk` names it.
#[test]
fn a_full_check_names_the_re_exporter_only_of_a_name_written_through_it() {
    let (_tmp, root) = typescript_both();
    // Indexed afresh, so that no other run decides whether the report says it is cached.
    let own = TempDir::new().unwrap();
    let checked = commands::check(
        &CheckOptions::new(config(&root)),
        &mut cache_in(own.path(), CacheUse::Off),
    )
    .unwrap();
    let report = json(&checked);
    assert_eq!(
        found(&report),
        [
            ("src/gw/direct.ts".to_string(), 1, "gw -> blob".to_string()),
            ("src/gw/named.ts".to_string(), 1, "gw -> blob".to_string()),
        ],
        "{report}"
    );
    // `direct.ts` names no re-exporter and has its own line alone as evidence;
    // `named.ts` names `chunk`, with the barrel's line.
    pinned(|| {
        insta::assert_json_snapshot!("check_reexport_json", report["violations"]);
        insta::assert_snapshot!("check_reexport_text", text(&checked));
    });
}

/// A baseline written before the re-exporter was named still legalizes the violation.
#[test]
fn a_baselined_dependency_through_a_re_export_stays_legalized() {
    let (_tmp, root) = typescript_both();
    fs::write(
        root.join("easement.baseline.yml"),
        "violations:\n- rule: dependency\n  file: src/gw/direct.ts\n  target: src/blob/index.ts\n  from: gw\n  to: blob\n- rule: dependency\n  file: src/gw/named.ts\n  target: src/blob/index.ts\n  from: gw\n  to: blob\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed, "{}", text(&checked));
    assert!(text(&checked).contains("2 legalized"), "{}", text(&checked));
}

/// The lines of `chunk` stand behind the violation of `gw`, so a check limited to
/// `chunk` reports it too, as it reports a cycle that `chunk` is part of.
#[test]
fn a_check_limited_to_the_re_exporter_reports_the_violation_it_stands_behind() {
    let (_tmp, root) = typescript_both();
    let report = json(&check_with(&root, |o| o.components = vec!["chunk".into()]));
    assert_eq!(
        found(&report),
        [("src/gw/named.ts".to_string(), 1, "gw -> blob".to_string())],
        "{report}"
    );
}

/// A glob import of `chunk` names its module on its own line, never on the line
/// that writes the name.
#[test]
fn a_re_export_used_through_a_glob_import_is_judged_in_a_diff_run() {
    let (_tmp, root) = rust(
        "pub fn make() -> blob::Block {\n    blob::Block { len: 1 }\n}\n\npub struct Block;\n",
        "use chunk::*;\n\npub fn named(_: Block) {}\n",
    );
    change(
        &root,
        "chunk/src/lib.rs",
        "pub fn make() -> blob::Block {\n    blob::Block { len: 1 }\n}\n\npub use blob::Block;\n",
    );
    judged_through_chunk(&root, "gw/src/lib.rs", 3, ("chunk/src/lib.rs", 5));
}

/// A barrel that re-exports the name itself, as the full check sees it above.
#[test]
fn a_typescript_barrel_re_export_changed_alone_is_judged_in_a_diff_run() {
    let (_tmp, root) = three(
        "src/",
        &[
            ("src/blob/index.ts", "export class Block {\n  len = 1;\n}\n"),
            (
                "src/chunk/index.ts",
                "import { Block as B } from \"../blob\";\n\nexport const make = (): B => new B();\n\nexport class Block {}\n",
            ),
            (
                "src/gw/named.ts",
                "import { Block } from \"../chunk\";\n\nexport const named = (b: Block) => b;\n",
            ),
        ],
    );
    change(
        &root,
        "src/chunk/index.ts",
        "import { Block as B } from \"../blob\";\n\nexport const make = (): B => new B();\n\nexport { Block } from \"../blob\";\n",
    );
    judged_through_chunk(&root, "src/gw/named.ts", 1, ("src/chunk/index.ts", 5));
}

/// A module alias leaves no occurrence of `blob` (`from app import blob as b`), so
/// the line of `b.Block` names no module; `gw` also imports something of `chunk`,
/// which depends on `blob`, but re-exports nothing that `gw` uses.
#[test]
fn a_name_written_through_a_module_alias_names_no_re_exporter() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = three(
        "app/",
        &[
            ("app/__init__.py", ""),
            ("app/blob/__init__.py", "class Block:\n    len = 1\n"),
            (
                "app/chunk/__init__.py",
                "from app.blob import Block\n\nown = Block()\n",
            ),
            (
                "app/gw/__init__.py",
                "from app import blob as b\nfrom app.chunk import own\n\n\ndef named(x: b.Block):\n    return own\n",
            ),
        ],
    );
    let report = json(&check(&root));
    let through_chunk: Vec<&serde_json::Value> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["reexported_by"] != serde_json::Value::Null)
        .collect();
    assert_eq!(through_chunk, Vec::<&serde_json::Value>::new(), "{report}");
    assert!(
        found(&report)
            .iter()
            .any(|(file, _, pair)| file == "app/gw/__init__.py" && pair == "gw -> blob"),
        "{report}"
    );
}

/// A PHP class import names only a module, as a glob import does, but brings in no
/// names: `Order` of `domain` stands on a line of its own, and `domain` refers to
/// `infra`, yet the function of `infra` that `web` calls comes from `infra` itself.
#[test]
fn a_class_import_beside_a_function_import_names_no_re_exporter() {
    if cannot_index_here(&["php"]) {
        return;
    }
    let (_tmp, root) = fixture("php");
    write_all(
        &root,
        &[
            (
                "composer.json",
                "{\n  \"autoload\": { \"psr-4\": { \"App\\\\\": \"src/\" }, \"files\": [\"src/Infra/functions.php\"] }\n}\n",
            ),
            (
                "src/Infra/functions.php",
                "<?php\nnamespace App\\Infra;\n\nfunction connect(): int { return 1; }\n",
            ),
            (
                "src/Domain/Order.php",
                "<?php\nnamespace App\\Domain;\n\nuse App\\Infra\\OrderRepository;\n\nclass Order { public ?OrderRepository $repo = null; }\n",
            ),
            (
                "src/Web/Status.php",
                "<?php\nnamespace App\\Web;\n\nuse App\\Domain\\Order;\nuse function App\\Infra\\connect;\n\nclass Status { public function up(Order $o): int { return connect(); } }\n",
            ),
        ],
    );
    composer_install(&root);
    let report = json(&check(&root));
    assert!(
        found(&report)
            .iter()
            .any(|(file, _, pair)| file == "src/Web/Status.php" && pair == "web -> infra"),
        "{report}"
    );
    let named: Vec<&serde_json::Value> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["reexported_by"] != serde_json::Value::Null)
        .collect();
    assert_eq!(named, Vec::<&serde_json::Value>::new(), "{report}");
}

/// `blob` opens only its `public` paths, and its `core` to `chunk` alone; `x` may
/// use `chunk`. The components are the directories under `dir`, beside `files`.
fn closed_core(dir: &str, public: &str, core: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let policy = format!(
        "version: 1\ncomponents:\n  blob:\n    in: {dir}blob\n    public: {public}\n    surfaces:\n      core: {{ in: {core}, importers: [chunk] }}\n  chunk: {{ in: {dir}chunk }}\n  x: {{ in: {dir}x, optional: true }}\n  gw: {{ in: {dir}gw }}\nrules:\n  gw: {{ may_depend_on: [chunk, blob, x] }}\n  x: {{ may_depend_on: [chunk, blob] }}\n  chunk: {{ may_depend_on: [blob] }}\n"
    );
    let mut all = vec![("easement.yml", policy.as_str())];
    all.extend_from_slice(files);
    project(&all)
}

/// TypeScript files of `gw` that use `Secret` of `blob`'s `core` itself, through
/// `chunk` and through `x`; `Block` of `core` through `blob`'s `api`, directly and
/// through `chunk`; and `Block` through a glob re-export of `core` in `chunk`.
const CLOSED_CORE_TS: &[(&str, &str)] = &[
    ("src/blob/core/secret.ts", "export class Secret {}\n"),
    ("src/blob/core/block.ts", "export class Block {}\n"),
    (
        "src/blob/api/index.ts",
        "export { Block } from \"../core/block\";\n",
    ),
    (
        "src/chunk/index.ts",
        "export { Secret } from \"../blob/core/secret\";\nexport { Block } from \"../blob/api\";\n",
    ),
    ("src/x/index.ts", "export { Secret } from \"../chunk\";\n"),
    (
        "src/chunk/all.ts",
        "export * from \"../blob/core/block\";\n",
    ),
    (
        "src/gw/direct.ts",
        "import { Secret } from \"../blob/core/secret\";\n\nexport const direct = (s: Secret) => s;\n",
    ),
    (
        "src/gw/all.ts",
        "import { Block } from \"../chunk/all\";\n\nexport const all = (b: Block) => b;\n",
    ),
    (
        "src/gw/secret.ts",
        "import { Secret } from \"../chunk\";\n\nexport const secret = (s: Secret) => s;\n",
    ),
    (
        "src/gw/far.ts",
        "import { Secret } from \"../x\";\n\nexport const far = (s: Secret) => s;\n",
    ),
    (
        "src/gw/api.ts",
        "import { Block } from \"../blob/api\";\n\nexport const api = (b: Block) => b;\n",
    ),
    (
        "src/gw/block.ts",
        "import { Block } from \"../chunk\";\n\nexport const block = (b: Block) => b;\n",
    ),
];

/// A re-export in a third component opens no file of a component that the file it
/// enters it by keeps closed (#131): `chunk` may use `core`, but `gw` and `x` may
/// not use it through `chunk`, by name or by a glob. A route that enters `blob` by
/// its `api` stays open.
#[test]
fn a_name_reached_through_a_third_components_re_export_is_judged_where_it_enters_its_component() {
    let (_tmp, root) = closed_core("src/", "src/blob/api", "src/blob/core", CLOSED_CORE_TS);
    let checked = check(&root);
    let report = json(&checked);
    assert_eq!(
        by_rule(&report),
        [
            ("src/gw/all.ts".to_string(), 1, "privacy".to_string()),
            ("src/gw/direct.ts".to_string(), 1, "privacy".to_string()),
            ("src/gw/far.ts".to_string(), 1, "privacy".to_string()),
            ("src/gw/secret.ts".to_string(), 1, "privacy".to_string()),
            ("src/x/index.ts".to_string(), 1, "privacy".to_string()),
        ],
        "{report}"
    );
    // `why` decides each dependency as `check` does.
    let why = commands::why(&config(&root), "gw", "blob", &mut cache())
        .unwrap()
        .text();
    assert!(
        why.contains("src/gw/secret.ts:1") && why.contains("privacy"),
        "{why}"
    );
    pinned(|| {
        insta::assert_json_snapshot!("check_reexport_surface_json", report["violations"]);
        insta::assert_snapshot!("check_reexport_surface_text", text(&checked));
    });
}

/// The same in Rust: `chunk` re-exports `Secret` of `blob`'s `core` with `pub use`,
/// and `Block` of `core` through `blob`'s public `api`.
#[test]
fn a_rust_pub_use_in_a_third_component_opens_no_closed_file() {
    let manifest = |name: &str, deps: &str| {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
        )
    };
    let (_tmp, root) = closed_core(
        "",
        "blob/src/api.rs",
        "blob/src/core.rs",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"gw\", \"chunk\", \"blob\"]\nresolver = \"2\"\n",
            ),
            ("blob/Cargo.toml", &manifest("blob", "")),
            ("blob/src/lib.rs", "pub mod api;\npub mod core;\n"),
            (
                "blob/src/core.rs",
                "pub struct Secret;\n\npub struct Block;\n",
            ),
            ("blob/src/api.rs", "pub use crate::core::Block;\n"),
            (
                "chunk/Cargo.toml",
                &manifest("chunk", "blob = { path = \"../blob\" }\n"),
            ),
            (
                "chunk/src/lib.rs",
                "pub use blob::core::Secret;\npub use blob::api::Block;\n",
            ),
            (
                "gw/Cargo.toml",
                &manifest("gw", "chunk = { path = \"../chunk\" }\n"),
            ),
            (
                "gw/src/lib.rs",
                "use chunk::Secret;\nuse chunk::Block;\n\npub fn secret(s: Secret) -> Secret {\n    s\n}\n\npub fn block(b: Block) -> Block {\n    b\n}\n",
            ),
        ],
    );
    let report = json(&check(&root));
    assert_eq!(
        by_rule(&report),
        [("gw/src/lib.rs".to_string(), 1, "privacy".to_string())],
        "{report}"
    );
}

/// The same in Python: `chunk`'s `__init__` imports `Secret` of `blob`'s `core`, and
/// `Block` of `core` from `blob`'s public `api`.
#[test]
fn a_python_init_import_in_a_third_component_opens_no_closed_file() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let (_tmp, root) = closed_core(
        "app/",
        "[app/blob/api.py, app/blob/__init__.py]",
        "app/blob/core.py",
        &[
            ("app/__init__.py", ""),
            ("app/blob/__init__.py", ""),
            (
                "app/blob/core.py",
                "class Secret:\n    pass\n\n\nclass Block:\n    pass\n",
            ),
            ("app/blob/api.py", "from app.blob.core import Block\n"),
            (
                "app/chunk/__init__.py",
                "from app.blob.core import Secret\nfrom app.blob.api import Block\n",
            ),
            (
                "app/gw/__init__.py",
                "from app.chunk import Secret\nfrom app.chunk import Block\n\n\ndef secret(s: Secret) -> Block:\n    return Block()\n",
            ),
        ],
    );
    let report = json(&check(&root));
    assert_eq!(
        by_rule(&report),
        [("app/gw/__init__.py".to_string(), 1, "privacy".to_string())],
        "{report}"
    );
}

/// A diff run in which only `chunk` changed, from re-exporting `Block` of `blob`'s
/// public `api` to re-exporting it from `core`, judges the use in `gw`.
#[test]
fn a_re_export_turned_to_a_closed_file_is_judged_in_a_diff_run() {
    let (_tmp, root) = closed_core(
        "src/",
        "src/blob/api",
        "src/blob/core",
        &[
            ("src/blob/core/block.ts", "export class Block {}\n"),
            (
                "src/blob/api/index.ts",
                "export { Block } from \"../core/block\";\n",
            ),
            (
                "src/chunk/index.ts",
                "export { Block } from \"../blob/api\";\n",
            ),
            (
                "src/gw/block.ts",
                "import { Block } from \"../chunk\";\n\nexport const block = (b: Block) => b;\n",
            ),
        ],
    );
    on_feature(&root);
    change(
        &root,
        "src/chunk/index.ts",
        "export { Block } from \"../blob/core/block\";\n",
    );
    let report = json(&check_with(&root, diff_main));
    assert_eq!(
        by_rule(&report),
        [("src/gw/block.ts".to_string(), 1, "privacy".to_string())],
        "{report}"
    );
    assert!(
        report["violations"][0]["evidence"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "file": "src/chunk/index.ts", "line": 1 })),
        "{report}"
    );
}

/// A way that enters `blob` by a closed module of it is judged there, not at the
/// file that defines the name, and the violation names that module.
#[test]
fn a_way_into_a_closed_module_that_re_exports_is_named_by_that_module() {
    let (_tmp, root) = closed_core(
        "src/",
        "src/blob/api",
        "src/blob/core",
        &[
            ("src/blob/api/block.ts", "export class Block {}\n"),
            (
                "src/blob/inner/index.ts",
                "export { Block } from \"../api/block\";\n",
            ),
            (
                "src/chunk/index.ts",
                "export { Block } from \"../blob/inner\";\n",
            ),
            (
                "src/gw/block.ts",
                "import { Block } from \"../chunk\";\n\nexport const block = (b: Block) => b;\n",
            ),
        ],
    );
    let checked = check(&root);
    let report = json(&checked);
    assert_eq!(
        by_rule(&report),
        [
            ("src/chunk/index.ts".to_string(), 1, "privacy".to_string()),
            ("src/gw/block.ts".to_string(), 1, "privacy".to_string()),
        ],
        "{report}"
    );
    assert!(
        text(&checked).contains(
            "src/gw/block.ts:1: gw -> blob: src/blob/inner/index.ts is not in the public surface of blob"
        ),
        "{}",
        text(&checked)
    );
}

/// A barrel that re-exports a module of its own component whole (`export *`) passes
/// on what that module re-exports, glob or named: the way is followed through it.
#[test]
fn a_way_through_a_barrel_of_barrels_is_followed_to_the_closed_file() {
    let (_tmp, root) = closed_core(
        "src/",
        "src/blob/api",
        "src/blob/core",
        &[
            ("src/blob/core/secret.ts", "export class Secret {}\n"),
            ("src/blob/core/block.ts", "export class Block {}\n"),
            (
                "src/chunk/index.ts",
                "export * from \"./named\";\nexport * from \"./all\";\n",
            ),
            (
                "src/chunk/named.ts",
                "export { Secret } from \"../blob/core/secret\";\n",
            ),
            (
                "src/chunk/all.ts",
                "export * from \"../blob/core/block\";\n",
            ),
            (
                "src/gw/secret.ts",
                "import { Secret } from \"../chunk\";\n\nexport const secret = (s: Secret) => s;\n",
            ),
            (
                "src/gw/block.ts",
                "import { Block } from \"../chunk\";\n\nexport const block = (b: Block) => b;\n",
            ),
        ],
    );
    let report = json(&check(&root));
    assert_eq!(
        by_rule(&report),
        [
            ("src/gw/block.ts".to_string(), 1, "privacy".to_string()),
            ("src/gw/secret.ts".to_string(), 1, "privacy".to_string()),
        ],
        "{report}"
    );
    let evidence = &report["violations"][0]["evidence"];
    for line in [
        serde_json::json!({ "file": "src/chunk/index.ts", "line": 2 }),
        serde_json::json!({ "file": "src/chunk/all.ts", "line": 1 }),
    ] {
        assert!(evidence.as_array().unwrap().contains(&line), "{report}");
    }
}
