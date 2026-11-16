//! The `unused` rule: a file no `entry` reaches, followed through what each
//! language loads without an import, and what it cannot judge.

use std::fs;
use std::path::Path;

use easement::commands::Outcome;
use serde_json::Value;

use crate::common::{fixture, project};
use crate::run::{check, composer_install, config, gap_messages, json, pinned, report_of, text};

/// The unused-rule verdicts and gaps of the check of `root`, as data.
fn unused_of(root: &Path) -> (Vec<Value>, Vec<Value>) {
    let report = json(&check(root));
    let of = |key: &str, field: &str, value: &str| {
        report[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item[field] == value)
            .cloned()
            .collect::<Vec<_>>()
    };
    (
        of("violations", "rule", "unused"),
        of("evidence_gaps", "kind", "unused"),
    )
}

/// scip-go resolves one side of an import cycle only (#113): the import it
/// dropped may reach a file no other chain reaches, so `unused` does not call
/// that file unused, and its gap names the import.
#[test]
fn a_file_an_unresolved_import_may_reach_is_not_judged_unused() {
    let (_tmp, root) = fixture("go_import_cycle");
    fs::write(
        config(&root),
        "version: 1\ncomponents:\n  a: { in: a }\n  b: { in: b }\nentry: [a/a.go]\n",
    )
    .unwrap();
    let (violations, gaps) = unused_of(&root);
    assert_eq!(violations, [] as [Value; 0]);
    pinned(|| insta::assert_json_snapshot!("unused_unresolved_import_gap", gaps));

    // From `b`, the resolved import reaches `a`: every file is judged.
    fs::write(
        config(&root),
        "version: 1\ncomponents:\n  a: { in: a }\n  b: { in: b }\nentry: [b/b.go]\n",
    )
    .unwrap();
    assert_eq!(unused_of(&root), (vec![], vec![]));
}

/// A TypeScript project of `files` whose `src/gen` the checks do not read, as
/// `unread` (`target_only` or `exclude`) says. `src/main.ts` is the entry and
/// reaches nothing.
fn with_unread_gen(
    unread: &str,
    files: &[(&str, &str)],
) -> (tempfile::TempDir, std::path::PathBuf) {
    let policy = format!(
        "version: 1\ncomponents:\n  app: {{ in: src/app }}\nentry: [src/main.ts]\n{unread}: [src/gen]\n"
    );
    let mut all = vec![
        ("easement.yml", policy.as_str()),
        ("src/main.ts", "console.log(1);\n"),
    ];
    all.extend_from_slice(files);
    project(&all)
}

/// A file only an unread file's unresolved import may reach is not judged
/// unused, whether `target_only` or `exclude` keeps that file from the checks;
/// the import is still no gap of `check`.
#[test]
fn a_file_an_unread_files_unresolved_import_may_reach_is_not_judged_unused() {
    for unread in ["target_only", "exclude"] {
        let (_tmp, root) = with_unread_gen(
            unread,
            &[
                (
                    "src/gen/gen.ts",
                    "import { gone } from \"../app/missing\";\nexport const g = gone;\n",
                ),
                ("src/app/maybe.ts", "export const maybe = 1;\n"),
            ],
        );
        let (violations, gaps) = unused_of(&root);
        assert_eq!(violations, [] as [Value; 0], "{unread}");
        pinned(|| insta::assert_json_snapshot!("unused_unread_unresolved_import_gap", gaps));
        assert_eq!(
            gap_messages(&json(&check(&root)), "unresolved_imports"),
            [] as [Value; 0],
            "{unread}"
        );
    }
}

/// What an unread file imports reaches a file as a checked file's import does:
/// a path its text names that the index left unresolved (a `?raw` import a
/// wildcard declaration catches), and a side-effect import, which `unused`
/// follows itself. Neither holds the verdict, so a dead file is still unused.
#[test]
fn an_unread_files_imports_the_index_left_unresolved_still_reach_files() {
    for unread in ["target_only", "exclude"] {
        let (_tmp, root) = with_unread_gen(
            unread,
            &[
                (
                    "src/shims.d.ts",
                    "declare module \"*?raw\" { const src: string; export default src; }\n",
                ),
                (
                    "src/gen/gen.ts",
                    "import raw from \"../app/raw.ts?raw\";\nimport \"../app/effect\";\nexport const g = raw;\n",
                ),
                ("src/app/raw.ts", "export const raw = 1;\n"),
                ("src/app/effect.ts", "(globalThis as any).x = 1;\n"),
                ("src/app/dead.ts", "export const dead = 1;\n"),
            ],
        );
        let (violations, gaps) = unused_of(&root);
        let unused: Vec<&Value> = violations.iter().map(|v| &v["file"]).collect();
        assert_eq!(unused, ["src/app/dead.ts"], "{unread}");
        assert_eq!(gaps, [] as [Value; 0], "{unread}");
        assert_eq!(
            gap_messages(&json(&check(&root)), "unresolved_imports"),
            [] as [Value; 0],
            "{unread}"
        );
    }
}

/// A side-effect import between two excluded files is followed too: it holds
/// no verdict, so a dead file is still unused.
#[test]
fn a_side_effect_import_between_excluded_files_holds_no_verdict() {
    let (_tmp, root) = with_unread_gen(
        "exclude",
        &[
            (
                "src/gen/gen.ts",
                "import \"./setup\";\nexport const g = 1;\n",
            ),
            ("src/gen/setup.ts", "(globalThis as any).x = 1;\n"),
            ("src/app/dead.ts", "export const dead = 1;\n"),
        ],
    );
    let (violations, gaps) = unused_of(&root);
    let unused: Vec<&Value> = violations.iter().map(|v| &v["file"]).collect();
    assert_eq!(unused, ["src/app/dead.ts"]);
    assert_eq!(gaps, [] as [Value; 0]);
}

#[test]
fn a_file_no_entry_reaches_is_unused() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\nentry: [src/main.ts]\n",
        ),
        (
            "src/main.ts",
            "import { a } from \"./app/a\";\nconsole.log(a);\n",
        ),
        (
            "src/app/a.ts",
            "import { b } from \"./b\";\nexport const a = b;\n",
        ),
        ("src/app/b.ts", "export const b = 1;\n"),
        ("src/app/dead.ts", "export const dead = 1;\n"),
        // Used by a test only: still used.
        ("src/app/helper.ts", "export const helper = 1;\n"),
        (
            "src/app/a.test.ts",
            "import { helper } from \"./helper\";\nexport const t = helper;\n",
        ),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains(
            "src/app/dead.ts:1: not reachable from any `entry`; if a framework, DI, reflection or a build tool loads it, add it to `entry` [unused]",
        ),
        "{said}"
    );
    assert!(!said.contains("src/app/b.ts:1"), "{said}");
    assert!(!said.contains("src/app/helper.ts:1"), "{said}");
}

#[test]
fn unused_follows_go_packages_and_python_packages() {
    let (_tmp, root) = project(&[
        ("go.mod", "module ex.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\n  dead: { in: dead }\nentry: [cmd]\n",
        ),
        (
            "cmd/main.go",
            "package main\n\nimport \"ex.com/m/app\"\n\nfunc main() { app.A() }\n",
        ),
        (
            "app/a.go",
            "package app\n\nfunc A() int { return helper() }\n",
        ),
        // Used from a.go without an import: Go compiles the package whole.
        (
            "app/b.go",
            "package app\n\nfunc helper() int { return 1 }\n",
        ),
        ("dead/d.go", "package dead\n\nfunc D() {}\n"),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains("dead/d.go:1: not reachable from any `entry`"),
        "{said}"
    );
    assert!(!said.contains("app/b.go:1"), "{said}");
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\nentry: [main.py]\n",
        ),
        ("main.py", "from app.a import a\nprint(a())\n"),
        ("app/__init__.py", ""),
        ("app/a.py", "def a():\n    return 1\n"),
        ("app/dead.py", "def dead():\n    return 2\n"),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains("app/dead.py:1: not reachable from any `entry`"),
        "{said}"
    );
    assert!(!said.contains("app/__init__.py:1"), "{said}");
    // Kotlin sees only imports, so the rule does not judge it, and says so.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\nentry: [src/Main.kt]\n",
        ),
        ("src/Main.kt", "package main\nclass Main\n"),
        ("src/app/A.kt", "package app\nclass A\n"),
    ]);
    let said = report_of(&root, Outcome::Passed);
    assert!(said.contains("`unused` cannot judge .kt files"), "{said}");
}

#[test]
fn unused_counts_what_tools_and_conventions_load() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\n  gen: { in: src/gen }\nentry: [src/main.ts]\ntarget_only: [src/gen]\n",
        ),
        (
            "src/main.ts",
            "import \"./app/polyfill\";\nimport { routes } from \"./gen/routes\";\nconsole.log(routes);\n",
        ),
        // Run for its side effect only.
        ("src/app/polyfill.ts", "(globalThis as any).x = 1;\n"),
        // Used only by generated code.
        (
            "src/gen/routes.ts",
            "import { page } from \"../app/page\";\nexport const routes = [page];\n",
        ),
        ("src/app/page.ts", "export const page = 1;\n"),
        // Ambient declarations and a package's own `bin`.
        ("src/app/globals.d.ts", "declare const VERSION: string;\n"),
        ("src/app/esm.d.mts", "declare const ESM: string;\n"),
        ("src/app/cjs.d.cts", "declare const CJS: string;\n"),
        ("src/app/cli.ts", "console.log(1);\n"),
        (
            "package.json",
            "{\"name\": \"m\", \"bin\": {\"m\": \"src/app/cli.ts\"}}\n",
        ),
        ("src/app/dead.ts", "export const dead = 1;\n"),
    ]);
    let out = report_of(&root, Outcome::Failed);
    // The index holds nothing of the side-effect import, but `unused` follows
    // it itself: it is no lost import, and `dead.ts` is still judged.
    assert!(
        out.contains("src/main.ts:1 (`./app/polyfill`)"),
        "the import is an unresolved one: {out}"
    );
    assert!(
        out.contains("src/app/dead.ts:1: not reachable from any `entry`"),
        "{out}"
    );
    for used in [
        "polyfill.ts",
        "page.ts",
        "globals.d.ts",
        "esm.d.mts",
        "cjs.d.cts",
        "cli.ts",
    ] {
        assert!(!out.contains(&format!("src/app/{used}:1")), "{used}: {out}");
    }

    // A stale `entry` would make everything unused: it fails as `unmatched`.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\nentry: [src/old_main.ts]\n",
        ),
        ("src/app/a.ts", "export const a = 1;\n"),
    ]);
    let said = report_of(&root, Outcome::Failed);
    assert!(
        said.contains("easement.yml:4: `entry` pattern `src/old_main.ts` matches no file"),
        "{said}"
    );
}

#[test]
fn unused_knows_go_programs_python_conventions_and_kotlin_beside_java() {
    let (_tmp, root) = project(&[
        ("go.mod", "module ex.com/m\n\ngo 1.22\n"),
        (
            "easement.yml",
            "version: 1\ncomponents:\n  tools: { in: tools }\nentry: [cmd]\n",
        ),
        ("cmd/main.go", "package main\n\nfunc main() {}\n"),
        // A second program, outside `entry`.
        ("tools/gen/main.go", "package main\n\nfunc main() {}\n"),
    ]);
    report_of(&root, Outcome::Passed);
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: app }\nentry: [main.py]\n",
        ),
        ("main.py", "print(1)\n"),
        ("app/conftest.py", "import pytest\n"),
        ("app/__main__.py", "print(2)\n"),
    ]);
    report_of(&root, Outcome::Passed);
    // Kotlin uses Java classes of its package without an import.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\nentry: [src/Main.kt]\n",
        ),
        ("src/Main.kt", "package app\nfun main() { Helper() }\n"),
        (
            "src/app/Helper.java",
            "package app;\npublic class Helper {}\n",
        ),
    ]);
    let said = report_of(&root, Outcome::Passed);
    assert!(
        said.contains("`unused` did not judge the java files: Kotlin beside it"),
        "{said}"
    );
}

/// The files that may declare an imported name are known to the languages an
/// adapter reads, and `unused` judges the languages an indexer covers: the
/// alternative declaration of a Kotlin file is never called unused.
#[test]
fn unused_never_calls_an_alternative_declaration_unused() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\nentry: [src/app/Main.kt]\n",
        ),
        (
            "src/app/Main.kt",
            "package app\n\nimport app.thing.Thing\n\nfun main() { Thing() }\n",
        ),
        (
            "src/app/thing/Thing.kt",
            "package app.thing\n\nclass Thing\n",
        ),
        (
            "src/app/other/Thing.kt",
            "package app.thing\n\nclass Thing\n",
        ),
    ]);
    let checked = check(&root);
    let report = json(&checked);
    assert!(
        report["violations"].as_array().unwrap().is_empty(),
        "{report}"
    );
    let said = text(&checked);
    assert!(said.contains("`unused` cannot judge .kt files"), "{said}");
}

/// C# and PHP: a class of the same namespace used without an import is
/// reached; a class nothing uses is not. Needs scip-dotnet and scip-php.
#[test]
fn unused_follows_csharp_and_php_namespaces() {
    let (_tmp, root) = fixture("csharp");
    fs::write(
        config(&root),
        "version: 1\ncomponents:\n  web: { in: Web }\n  infra: { in: Infra }\n  domain: { in: Domain }\nentry: [Web/OrderController.cs]\n",
    )
    .unwrap();
    fs::write(
        root.join("Infra/OrderRepository.cs"),
        "using Shop.Domain;\n\nnamespace Shop.Infra;\n\npublic class OrderRepository { Order order; Helper helper; }\n",
    )
    .unwrap();
    fs::write(
        root.join("Infra/Helper.cs"),
        "namespace Shop.Infra;\n\npublic class Helper {}\n",
    )
    .unwrap();
    fs::write(
        root.join("Infra/Dead.cs"),
        "namespace Shop.Infra;\n\npublic class Dead {}\n",
    )
    .unwrap();
    let out = report_of(&root, Outcome::Failed);
    assert!(
        out.contains("Infra/Dead.cs:1: not reachable from any `entry`"),
        "{out}"
    );
    assert!(!out.contains("Infra/Helper.cs:1"), "{out}");

    let (_tmp, root) = fixture("php");
    composer_install(&root);
    fs::write(
        config(&root),
        "version: 1\ncomponents:\n  web: { in: src/Web }\n  infra: { in: src/Infra }\n  domain: { in: src/Domain }\nentry: [src/Web/OrderController.php]\n",
    )
    .unwrap();
    fs::write(
        root.join("src/Infra/OrderRepository.php"),
        "<?php\nnamespace App\\Infra;\n\nuse App\\Domain\\Order;\n\nclass OrderRepository { public function find(Order $o): Helper { return new Helper(); } }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/Infra/Helper.php"),
        "<?php\nnamespace App\\Infra;\n\nclass Helper {}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/Infra/Dead.php"),
        "<?php\nnamespace App\\Infra;\n\nclass Dead {}\n",
    )
    .unwrap();
    let out = report_of(&root, Outcome::Failed);
    assert!(
        out.contains("src/Infra/Dead.php:1: not reachable from any `entry`"),
        "{out}"
    );
    assert!(!out.contains("src/Infra/Helper.php:1"), "{out}");
}

/// The files of the project `unused` reports, by path.
fn unused_files(root: &Path) -> Vec<String> {
    unused_of(root)
        .0
        .iter()
        .map(|v| v["file"].as_str().unwrap().to_string())
        .collect()
}

/// A package that depends on `@sveltejs/kit` loads its routes, hooks and param
/// matchers by convention: none is unused, though nothing imports them.
#[test]
fn sveltekit_routes_hooks_and_params_are_entries() {
    let (_tmp, root) = fixture("sveltekit");
    assert_eq!(unused_files(&root), ["src/lib/x.ts"]);
    // The report names the framework it recognised, and where.
    let checked = check(&root);
    pinned(|| insta::assert_json_snapshot!("unused_frameworks", json(&checked)["frameworks"]));
    let said = text(&checked);
    let section = said
        .split("\n\n")
        .find(|part| part.starts_with("Frameworks"))
        .unwrap_or_else(|| panic!("{said}"));
    pinned(|| insta::assert_snapshot!("unused_frameworks_text", section));
}

/// A package that depends on `next` loads its `app/` and `pages/` routes, at its
/// root or under `src/`, its middleware, instrumentation and config.
#[test]
fn nextjs_routes_are_entries_at_the_root_and_under_src() {
    let (_tmp, root) = fixture("nextjs");
    assert_eq!(unused_files(&root), ["apps/admin/src/lib/x.ts", "lib/x.ts"]);
}

/// A framework's conventions hold in the package that depends on it alone: a
/// sibling with the same layout is judged.
#[test]
fn a_framework_holds_in_its_own_package_of_a_monorepo() {
    let (_tmp, root) = fixture("monorepo_frameworks");
    assert_eq!(
        unused_files(&root),
        [
            "apps/api/src/hooks.server.ts",
            "apps/api/src/routes/+page.ts"
        ]
    );
}

/// A config file that may move a framework's conventions (SvelteKit's
/// `kit.files`, Next.js's `pageExtensions`), and a `package.json` that is no
/// JSON, leave the files of the package unjudged: none is reported, and the gap
/// says why.
#[test]
fn a_moved_convention_or_a_broken_manifest_leaves_the_package_unjudged() {
    let (_tmp, root) = fixture("sveltekit");
    fs::write(
        root.join("svelte.config.js"),
        "const config = {\n  kit: {\n    files: { routes: \"src/pages\" },\n  },\n};\n\nexport default config;\n",
    )
    .unwrap();
    let (violations, gaps) = unused_of(&root);
    assert_eq!(violations, [] as [Value; 0]);
    pinned(|| insta::assert_json_snapshot!("unused_moved_convention_gap", gaps));

    let (_tmp, root) = fixture("nextjs");
    fs::write(
        root.join("apps/admin/next.config.ts"),
        "export default { pageExtensions: [\"page.tsx\"] };\n",
    )
    .unwrap();
    // The site's own conventions still hold.
    assert_eq!(unused_files(&root), ["lib/x.ts"]);
    let messages = gap_messages(&json(&check(&root)), "unused");
    assert_eq!(
        messages,
        [
            "`unused` did not judge the files of apps/admin/package.json, which uses Next.js: apps/admin/next.config.ts may set `pageExtensions`, which moves the files it loads, so it took every file of the package as used"
        ]
    );

    let (_tmp, root) = fixture("monorepo_frameworks");
    fs::write(root.join("apps/api/package.json"), "{ \"name\": \"api\",\n").unwrap();
    let (violations, gaps) = unused_of(&root);
    assert_eq!(violations, [] as [Value; 0]);
    pinned(|| insta::assert_json_snapshot!("unused_broken_manifest_gap", gaps));
}
