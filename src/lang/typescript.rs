//! TypeScript and JavaScript beyond their index: how the bundlers resolve a
//! relative specifier, and the uses of a file that an index may not report.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use super::registry::{Explained, Spelled, TYPESCRIPT, UnindexedUse, UnrelatedSpan};
use super::{Cause, Form, Import};
use crate::relpath::{self, parent};

/// The file a relative specifier names, as the bundlers resolve it: the path
/// itself, with an extension, or its index.
pub fn resolve(from_dir: &str, spec: &str, files: &BTreeSet<&str>) -> Option<String> {
    let base = relpath::resolve(from_dir, spec)?;
    let exts: Vec<&str> = TYPESCRIPT
        .extensions
        .iter()
        .copied()
        .chain(["d.ts"])
        .collect();
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
        cause: Cause::Require,
        because: "the compiler records `require()` only where it knows `require`: in JavaScript, or with `@types/node`",
        close: Some("write `import`, or `import x = require()`, instead of `require()`"),
    },
    Explained {
        cause: Cause::Computed,
        because: "a non-literal `import()` or `require()`, or `import.meta.glob`, names no one file",
        close: Some("write each target as a literal path, or review those dependencies by hand"),
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
/// with `@types/node`, so there it is [`Form::Require`].
pub fn written_imports(file: &str, text: &str) -> Vec<Import> {
    let extension = relpath::extension(file);
    let tree = super::tree(&grammar(file), text);
    let require = match extension {
        "js" | "jsx" | "mjs" | "cjs" => Form::Static,
        _ => Form::Require,
    };
    let mut imports = Vec::new();
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        if let Some(import) = import_at(node, text, require) {
            imports.push(import);
        }
        let mut cursor = node.walk();
        pending.extend(
            node.named_children(&mut cursor)
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        );
    }
    imports.sort_by_key(|import| import.line);
    imports
}

/// The declarations of a TypeScript or JavaScript file that scip-typescript
/// gives no relations, though they may extend or implement a type: interface
/// declarations and class expressions, and the `implements` clause of a class
/// declaration, which counts only when it names a type alias.
pub fn unrelated(file: &str, text: &str) -> Vec<UnrelatedSpan> {
    let tree = super::tree(&grammar(file), text);
    let lines = |node: Node| node.start_position().row + 1..=node.end_position().row + 1;
    let mut found = Vec::new();
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
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
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
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
                (None, Some(_)) if super::has_child(node, "import_clause") => Form::Static,
                (None, _) => Form::SideEffect,
            };
            let string = clause
                .and_then(|clause| clause.child_by_field_name("source"))
                .or_else(|| node.child_by_field_name("source"))?;
            literal(string, text).map(|specifier| at(string, specifier, form))
        }
        "export_statement" => {
            let string = node.child_by_field_name("source")?;
            literal(string, text).map(|specifier| at(string, specifier, Form::Static))
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            let form = match (function.kind(), source(function)?) {
                ("import", _) => Form::Static,
                ("identifier", "require") => require,
                (_, "import.meta.glob") => Form::Computed,
                _ => return None,
            };
            let arguments = node.child_by_field_name("arguments")?;
            let mut cursor = arguments.walk();
            let argument = arguments.named_children(&mut cursor).next()?;
            match literal(argument, text) {
                // A glob is a pattern of targets, never one.
                Some(specifier) if form != Form::Computed => Some(at(argument, specifier, form)),
                Some(pattern) => Some(at(argument, pattern, Form::Computed)),
                None => Some(at(argument, source(argument)?.to_string(), Form::Computed)),
            }
        }
        _ => None,
    }
}

/// What a string, or a template with nothing substituted, holds between its quotes.
fn literal(node: Node, text: &str) -> Option<String> {
    match node.kind() {
        "string" => {}
        "template_string" if !super::has_child(node, "template_substitution") => {}
        _ => return None,
    }
    let quoted = node.utf8_text(text.as_bytes()).ok()?;
    quoted
        .get(1..quoted.len().checked_sub(1)?)
        .map(str::to_string)
}

/// The import of `specifier` in `form`, at the line `node` starts on.
fn at(node: Node, specifier: String, form: Form) -> Import {
    Import {
        specifier,
        line: node.start_position().row + 1,
        form,
        recorded_on: None,
    }
}

/// What a specifier spells when the index takes it to no source target: a
/// relative path, without the query a bundler reads (`?raw`); a package; or
/// neither, such as an alias that `paths` or the bundler defines.
pub fn spelled(specifier: &str) -> Spelled {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let path = specifier
            .split_once('?')
            .map_or(specifier, |(path, _)| path);
        return Spelled::Path(path.to_string());
    }
    match crate::lang::registry::Manifest::Npm.named_by(specifier) {
        Some(package) => Spelled::Package(package),
        None => Spelled::Neither,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                (15, "./y", Form::Require),
                (16, "./z", s),
                (17, "./tpl", s),
                (19, "`./${name}`", Form::Computed),
                (20, "\"./\" + name", Form::Computed),
                (21, "./pages/*.ts", Form::Computed),
                (22, "name", Form::Computed),
            ]
            .map(|(line, specifier, form)| (line, specifier.to_string(), form))
        );
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
        assert_eq!(spelled("./app.css"), Spelled::Path("./app.css".into()));
        assert_eq!(
            spelled("../lib/util.ts?raw"),
            Spelled::Path("../lib/util.ts".into())
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
