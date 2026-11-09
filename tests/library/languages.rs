//! How each language's imports and members resolve into dependencies: Go,
//! TypeScript, Python, Java, C#, Rust (with its test code: `#[cfg(test)]`,
//! integration tests, benches and examples), PHP, Ruby, Kotlin, Swift and Svelte; and
//! what `exclude` and the baseline do with what was resolved.

use std::fs;
use std::path::{Path, PathBuf};

use easement::commands::{self, Outcome};
use tempfile::TempDir;

use crate::common::{fixture, write_all};
use crate::run::{
    cache, check, check_with, composer_install, config, gap_messages, json, leaves_out,
    no_scip_ruby_here, text,
};

/// The violations of a check as (file, target), and its counters.
fn violations(root: &Path) -> (Vec<(String, String)>, serde_json::Value) {
    let report = json(&check(root));
    let pairs = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["target"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    (pairs, report["stats"].clone())
}

/// The violations of a check, as (file, line, rule, from -> to).
pub(crate) fn judged(root: &Path) -> (Vec<(String, u64, String, String)>, serde_json::Value) {
    let report = json(&check(root));
    let found = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["rule"].as_str().unwrap().to_string(),
                format!(
                    "{} -> {}",
                    v["from"].as_str().unwrap(),
                    v["to"].as_str().unwrap()
                ),
            )
        })
        .collect();
    (found, report)
}

/// What `explain` says of `file` under the policy of `root`.
fn explained(root: &Path, file: &str) -> String {
    commands::explain(&config(root), &root.join(file))
        .unwrap()
        .text()
        .unwrap()
}

#[test]
fn go_reports_layer_violation() {
    let (_tmp, root) = fixture("go");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains("internal/handlers/books.go:6: handler -> repository is not allowed"),
        "{report}"
    );
    assert!(report.contains("1 violation(s)"), "{report}");
}

#[test]
fn ts_resolves_relative_alias_require_and_js_extension() {
    let (_tmp, root) = fixture("ts");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = json(&checked);
    let pairs: Vec<(String, String, String)> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().into(),
                v["to"].as_str().unwrap().into(),
                v["target"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            (
                "src/app/checkout.ts".into(),
                "infra".into(),
                "src/infra/db.ts".into()
            ),
            (
                "src/ui/legacy.js".into(),
                "infra".into(),
                "src/infra/db.ts".into()
            ),
        ]
    );
    assert_eq!(report["passed"], false);
    assert_eq!(report["stats"]["internal_deps"], 6);
}

#[test]
fn baseline_is_a_ratchet() {
    let (_tmp, root) = fixture("ts");
    let baseline = || commands::baseline(&config(&root), false, &mut cache()).unwrap();

    let recorded = baseline();
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("2 violation(s)"),
        "{}",
        recorded.text()
    );
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    assert!(text(&checked).contains("2 legalized"), "{}", text(&checked));

    // Paying off debt without shrinking the baseline fails the gate.
    let page = root.join("src/ui/legacy.js");
    let fixed = fs::read_to_string(&page)
        .unwrap()
        .replace("const db = require(\"../infra/db\");", "const db = 1;");
    fs::write(&page, fixed).unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("Stale baseline entries"),
        "{}",
        text(&checked)
    );

    let recorded = baseline();
    assert_eq!(recorded.outcome(), Outcome::Passed);
    assert!(
        recorded.text().contains("1 violation(s)"),
        "{}",
        recorded.text()
    );
    assert_eq!(check(&root).outcome(), Outcome::Passed);

    // New debt is never accepted silently.
    fs::write(
        root.join("src/domain/leak.ts"),
        "import { checkout } from \"../app/checkout\";\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("src/domain/leak.ts:1: domain -> app is not allowed"),
        "{}",
        text(&checked)
    );

    let checked = check_with(&root, |o| o.no_baseline = true);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("2 violation(s)"),
        "{}",
        text(&checked)
    );
}

#[test]
fn exclude_skips_files() {
    let (_tmp, root) = fixture("go");
    let config = fs::read_to_string(root.join("easement.yml")).unwrap();
    fs::write(
        root.join("easement.yml"),
        format!("{config}\nexclude: [internal/handlers]\n"),
    )
    .unwrap();
    // The violations of the handlers are gone with their files, and the
    // component left without a file is reported instead.
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(!report.contains("[dependency]"), "{report}");
    assert!(
        report.contains("component `handler` matches no file"),
        "{report}"
    );
}

#[test]
fn python_resolves_absolute_relative_and_from_imports() {
    let (_tmp, root) = fixture("python");
    let (pairs, stats) = violations(&root);
    assert_eq!(
        pairs,
        vec![
            (
                "src/shop/api/handlers.py".to_string(),
                "src/shop/infra/db.py".to_string()
            ),
            (
                "src/shop/api/views.py".to_string(),
                "src/shop/infra/__init__.py".to_string()
            ),
        ]
    );
    // handlers: shop.domain, shop.domain.order, shop.infra.db, .views; db: ..domain.order;
    // views: shop.infra
    assert_eq!(stats["internal_deps"], 6);
    // dataclasses
    assert_eq!(stats["external_deps"], 1);
}

#[test]
fn java_resolves_by_package_declaration() {
    let (_tmp, root) = fixture("java");
    let (pairs, stats) = violations(&root);
    assert_eq!(
        pairs,
        vec![(
            "src/main/java/com/acme/shop/web/OrderController.java".to_string(),
            "src/main/java/com/acme/shop/infra/OrderRepository.java".to_string()
        )]
    );
    // repository -> Order, controller -> Order (static), controller -> OrderRepository (infra.*)
    assert_eq!(stats["internal_deps"], 3);
}

/// The case of #104: `gw` may depend on `chunk` and `chunk` on `blob`, but `gw` not on `blob`.
pub(crate) const THREE: &str = "version: 1\ncomponents:\n  blob: { in: BLOB }\n  chunk: { in: CHUNK }\n  gw: { in: GW }\nrules:\n  gw: { may_depend_on: [chunk] }\n  chunk: { may_depend_on: [blob] }\n";

/// A Java project of the case of #104: `Gw` calls a method on what `Chunk.make()`
/// returns, `Wild` writes `new Block()` through a wildcard import, and `Qualified`
/// writes `new fx.blob.Block()`.
pub(crate) fn java_three() -> (TempDir, PathBuf) {
    let (tmp, root) = fixture("java");
    fs::remove_dir_all(root.join("src")).unwrap();
    let base = "src/main/java/fx";
    write_all(
        &root,
        &[
            (
                &format!("{base}/blob/Block.java"),
                "package fx.blob;\n\npublic class Block {\n    public int len;\n    public int size() { return len; }\n}\n",
            ),
            (
                &format!("{base}/chunk/Chunk.java"),
                "package fx.chunk;\n\nimport fx.blob.Block;\n\npublic class Chunk {\n    public static Block make() { return new Block(); }\n}\n",
            ),
            (
                &format!("{base}/gw/Gw.java"),
                "package fx.gw;\n\nimport fx.chunk.Chunk;\n\npublic class Gw {\n    public int run() {\n        var b = Chunk.make();\n        return b.size() + b.len;\n    }\n}\n",
            ),
            (
                &format!("{base}/gw/Wild.java"),
                "package fx.gw;\n\nimport fx.blob.*;\n\npublic class Wild {\n    public Object make() { return new Block(); }\n}\n",
            ),
            (
                &format!("{base}/gw/Qualified.java"),
                "package fx.gw;\n\npublic class Qualified {\n    public Object make() { return new fx.blob.Block(); }\n}\n",
            ),
        ],
    );
    let policy = THREE
        .replace("BLOB", &format!("{base}/blob"))
        .replace("CHUNK", &format!("{base}/chunk"))
        .replace("GW", &format!("{base}/gw"));
    fs::write(root.join("easement.yml"), policy).unwrap();
    (tmp, root)
}

#[test]
fn a_java_method_on_a_value_is_no_dependency_and_a_constructor_is() {
    let (_tmp, root) = java_three();
    let (found, _) = judged(&root);
    let blob = |file: &str, line| {
        (
            format!("src/main/java/fx/gw/{file}"),
            line,
            "dependency".to_string(),
            "gw -> blob".to_string(),
        )
    };
    assert_eq!(found, [blob("Qualified.java", 4), blob("Wild.java", 6)]);
}

/// The C# case of #104: `Chunk` makes a `Block` of `Blob`, `Gw` uses one through a
/// value, and `Marked` carries an attribute of `Blob`.
pub(crate) fn csharp_three() -> (TempDir, PathBuf) {
    let (tmp, root) = fixture("csharp");
    for dir in ["Domain", "Infra", "Web"] {
        fs::remove_dir_all(root.join(dir)).unwrap();
    }
    write_all(
        &root,
        &[
            (
                "Blob/Block.cs",
                "namespace Fx.Blob;\n\npublic class Block {\n    public int Len;\n    public int Size() { return Len; }\n}\n\npublic class MarkAttribute : System.Attribute {}\n",
            ),
            (
                "Chunk/Chunk.cs",
                "using Fx.Blob;\n\nnamespace Fx.Chunk;\n\npublic class Chunk {\n    public static Block Make() { return new Block(); }\n}\n",
            ),
            (
                "Gw/Gw.cs",
                "using Fx.Chunk;\n\nnamespace Fx.Gw;\n\npublic class Gw {\n    public int Run() {\n        var b = Chunk.Chunk.Make();\n        return b.Size() + b.Len;\n    }\n}\n",
            ),
            (
                "Gw/Marked.cs",
                "namespace Fx.Gw;\n\n[Fx.Blob.Mark]\npublic class Marked {}\n",
            ),
        ],
    );
    let policy = THREE
        .replace("BLOB", "Blob")
        .replace("CHUNK", "Chunk")
        .replace("GW", "Gw");
    fs::write(root.join("easement.yml"), policy).unwrap();
    (tmp, root)
}

#[test]
fn a_csharp_attribute_is_a_dependency_on_its_class_and_a_member_on_a_value_is_not() {
    let (_tmp, root) = csharp_three();
    let (found, report) = judged(&root);
    assert_eq!(
        found,
        [(
            "Gw/Marked.cs".to_string(),
            3,
            "dependency".to_string(),
            "gw -> blob".to_string()
        )],
        "{report}"
    );
}

/// A Cargo workspace of the case of #104: `chunk` re-exports `blob::Block`.
fn rust_three(gw: &str) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let manifest = |name: &str, deps: &str| {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
        )
    };
    write_all(
        &root,
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"gw\", \"chunk\", \"blob\"]\nresolver = \"2\"\n",
            ),
            ("blob/Cargo.toml", &manifest("blob", "")),
            (
                "blob/src/lib.rs",
                "pub struct Block {\n    pub len: usize,\n}\n\nimpl Block {\n    pub fn size(&self) -> usize {\n        self.len\n    }\n}\n",
            ),
            (
                "chunk/Cargo.toml",
                &manifest("chunk", "blob = { path = \"../blob\" }\n"),
            ),
            (
                "chunk/src/lib.rs",
                "pub use blob::Block;\n\npub fn make() -> blob::Block {\n    blob::Block { len: 1 }\n}\n",
            ),
            (
                "gw/Cargo.toml",
                &manifest("gw", "chunk = { path = \"../chunk\" }\n"),
            ),
            ("gw/src/lib.rs", gw),
        ],
    );
    let policy = THREE
        .replace("BLOB", "blob")
        .replace("CHUNK", "chunk")
        .replace("GW", "gw");
    fs::write(root.join("easement.yml"), policy).unwrap();
    (tmp, root)
}

#[test]
fn a_rust_method_on_a_value_is_no_dependency_and_a_re_exported_name_is() {
    let (_tmp, root) = rust_three(
        "pub fn run() -> usize {\n    let b = chunk::make();\n    b.size() + b.len\n}\n",
    );
    let (found, report) = judged(&root);
    assert_eq!(found, [], "{report}");
    let (_tmp, root) = rust_three(
        "pub fn run() -> usize {\n    chunk::make().len\n}\n\npub fn named() -> chunk::Block {\n    chunk::make()\n}\n",
    );
    let (found, report) = judged(&root);
    assert_eq!(
        found,
        [(
            "gw/src/lib.rs".to_string(),
            5,
            "dependency".to_string(),
            "gw -> blob".to_string()
        )],
        "{report}"
    );
}

#[test]
fn a_typescript_name_re_exported_by_a_barrel_is_a_dependency_on_its_component() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    write_all(
        &root,
        &[
            (
                "tsconfig.json",
                "{ \"compilerOptions\": { \"strict\": true }, \"include\": [\"src\"] }\n",
            ),
            (
                "src/blob/index.ts",
                "export class Block {\n  len = 1;\n  size(): number {\n    return this.len;\n  }\n}\n",
            ),
            (
                "src/chunk/index.ts",
                "import { Block } from \"../blob\";\n\nexport { Block } from \"../blob\";\n\nexport const make = (): Block => new Block();\n",
            ),
            (
                "src/gw/run.ts",
                "import { make } from \"../chunk\";\n\nexport const run = () => make().size() + make().len;\n",
            ),
            (
                "src/gw/named.ts",
                "import { Block } from \"../chunk\";\n\nexport const named = (b: Block) => b;\n",
            ),
        ],
    );
    let policy = THREE
        .replace("BLOB", "src/blob")
        .replace("CHUNK", "src/chunk")
        .replace("GW", "src/gw");
    fs::write(root.join("easement.yml"), policy).unwrap();
    let (found, report) = judged(&root);
    assert_eq!(
        found,
        [(
            "src/gw/named.ts".to_string(),
            1,
            "dependency".to_string(),
            "gw -> blob".to_string()
        )],
        "{report}"
    );
}

#[test]
fn a_php_function_or_constant_brought_in_by_use_is_a_dependency_on_its_file() {
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
                "<?php\nnamespace App\\Infra;\n\nconst POOL = 2;\n\nfunction connect(): int { return POOL; }\n",
            ),
            (
                "src/Web/Status.php",
                "<?php\nnamespace App\\Web;\n\nuse function App\\Infra\\connect;\nuse const App\\Infra\\POOL;\n\nclass Status { public function up(): int { return connect() + POOL; } }\n",
            ),
        ],
    );
    composer_install(&root);
    let (found, report) = judged(&root);
    let infra = |file: &str, line| {
        (
            format!("src/Web/{file}"),
            line,
            "dependency".to_string(),
            "web -> infra".to_string(),
        )
    };
    assert_eq!(
        found,
        [infra("OrderController.php", 5), infra("Status.php", 4)],
        "{report}"
    );
}

/// Every layered fixture has one forbidden import, web -> infra, and allowed ones
/// that must be seen: the count of internal dependencies proves the adapter resolved them.
fn layered(name: &str, file: &str, target: &str, internal_deps: u64) {
    if name == "ruby" && no_scip_ruby_here() {
        return;
    }
    let (_tmp, root) = fixture(name);
    if name == "rust" {
        // Not stored in the fixture: tools that walk the repository would take it for a crate.
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
    }
    if name == "php" {
        composer_install(&root);
    }
    let (found, stats) = violations(&root);
    assert_eq!(
        found,
        vec![(file.to_string(), target.to_string())],
        "{name}"
    );
    assert_eq!(stats["internal_deps"], internal_deps, "{name}");
}

#[test]
fn rust_resolves_the_module_tree() {
    // `use super::*` in the inline test module adds nothing; `use std::fmt` is external.
    layered("rust", "src/web/mod.rs", "src/infra/repo.rs", 8);
}

/// A crate's unit tests sit in the file they test, and the dev-dependency they use
/// is not a dependency of the crate: only the line outside `#[cfg(test)]` is judged
/// as the file's.
fn rust_cfg_test_fixture() -> (TempDir, PathBuf) {
    let (tmp, root) = fixture("rust_cfg_test");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dev-dependencies]\ntestkit = { path = \"testkit\" }\n\n[dependencies]\ntestkit = { path = \"testkit\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("testkit/Cargo.toml"),
        "[package]\nname = \"testkit\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    (tmp, root)
}

#[test]
fn rust_cfg_test_code_is_tests_not_the_file_it_sits_in() {
    let (_tmp, root) = rust_cfg_test_fixture();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains(
            "src/store/mod.rs:1: store -> kit is not allowed (written as testkit/src/lib.rs)"
        ),
        "{report}"
    );
    assert!(!report.contains("src/store/ok.rs"), "{report}");
    assert!(report.contains("1 violation(s)"), "{report}");
}

/// A `#[cfg(test)] mod name;` is test code as a whole: its file is `tests`, not the
/// component of the file that declares it.
#[test]
fn a_cfg_test_module_in_another_file_is_tests_too() {
    let (_tmp, root) = rust_cfg_test_fixture();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(!report.contains("src/store/extra.rs"), "{report}");
    // `inner.rs` opens with `#![cfg(test)]`: all of it is test code.
    assert!(!report.contains("src/store/inner.rs"), "{report}");
    assert!(report.contains("1 violation(s)"), "{report}");
}

/// A module that test code declares exists only in a test build, however plainly it
/// is declared: `extra.rs` declares `mod tests;`, which declares `mod deeper;` (a
/// `tests/` under `src/`, so no glob takes it), and the `#![cfg(test)]` file `inner.rs`
/// declares `mod support;`. A plain module of the crate's own code stays its component's.
#[test]
fn a_module_that_test_code_declares_is_tests_all_the_way_down() {
    let (_tmp, root) = rust_cfg_test_fixture();
    let declaring = root.join("src/store/mod.rs");
    let source = fs::read_to_string(&declaring).unwrap();
    fs::write(&declaring, format!("{source}\nmod plain;\n")).unwrap();
    fs::write(
        root.join("src/store/plain.rs"),
        "use testkit::Clock;\n\npub fn p() {\n    let _ = Clock;\n}\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(!report.contains("src/store/extra/tests.rs"), "{report}");
    assert!(
        !report.contains("src/store/extra/tests/deeper.rs"),
        "{report}"
    );
    assert!(!report.contains("src/store/inner/support.rs"), "{report}");
    assert!(
        report.contains("src/store/plain.rs:1: store -> kit is not allowed"),
        "{report}"
    );
    assert!(report.contains("2 violation(s)"), "{report}");
}

/// A module that test code declares but whose file easement cannot find is said too,
/// naming the declaring file once however many of its modules are missing.
#[test]
fn a_module_that_test_code_declares_whose_file_is_missing_is_flagged_as_not_seen() {
    let (_tmp, root) = rust_cfg_test_fixture();
    fs::remove_file(root.join("src/store/extra/tests/deeper.rs")).unwrap();
    let declaring = root.join("src/store/extra/tests.rs");
    let source = fs::read_to_string(&declaring).unwrap();
    fs::write(&declaring, format!("mod gone;\n{source}")).unwrap();
    let report = json(&check(&root));
    assert!(leaves_out(&report), "{report}");
    let partial = gap_messages(&report, "parts");
    assert_eq!(partial.len(), 1, "{partial:?}");
    let reason = partial[0].as_str().unwrap();
    assert_eq!(
        reason.matches("src/store/extra/tests.rs").count(),
        1,
        "{reason}"
    );
    let gap = &report["evidence_gaps"][0];
    assert_eq!(gap["kind"], "parts", "{gap}");
    assert_eq!(
        gap["files"],
        serde_json::json!(["src/store/extra/tests.rs"])
    );
    assert_eq!(gap["count"], 1);
}

/// Cargo's integration tests sit in `tests/` beside the manifest; a module named
/// `tests` under `src/` is the crate's own code unless `#[cfg(test)]` says otherwise.
#[test]
fn rust_integration_tests_are_tests_and_a_tests_module_in_src_is_not() {
    let (_tmp, root) = rust_cfg_test_fixture();
    fs::create_dir_all(root.join("tests/common")).unwrap();
    fs::write(root.join("tests/api.rs"), "mod common;\n").unwrap();
    fs::write(root.join("tests/common/mod.rs"), "").unwrap();
    fs::create_dir_all(root.join("src/store/tests")).unwrap();
    fs::write(root.join("src/store/tests/fixtures.rs"), "").unwrap();
    for file in ["tests/api.rs", "tests/common/mod.rs"] {
        let said = explained(&root, file);
        assert!(said.contains("component: tests"), "{file}: {said}");
    }
    let said = explained(&root, "src/store/tests/fixtures.rs");
    assert!(said.contains("component: store"), "{said}");
}

/// Benchmarks and examples are Cargo targets built with the dev-dependencies, like
/// integration tests; a `benches` or `examples` module under `src/` is the crate's.
#[test]
fn rust_benches_and_examples_are_tests_and_modules_of_that_name_in_src_are_not() {
    let (_tmp, root) = rust_cfg_test_fixture();
    let targets = [
        "benches/load.rs",
        "examples/hello.rs",
        "examples/demo/main.rs",
    ];
    let modules = ["src/store/benches/data.rs", "src/store/examples/data.rs"];
    for file in targets.iter().chain(&modules) {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            "use testkit::Clock;\n\nfn main() {\n    let _ = Clock;\n}\n",
        )
        .unwrap();
    }
    // The dev-dependency they use is one of `tests`: only the fixture's own violation is left.
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert!(
        text(&checked).contains("1 violation(s)"),
        "{}",
        text(&checked)
    );
    for file in targets {
        let said = explained(&root, file);
        assert!(said.contains("component: tests"), "{file}: {said}");
    }
    for file in modules {
        let said = explained(&root, file);
        assert!(said.contains("component: store"), "{file}: {said}");
    }
}

/// A module file easement cannot find is said, not silently judged as the file's.
#[test]
fn a_cfg_test_module_whose_file_is_missing_is_flagged_as_not_seen() {
    let (_tmp, root) = rust_cfg_test_fixture();
    fs::remove_file(root.join("src/store/extra.rs")).unwrap();
    let report = json(&check(&root));
    let partial = gap_messages(&report, "parts");
    assert_eq!(partial.len(), 1, "{partial:?}");
    assert!(partial[0].as_str().unwrap().contains("src/store/mod.rs"));
}

#[test]
fn csharp_resolves_namespaces() {
    layered(
        "csharp",
        "Web/OrderController.cs",
        "Infra/OrderRepository.cs",
        3,
    );
}

#[test]
fn ruby_resolves_constants() {
    layered(
        "ruby",
        "lib/shop/web/order_controller.rb",
        "lib/shop/infra/order_repository.rb",
        3,
    );
}

#[test]
fn kotlin_resolves_packages_and_declarations() {
    layered(
        "kotlin",
        "src/main/kotlin/com/acme/shop/web/OrderController.kt",
        "src/main/kotlin/com/acme/shop/infra/OrderRepository.kt",
        3,
    );
}

#[test]
fn php_resolves_psr4_names() {
    layered(
        "php",
        "src/Web/OrderController.php",
        "src/Infra/OrderRepository.php",
        3,
    );
}

#[test]
fn svelte_resolves_relative_and_subpath_imports() {
    layered(
        "svelte",
        "src/routes/web/OrderController.svelte",
        "src/lib/infra/OrderRepository.svelte",
        3,
    );
}

#[test]
fn svelte_components_imported_from_typescript_are_dependencies() {
    // The TypeScript index points the import at the ambient `*.svelte` module,
    // not at the component, so the adapter adds the dependency itself.
    let (_tmp, root) = fixture("svelte");
    write_all(
        &root,
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "strict": true, "moduleResolution": "bundler", "module": "esnext" }, "include": ["src"] }"#,
            ),
            ("src/svelte.d.ts", "declare module '*.svelte';\n"),
            (
                "src/lib/domain/order.ts",
                "import Controller from '../../routes/web/OrderController.svelte';\nexport const c = Controller;\n",
            ),
        ],
    );
    let (found, _) = violations(&root);
    assert!(
        found.contains(&(
            "src/lib/domain/order.ts".to_string(),
            "src/routes/web/OrderController.svelte".to_string()
        )),
        "{found:?}"
    );
}

#[test]
fn svelte_lib_resolves_without_the_generated_config() {
    // A fresh checkout: `.svelte-kit/` is not generated yet, but `$lib` is
    // SvelteKit's own default.
    let (_tmp, root) = fixture("svelte");
    write_all(
        &root,
        &[
            (
                "package.json",
                r##"{ "name": "shop", "private": true, "imports": { "#lib/*": "./src/lib/*" }, "devDependencies": { "@sveltejs/kit": "^3.0.0" } }"##,
            ),
            (
                "src/routes/web/OrderController.svelte",
                "<script>\n  import OrderRepository from '$lib/infra/OrderRepository.svelte';\n</script>\n",
            ),
        ],
    );
    let (found, _) = violations(&root);
    assert_eq!(
        found,
        [(
            "src/routes/web/OrderController.svelte".to_string(),
            "src/lib/infra/OrderRepository.svelte".to_string()
        )]
    );
}

#[test]
fn svelte_local_import_that_names_no_file_is_an_evidence_gap() {
    // A `kit.alias` whose generated config is missing resolves to nothing: it is
    // not an external module, and the check says it could not judge it.
    let (_tmp, root) = fixture("svelte");
    write_all(
        &root,
        &[(
            "src/routes/web/OrderController.svelte",
            "<script>\n  import Repo from '$infra/OrderRepository.svelte';\n  import { page } from '$app/state';\n  import type { PageData } from './$types';\n</script>\n",
        )],
    );
    let checked = check(&root);
    let report = json(&checked);
    let gaps = gap_messages(&report, "unresolved_imports");
    assert_eq!(gaps.len(), 1, "{report}");
    let message = gaps[0].as_str().unwrap();
    assert!(
        message.contains("$infra/OrderRepository.svelte"),
        "{message}"
    );
    assert!(
        !message.contains("$app/state") && !message.contains("$types"),
        "{message}"
    );
}

#[test]
fn swift_resolves_package_modules() {
    layered(
        "swift",
        "Sources/Web/OrderController.swift",
        "Sources/Infra",
        3,
    );
}
