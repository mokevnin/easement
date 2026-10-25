use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, dynamic_calls, query_imports, relative};

/// `import … from`, `export … from`, `import x = require()`, `require()`, `import()`.
const QUERY: &str = r#"
(import_statement source: (string) @spec)
(export_statement source: (string) @spec)
(import_require_clause source: (string) @spec)
(call_expression
  function: (identifier) @fn
  arguments: (arguments . (string) @spec)
  (#eq? @fn "require"))
(call_expression
  function: (import)
  arguments: (arguments . (string) @spec))
"#;

/// `require(x)` and `import(x)` with their first argument.
const DYNAMIC_QUERY: &str = r#"
(call_expression
  function: [(identifier) (import)] @fn
  arguments: (arguments . (_) @arg))
"#;

const TS_EXTENSIONS: &[&str] = &["ts", "mts", "cts"];
const TSX_EXTENSIONS: &[&str] = &["tsx", "js", "jsx", "mjs", "cjs"];

/// A tree-sitter language with the query for static imports and the one for dynamic ones.
type Grammar = (Language, Query, Query);

/// TypeScript and JavaScript. Resolution follows Node/bundler rules via
/// `oxc_resolver`: relative paths, `tsconfig` paths, package `exports`,
/// workspaces. Anything that lands in `node_modules` or outside the project is external.
pub struct TypeScript {
    root: PathBuf,
    ts: Grammar,
    tsx: Grammar,
    resolver: Resolver,
    /// Same options without tsconfig: used when a tsconfig cannot be loaded,
    /// e.g. it `extends` a package that is not installed.
    fallback: Resolver,
}

impl TypeScript {
    pub fn new(root: &Path) -> Result<Self> {
        let grammar = |language: Language| -> Result<Grammar> {
            let query = Query::new(&language, QUERY)?;
            let dynamic = Query::new(&language, DYNAMIC_QUERY)?;
            Ok((language, query, dynamic))
        };
        let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let options = ResolveOptions {
            extensions: strings(&[
                ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json",
            ]),
            extension_alias: vec![
                (".js".into(), strings(&[".ts", ".tsx", ".js", ".jsx"])),
                (".mjs".into(), strings(&[".mts", ".mjs"])),
                (".cjs".into(), strings(&[".cts", ".cjs"])),
            ],
            condition_names: strings(&["types", "import", "require", "node", "default"]),
            main_fields: strings(&["types", "module", "main"]),
            ..ResolveOptions::default()
        };
        let resolver = Resolver::new(ResolveOptions {
            tsconfig: Some(TsconfigDiscovery::Auto),
            ..options.clone()
        });
        let fallback = Resolver::new(options);
        Ok(Self {
            root: root.to_path_buf(),
            ts: grammar(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())?,
            tsx: grammar(tree_sitter_typescript::LANGUAGE_TSX.into())?,
            resolver,
            fallback,
        })
    }

    fn grammar_for(&self, path: &Path) -> Option<&Grammar> {
        let ext = path.extension()?.to_str()?;
        if path.to_str().is_some_and(|p| p.ends_with(".d.ts")) {
            return None;
        }
        if TS_EXTENSIONS.contains(&ext) {
            Some(&self.ts)
        } else if TSX_EXTENSIONS.contains(&ext) {
            Some(&self.tsx)
        } else {
            None
        }
    }
}

fn is_relative(specifier: &str) -> bool {
    specifier.starts_with('.') || specifier.starts_with('/')
}

impl Adapter for TypeScript {
    fn name(&self) -> &'static str {
        "typescript"
    }

    fn handles(&self, path: &Path) -> bool {
        self.grammar_for(path).is_some()
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        match self.grammar_for(path) {
            Some((language, query, _)) => query_imports(language, query, path, source),
            None => Ok(Vec::new()),
        }
    }

    fn dynamic_imports(&self, path: &Path, source: &str) -> Result<Vec<usize>> {
        let Some((language, _, query)) = self.grammar_for(path) else {
            return Ok(Vec::new());
        };
        dynamic_calls(language, query, path, source, |callee, arg| {
            (callee == "require" || callee == "import") && arg.kind() != "string"
        })
    }

    fn resolve(&self, file: &Path, specifier: &str) -> Target {
        let resolved = self
            .resolver
            .resolve_file(file, specifier)
            .or_else(|_| self.fallback.resolve_file(file, specifier));
        match resolved {
            Ok(resolution) => {
                let path = resolution.into_path_buf();
                let in_node_modules = path
                    .components()
                    .any(|c| c == Component::Normal("node_modules".as_ref()));
                match relative(&self.root, &path) {
                    Some(rel) if !in_node_modules => Target::Internal(rel),
                    _ => Target::External(specifier.to_string()),
                }
            }
            Err(_) if is_relative(specifier) => Target::Unresolved,
            Err(_) => Target::External(specifier.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_all_import_forms() {
        let ts = TypeScript::new(Path::new("/nonexistent")).unwrap();
        let src = r#"import a from "./a";
import type { B } from './b';
export { c } from "./c";
export * from "./d";
import e = require("./e");
const f = require("./f");
const g = await import("./g");
const h = notRequire("./h");
"#;
        let got: Vec<_> = ts
            .imports(Path::new("x.ts"), src)
            .unwrap()
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(got, ["./a", "./b", "./c", "./d", "./e", "./f", "./g"]);
    }

    #[test]
    fn finds_dynamic_imports() {
        let ts = TypeScript::new(Path::new("/nonexistent")).unwrap();
        let src = "const a = require('./a');\nconst b = require(name);\nawait import(`./x/${n}`);\nawait import('./y');\nrequireX(z);\n";
        let got = ts.dynamic_imports(Path::new("x.ts"), src).unwrap();
        assert_eq!(got, [2, 3]);
    }

    #[test]
    fn parses_jsx() {
        let ts = TypeScript::new(Path::new("/nonexistent")).unwrap();
        let src = "import React from 'react';\nexport const X = () => <div/>;\n";
        let got = ts.imports(Path::new("x.jsx"), src).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].specifier, "react");
    }
}
