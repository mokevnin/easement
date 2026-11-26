//! TypeScript and JavaScript beyond their index: how the bundlers resolve a
//! relative specifier, and the uses of a file that an index may not report.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use super::language::module_path_in;
use super::language::{Explained, Language, Spelled, UnindexedUse, UnrelatedSpan};
use super::{Cause, Form, Import};
use crate::packages::npm::{self, npm_name};
use crate::relpath::{self, parent};
use crate::scip::symbol::{self, Installs, Kind, Parsed, Qualifier};

/// The extensions of its files.
pub const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// The suffixes of its declaration files (`a.d.ts`).
pub const DECLARATIONS: &[&str] = &["d.ts", "d.mts", "d.cts"];

/// Where its package managers install third-party packages.
pub const THIRD_PARTY: &[&str] = &["node_modules"];

/// The file a relative specifier names, as the bundlers resolve it: the path
/// itself, with an extension, or its index.
pub fn resolve(from_dir: &str, spec: &str, files: &BTreeSet<&str>) -> Option<String> {
    let base = relpath::resolve(from_dir, spec)?;
    let exts: Vec<&str> = EXTENSIONS.iter().copied().chain(["d.ts"]).collect();
    std::iter::once(base.clone())
        .chain(exts.iter().map(|e| format!("{base}.{e}")))
        .chain(exts.iter().map(|e| format!("{base}/index.{e}")))
        .find(|candidate| files.contains(candidate.as_str()))
}

/// The files `file` reaches through an import for its side effect
/// (`import "./polyfill"`), `require` and a literal `import()` of a relative
/// path, which an index may not report as a use.
pub fn unindexed_uses(text: &str, file: &str, files: &BTreeSet<&str>) -> Vec<UnindexedUse> {
    static RELATIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?m)(?:^\s*import\s+|\brequire\s*\(\s*|\bimport\s*\(\s*)["'](\.{1,2}/[^"']+)["']"#,
        )
        .expect("valid regex")
    });
    RELATIVE
        .captures_iter(text)
        .filter_map(|capture| {
            let specifier = capture.get(1)?;
            Some(UnindexedUse {
                line: text[..specifier.start()].matches('\n').count() + 1,
                specifier: specifier.as_str().to_string(),
                to: resolve(parent(file), specifier.as_str(), files)?,
            })
        })
        .collect()
}

/// What brings about each cause of an unresolved TypeScript or JavaScript import,
/// and how to close it.
pub const EXPLAINED: &[Explained] = &[
    Explained {
        cause: Cause::NoTarget,
        because: "for example, a missing file, broken `paths`, or a generated config that is missing, such as the one `svelte-kit sync` writes",
        close: Some(
            "fix the path, or the `paths` it goes through, or generate the missing config (`svelte-kit sync`), then check again",
        ),
    },
    Explained {
        cause: Cause::Wildcard,
        because: "a wildcard such as `declare module \"*.css\"` stands for every file it matches and names none",
        close: Some(
            "write each as the relative path of the file it means, or as the package it is in, or review those dependencies by hand",
        ),
    },
    Explained {
        cause: Cause::SideEffect,
        because: "the compiler records one only when its file is a module, with an `import` or `export`; a file that does not exist leaves nothing either",
        close: Some("make each file imported a module with `export {}`, or fix its path"),
    },
    Explained {
        cause: Cause::ImportCall,
        because: "the compiler records `require()` only where it knows `require`: in JavaScript, or with `@types/node`",
        close: Some("write `import`, or `import x = require()`, instead of `require()`"),
    },
    Explained {
        cause: Cause::Computed,
        because: "a non-literal `import()` or `require()`, or `import.meta.glob`, names no one file; one whose static prefix is a path into one installed package (`/node_modules/pkg/*.svg`) is judged as a use of that package instead",
        close: Some(
            "write each target as a literal path, or review those dependencies and accept them in `accepted_gaps` (`kind: unresolved_imports` with the `file` and `specifier` of each) with the reason",
        ),
    },
];

/// The imports a TypeScript or JavaScript file writes, at the line of each
/// specifier: `import` (`import type` and an import for its side effects among
/// them), `export … from`, `import x = require()`, `import()` and `require()`,
/// with a literal specifier; and, with the text of what they are given,
/// `import()`, `require()` and `import.meta.glob` whose target is computed.
///
/// A file whose extension may hold JSX is read with the TSX grammar, the others
/// with TypeScript's, where `<T>x` is a cast. The compiler knows `require` in a
/// JavaScript file, so the index records it there; in a TypeScript file only
/// with `@types/node`, so there it is an import call ([`Form::ImportCall`]).
pub fn written_imports(file: &str, text: &str) -> Vec<Import> {
    let extension = relpath::extension(file);
    let tree = super::syntax::tree(&grammar(file), text);
    let require = match extension {
        "js" | "jsx" | "mjs" | "cjs" => Form::Static,
        _ => Form::ImportCall { as_import: true },
    };
    let mut imports: Vec<Import> = super::syntax::preorder(tree.root_node())
        .filter_map(|node| import_at(node, text, require))
        .collect();
    imports.sort_by_key(|import| import.line);
    imports
}

/// The declarations of a TypeScript or JavaScript file that scip-typescript
/// gives no relations, though they may extend or implement a type: interface
/// declarations and class expressions, and the `implements` clause of a class
/// declaration, which counts only when it names a type alias.
pub fn unrelated(file: &str, text: &str) -> Vec<UnrelatedSpan> {
    let tree = super::syntax::tree(&grammar(file), text);
    let lines = |node: Node| node.start_position().row + 1..=node.end_position().row + 1;
    let mut found = Vec::new();
    for node in super::syntax::preorder(tree.root_node()) {
        if matches!(node.kind(), "interface_declaration" | "class") {
            found.push(UnrelatedSpan {
                lines: lines(node),
                through_alias: false,
            });
        }
        let declared = node
            .parent()
            .and_then(|heritage| heritage.parent())
            .is_some_and(|class| {
                matches!(
                    class.kind(),
                    "class_declaration" | "abstract_class_declaration"
                )
            });
        if node.kind() == "implements_clause" && declared {
            found.push(UnrelatedSpan {
                lines: lines(node),
                through_alias: true,
            });
        }
    }
    found.sort_by_key(|span| *span.lines.start());
    found
}

/// The grammar of a file by its extension: TSX reads JavaScript and JSX too.
fn grammar(file: &str) -> tree_sitter::Language {
    match relpath::extension(file) {
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        _ => tree_sitter_typescript::LANGUAGE_TSX.into(),
    }
}

/// The import `node` writes, when it is one.
fn import_at(node: Node, text: &str, require: Form) -> Option<Import> {
    let source = |node: Node| node.utf8_text(text.as_bytes()).ok();
    match node.kind() {
        // `import … from "x"`, `import "x"`; `import x = require("x")` holds its
        // string in the clause.
        "import_statement" => {
            let mut cursor = node.walk();
            let clause = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "import_require_clause");
            let form = match (clause, node.child_by_field_name("source")) {
                (Some(_), _) => Form::Static,
                (None, Some(_)) if super::syntax::has_child(node, "import_clause") => Form::Static,
                (None, _) => Form::SideEffect,
            };
            let string = clause
                .and_then(|clause| clause.child_by_field_name("source"))
                .or_else(|| node.child_by_field_name("source"))?;
            literal(string, text).map(|specifier| Import::at(string, specifier, form))
        }
        "export_statement" => {
            let string = node.child_by_field_name("source")?;
            literal(string, text).map(|specifier| Import::at(string, specifier, Form::Static))
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            let form = match (function.kind(), source(function)?) {
                ("import", _) => Form::Static,
                ("identifier", "require") => require,
                (_, "import.meta.glob") => Form::Computed,
                _ => return None,
            };
            let argument = super::syntax::first_argument(node)?;
            match literal(argument, text) {
                // A glob is a pattern of targets, never one.
                Some(specifier) if form != Form::Computed => {
                    Some(Import::at(argument, specifier, form))
                }
                Some(pattern) => Some(Import {
                    prefix: Some(glob_prefix(&pattern)),
                    ..Import::at(argument, pattern, Form::Computed)
                }),
                None => Some(Import {
                    prefix: template_prefix(argument, text),
                    ..Import::at(argument, source(argument)?, Form::Computed)
                }),
            }
        }
        _ => None,
    }
}

/// The whole segments of a glob pattern before the first that is not literal:
/// one that holds any character of the syntax of tinyglobby, which Vite reads
/// (`* ? [ ] { } ( ) !`), or an escape. A wide set: a doubt cuts the prefix
/// short, and leaves a gap. A pattern of literal segments alone is all prefix.
fn glob_prefix(pattern: &str) -> String {
    const SYNTAX: &[char] = &['*', '?', '[', ']', '{', '}', '(', ')', '!', '\\'];
    let segments: Vec<&str> = pattern.split('/').collect();
    match segments.iter().position(|segment| segment.contains(SYNTAX)) {
        Some(first_glob) => segments[..first_glob]
            .iter()
            .map(|segment| format!("{segment}/"))
            .collect(),
        None => pattern.to_string(),
    }
}

/// The whole segments of a template string before its first substitution, from
/// the text the syntax tree gives as its fragments up to the first other part
/// (a substitution, or an escape, read as no literal text). `None` for anything
/// that is not a template.
fn template_prefix(node: Node, text: &str) -> Option<String> {
    if node.kind() != "template_string" {
        return None;
    }
    let mut cursor = node.walk();
    let literal: String = node
        .named_children(&mut cursor)
        .take_while(|child| child.kind() == "string_fragment")
        .filter_map(|fragment| fragment.utf8_text(text.as_bytes()).ok())
        .collect();
    // The segment the first other part cuts short is no part of it.
    Some(
        literal
            .rfind('/')
            .map_or_else(String::new, |end| literal[..=end].to_string()),
    )
}

/// What a string, or a template with nothing substituted, holds between its quotes.
fn literal(node: Node, text: &str) -> Option<String> {
    match node.kind() {
        "string" => {}
        "template_string" if !super::syntax::has_child(node, "template_substitution") => {}
        _ => return None,
    }
    let quoted = node.utf8_text(text.as_bytes()).ok()?;
    quoted
        .get(1..quoted.len().checked_sub(1)?)
        .map(str::to_string)
}

/// Where its package managers install packages, and how a path below one names
/// the package, as the path of a symbol is read.
const INSTALLS: Installs<'static> = Installs {
    dirs: THIRD_PARTY,
    package: npm::package,
};

/// The package installed where `path` runs into `node_modules`, the innermost
/// when they nest, when the path holds its whole name, one npm takes, and
/// something inside it.
fn installed(path: &str) -> Option<String> {
    let segments: Vec<&str> = path.split('/').collect();
    let (package, _) = INSTALLS.package_in(&segments)?;
    (npm::named_by(&package).as_ref() == Some(&package)).then_some(package)
}

/// Whether a specifier is a path: from the importing file, or from the root of
/// the project (`/`, as Vite reads it).
fn is_path(specifier: &str) -> bool {
    ["/", "./", "../"]
        .iter()
        .any(|start| specifier.starts_with(start))
}

/// What a specifier spells when the index takes it to no source target, without
/// the query a bundler reads (`?raw`): the package installed where a path runs
/// into `node_modules`; another relative path; a package; or neither, such as an
/// alias that `paths` or the bundler defines.
pub fn spelled(specifier: &str) -> Spelled {
    let path = specifier
        .split_once('?')
        .map_or(specifier, |(path, _)| path);
    if is_path(specifier) {
        return match installed(path) {
            Some(package) => Spelled::Package(package),
            // A path from the root of the project names no file the way the
            // index reads one.
            None if specifier.starts_with('/') => Spelled::Neither,
            None => Spelled::Paths(vec![path.to_string()]),
        };
    }
    match npm::named_by(specifier) {
        Some(package) => Spelled::Package(package),
        None => Spelled::Neither,
    }
}

/// The package an import computed at run time uses, read from its static prefix
/// ([`Import::prefix`]): the one installed where a path runs into `node_modules`,
/// as [`spelled`] reads a written path. A bare prefix names none: Vite reads a
/// glob that does not start with `./` or `/` through an alias, and a bare
/// `` import(`pkg/${x}`) `` may go through `paths` or an alias, which only the
/// index could rule out.
pub fn computed(prefix: &str) -> Option<String> {
    is_path(prefix).then(|| installed(prefix)).flatten()
}

/// scip-typescript reads what the TypeScript compiler does: the sources and declarations, every
/// JSON file (a `tsconfig.json` and what it extends, a `package.json`, a JSON module an import
/// names) and the lock files that decide which dependency is installed.
pub fn inputs(language: &Language, path: &str) -> bool {
    let name = relpath::file_name(path);
    language.extensions.contains(&relpath::extension(name))
        || name.ends_with(".json")
        || name.ends_with(".json5")
        || matches!(
            name,
            "pnpm-lock.yaml" | "pnpm-workspace.yaml" | "yarn.lock" | "bun.lock" | "bun.lockb"
        )
}

/// Without `node_modules` the TypeScript indexer cannot see third-party
/// modules, so external dependencies would silently disappear from the check.
pub fn needs(root: &Path, dir: &Path, shown: &str) -> anyhow::Result<()> {
    let manifest = dir.join("package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Ok(());
    };
    let declares = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .is_some_and(|json| {
            ["dependencies", "devDependencies"].iter().any(|key| {
                json.get(key)
                    .and_then(|v| v.as_object())
                    .is_some_and(|o| !o.is_empty())
            })
        });
    if !declares {
        return Ok(());
    }
    let mut at: Option<PathBuf> = Some(dir.to_path_buf());
    while let Some(current) = at {
        if current.join("node_modules").is_dir() {
            return Ok(());
        }
        at = (current != root)
            .then(|| current.parent().map(Path::to_path_buf))
            .flatten();
    }
    anyhow::bail!(
        "dependencies are not installed in {shown}: run `npm install` (or pnpm, yarn) first; `easement doctor` lists everything else that is missing"
    )
}

/// A TypeScript symbol's names are called by the file that defines them, or by
/// the npm package when no file of the project does.
pub fn qualifier(module: &mut symbol::Module<'_>) -> Qualifier {
    Qualifier::File(npm_name(&module.parsed.package))
}

/// The external module of a TypeScript symbol: the npm package and the file inside
/// it that was imported (`lodash/fp`, `moment/locale/ru`); `@types/x` stands for `x`.
pub fn external(parsed: &Parsed<'_>) -> String {
    let mut parts: Vec<String> = parsed
        .descriptors
        .iter()
        .take_while(|(_, kind)| *kind == Kind::Namespace)
        .map(|(name, _)| name.clone())
        .collect();
    if let Some(last) = parts.last_mut() {
        *last = module_path_in(last, DECLARATIONS, EXTENSIONS).to_string();
        if last == "index" {
            parts.pop();
        }
    }
    std::iter::once(npm_name(&parsed.package))
        .chain(parts)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::registry::TYPESCRIPT;

    #[test]
    fn an_npm_package_is_named_as_its_imports_spell_it() {
        let deep = crate::scip::symbol::split("scip-typescript npm moment 2.0.0 locale/`ru.d.ts`/")
            .unwrap();
        assert_eq!(external(&deep), "moment/locale/ru");
    }

    /// The file of an npm package is named by its module path, whatever extension
    /// or declaration suffix TypeScript gives it; an `index` file is its directory.
    #[test]
    fn an_npm_file_is_named_without_any_typescript_suffix() {
        let module = |file: &str| {
            external(
                &crate::scip::symbol::split(&format!("scip-typescript npm pkg 1.0.0 `{file}`/"))
                    .unwrap(),
            )
        };
        for file in [
            "foo.cjs",
            "foo.mts",
            "foo.cts",
            "foo.jsx",
            "foo.d.mts",
            "foo.d.ts",
        ] {
            assert_eq!(module(file), "pkg/foo", "{file}");
        }
        for file in ["index.cjs", "index.d.cts", "index.ts"] {
            assert_eq!(module(file), "pkg", "{file}");
        }
    }

    #[test]
    fn the_cache_key_reads_only_what_scip_typescript_does() {
        for path in [
            "src/a.tsx",
            "types/x.d.ts",
            "lib/a.mjs",
            "tsconfig.json",
            "tsconfig.app.json",
            "package.json",
            "locales/en.json",
            "pnpm-lock.yaml",
            "yarn.lock",
        ] {
            assert!(inputs(&TYPESCRIPT, path), "{path}");
        }
        for path in ["main.go", "go.mod", "README.md", "style.css", "logo.svg"] {
            assert!(!inputs(&TYPESCRIPT, path), "{path}");
        }
    }

    #[test]
    fn missing_node_modules_is_an_error_only_when_dependencies_are_declared() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        needs(root, root, ".").unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"x"}"#).unwrap();
        needs(root, root, ".").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"lodash":"1"}}"#,
        )
        .unwrap();
        let err = needs(root, root, ".").unwrap_err();
        assert!(err.to_string().contains("not installed"));
        assert!(err.to_string().contains("easement doctor"));
        std::fs::create_dir(root.join("node_modules")).unwrap();
        needs(root, root, ".").unwrap();
    }

    /// A workspace installs its packages' dependencies above them, up to the root of
    /// the project; a `node_modules` above the root is no install of the project's.
    #[test]
    fn node_modules_counts_up_to_the_root_of_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let package = root.join("packages/web");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"dependencies":{"lodash":"1"}}"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        assert!(needs(&root, &package, "packages/web").is_err());
        std::fs::create_dir(root.join("node_modules")).unwrap();
        needs(&root, &package, "packages/web").unwrap();
    }

    /// `(line, specifier, form)` of each import `file`, holding `text`, writes.
    fn written(file: &str, text: &str) -> Vec<(usize, String, Form)> {
        written_imports(file, text)
            .into_iter()
            .map(|import| (import.line, import.specifier, import.form))
            .collect()
    }

    #[test]
    fn interfaces_and_class_expressions_are_the_declarations_without_relations() {
        let text = "export interface Port {\n  run(): void;\n}\n\nexport class Named implements Port {\n  run() {}\n}\n\nexport const Anonymous = class implements Port {\n  run() {}\n};\n";
        let spans: Vec<(std::ops::RangeInclusive<usize>, bool)> = unrelated("a.ts", text)
            .into_iter()
            .map(|span| (span.lines, span.through_alias))
            .collect();
        // The interface and the class expression; the `implements` clause of the
        // named class, which has relations unless it names a type alias.
        assert_eq!(spans, [(1..=3, false), (5..=5, true), (9..=11, false)]);
    }

    #[test]
    fn written_imports_are_every_form_at_the_line_of_its_specifier() {
        let text = r#"// import "not/this"
/* require("nor/this") */
import { a } from "./a";
import type { B } from './b';
import { type C, d } from "./c";
import "./polyfill";
import def, * as ns from "pkg";
import {
  e,
} from "./multi";
export { f } from "./f";
export * from "./star";
export type { G } from "./g";
import x = require("./x");
const y = require("./y");
const z = import("./z");
const t = import(`./tpl`);
const s = "import 'not/this'";
const u = import(`./${name}`);
const v = import("./" + name);
const w = import.meta.glob("./pages/*.ts");
const r = require(name);
const cz = import(/* webpackChunkName: "z" */ "./z");
const cy = require(/* c */ "./y");
const cu = import(/* @vite-ignore */ `./${name}`);
const cw = import.meta.glob(/* g */ "./pages/*.ts");
const none = import(/* x */);
const cl = import(// c
  "./z");
const cc = require(/* a */ /* b */ "./y");
export const q = 1;
"#;
        let s = Form::Static;
        assert_eq!(
            written("src/app.ts", text),
            [
                (3, "./a", s),
                (4, "./b", s),
                (5, "./c", s),
                (6, "./polyfill", Form::SideEffect),
                (7, "pkg", s),
                (10, "./multi", s),
                (11, "./f", s),
                (12, "./star", s),
                (13, "./g", s),
                (14, "./x", s),
                (15, "./y", Form::ImportCall { as_import: true }),
                (16, "./z", s),
                (17, "./tpl", s),
                (19, "`./${name}`", Form::Computed),
                (20, "\"./\" + name", Form::Computed),
                (21, "./pages/*.ts", Form::Computed),
                (22, "name", Form::Computed),
                // A comment before the argument is no part of it.
                (23, "./z", s),
                (24, "./y", Form::ImportCall { as_import: true }),
                (25, "`./${name}`", Form::Computed),
                (26, "./pages/*.ts", Form::Computed),
                (29, "./z", s),
                (30, "./y", Form::ImportCall { as_import: true }),
            ]
            .map(|(line, specifier, form)| (line, specifier.to_string(), form))
        );
    }

    /// The static prefix of each import computed at run time `text` writes: its
    /// whole path segments before the first that is not literal, as a glob pattern
    /// or a template's text before its first substitution spells them.
    fn prefixes(text: &str) -> Vec<(String, Option<String>)> {
        written_imports("src/app.ts", text)
            .into_iter()
            .filter(|import| import.form == Form::Computed)
            .map(|import| (import.specifier, import.prefix))
            .collect()
    }

    #[test]
    fn a_computed_import_has_the_whole_literal_segments_before_its_first_variant_one() {
        let glob = |pattern: &str, prefix: Option<&str>| {
            (
                format!("import.meta.glob('{pattern}');\n"),
                prefix.map(str::to_string),
            )
        };
        let cases = [
            glob(
                "/node_modules/flag-icons/flags/4x3/*.svg",
                Some("/node_modules/flag-icons/flags/4x3/"),
            ),
            // A segment a wildcard cuts short is no part of it, a package's name
            // included.
            glob("/node_modules/flag-*/x.svg", Some("/node_modules/")),
            glob("/node_modules/@scope/*/x.js", Some("/node_modules/@scope/")),
            glob("/node_modules/*", Some("/node_modules/")),
            glob("/node_modules/.bin/*", Some("/node_modules/.bin/")),
            glob(
                "/node_modules/.pnpm/a@1.0.0/node_modules/a/*.js",
                Some("/node_modules/.pnpm/a@1.0.0/node_modules/a/"),
            ),
            glob("/node_modules/@(a|b)/x.js", Some("/node_modules/")),
            glob("/node_modules/a/{x,y}.js", Some("/node_modules/a/")),
            glob("/node_modules/a/[xy].js", Some("/node_modules/a/")),
            glob("/node_modules/a/x?.js", Some("/node_modules/a/")),
            glob("/node_modules/a\\\\b/x.js", Some("/node_modules/")),
            glob("pkg/*.svg", Some("pkg/")),
            glob("./locales/*.json", Some("./locales/")),
            glob("**/*.ts", Some("")),
            glob("!/node_modules/a/*", Some("")),
            // A pattern of literal segments alone is its own prefix.
            glob("/node_modules/a/x.svg", Some("/node_modules/a/x.svg")),
            ("import.meta.glob(['/node_modules/a/*.js']);\n".into(), None),
            (
                "import(`/node_modules/pkg/${x}.js`);\n".into(),
                Some("/node_modules/pkg/".into()),
            ),
            ("import(`pkg/${x}`);\n".into(), Some("pkg/".into())),
            // A comment before the argument changes nothing.
            (
                "import(/* @vite-ignore */ `/node_modules/pkg/${x}.js`);\n".into(),
                Some("/node_modules/pkg/".into()),
            ),
            (
                "import.meta.glob(/* g */ '/node_modules/a/*.js');\n".into(),
                Some("/node_modules/a/".into()),
            ),
            (
                "import(`/node_modules/pkg-${x}/a.js`);\n".into(),
                Some("/node_modules/".into()),
            ),
            // An escape is read as no literal text.
            (
                "import(`/node_modules/p\\x41g/${x}`);\n".into(),
                Some("/node_modules/".into()),
            ),
            ("import(\"/node_modules/a/\" + x);\n".into(), None),
            ("require(name);\n".into(), None),
        ];
        for (text, prefix) in cases {
            let found = prefixes(&text);
            assert_eq!(found.len(), 1, "{text}");
            assert_eq!(found[0].1, prefix, "{text}");
        }
    }

    /// A static prefix that is a path into one installed package is a use of that
    /// package; any other one, a bare one among them, which an alias may stand
    /// for, names none.
    #[test]
    fn a_static_prefix_into_an_installed_package_names_it() {
        for (prefix, package) in [
            ("/node_modules/flag-icons/flags/4x3/", "flag-icons"),
            ("/node_modules/flag-icons/", "flag-icons"),
            ("/node_modules/@scope/pkg/", "@scope/pkg"),
            ("/node_modules/.pnpm/a@1.0.0/node_modules/a/", "a"),
            ("../node_modules/a/x.svg", "a"),
        ] {
            assert_eq!(computed(prefix), Some(package.into()), "{prefix}");
        }
        for none in [
            "/node_modules/",
            "/node_modules/@scope/",
            "/node_modules/.bin/",
            "pkg/",
            "node_modules/pkg/",
            "./locales/",
            "",
            "/node_modules/a/../",
            "/node_modules/a/../../src/",
        ] {
            assert_eq!(computed(none), None, "{none}");
        }
    }

    /// The compiler knows `require` in a JavaScript file, so the index records it there.
    #[test]
    fn a_require_in_javascript_is_a_static_import_and_jsx_parses() {
        let text = "const u = require(\"./u\");\nexport const el = <div>{u}</div>;\n";
        for file in ["src/a.js", "src/a.jsx", "src/a.cjs", "src/a.mjs"] {
            assert_eq!(
                written(file, text),
                [(1, "./u".to_string(), Form::Static)],
                "{file}"
            );
        }
        let tsx = "import { u } from \"./u\";\nexport const el = <div>{u}</div>;\n";
        assert_eq!(
            written("src/a.tsx", tsx),
            [(1, "./u".to_string(), Form::Static)]
        );
        // An angle-bracket cast is TypeScript, not JSX.
        let cast = "import { u } from \"./u\";\nconst n = <number>u;\n";
        assert_eq!(
            written("src/a.ts", cast),
            [(1, "./u".to_string(), Form::Static)]
        );
    }

    #[test]
    fn a_specifier_spells_a_path_without_its_query_a_package_or_neither() {
        assert_eq!(
            spelled("./app.css"),
            Spelled::Paths(vec!["./app.css".into()])
        );
        assert_eq!(
            spelled("../lib/util.ts?raw"),
            Spelled::Paths(vec!["../lib/util.ts".into()])
        );
        assert_eq!(
            spelled("some-pkg/style.css"),
            Spelled::Package("some-pkg".into())
        );
        assert_eq!(
            spelled("@scope/pkg/x.css?inline"),
            Spelled::Package("@scope/pkg".into())
        );
        assert_eq!(spelled("JSONStream"), Spelled::Package("JSONStream".into()));
        for neither in [
            "#lib/x.css",
            "$lib/x.css",
            "~/x.css",
            "@/x.css",
            "virtual:x",
            "node:fs",
            "/src/x.css",
            ".hidden",
            "_private",
            "@scope",
            "",
        ] {
            assert_eq!(spelled(neither), Spelled::Neither, "{neither}");
        }
    }

    /// A path that runs into an installed package spells that package, by the
    /// innermost `node_modules`, whether it starts at the root of the project or at
    /// the importing file; a path into `node_modules` that holds no package spells
    /// none.
    #[test]
    fn a_path_into_node_modules_spells_the_package_installed_there() {
        for (path, package) in [
            (
                "/node_modules/flag-icons/flags/4x3/de.svg?url",
                "flag-icons",
            ),
            ("./node_modules/@scope/pkg/x.css", "@scope/pkg"),
            ("../../node_modules/.pnpm/a@1.0.0/node_modules/a/x.js", "a"),
        ] {
            assert_eq!(spelled(path), Spelled::Package(package.into()), "{path}");
        }
        for neither in [
            "/node_modules/.bin/x",
            "/node_modules/@scope/x.css",
            "/node_modules/pkg",
            "/node_modules/",
            "/node_modules/a/../b/x.js",
        ] {
            assert_eq!(spelled(neither), Spelled::Neither, "{neither}");
        }
        // A relative path into no package names a file, which is never walked.
        assert_eq!(
            spelled("./node_modules/.bin/x"),
            Spelled::Paths(vec!["./node_modules/.bin/x".into()])
        );
    }

    #[test]
    fn resolves_relative_specifiers_like_a_bundler() {
        let files: BTreeSet<&str> = ["src/a.ts", "src/lib/index.ts", "src/p.d.ts"].into();
        assert_eq!(
            resolve("src/x", "../a", &files).as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            resolve("src", "./lib", &files).as_deref(),
            Some("src/lib/index.ts")
        );
        assert_eq!(resolve("src", "./p", &files).as_deref(), Some("src/p.d.ts"));
        assert_eq!(resolve("", "../../a", &files), None);
    }
}
