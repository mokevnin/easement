use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Node, Query};

use super::{Adapter, Import, Target, captured_nodes, dynamic_calls, parse, relative};

const QUERY: &str = "[(import_statement) (import_from_statement)] @node";

/// Calls that load a module by name, with their first argument.
const DYNAMIC_QUERY: &str = "(call function: (_) @fn arguments: (argument_list . (_) @arg))";
const DYNAMIC_CALLS: &[&str] = &["importlib.import_module", "import_module", "__import__"];

/// Python. `import a.b` and `from a import b` are resolved against the source
/// roots (`sys.path` entries inside the project) to the longest dotted prefix
/// that exists as a module or package; relative imports against the file's package.
///
/// `from a import b` is recorded as `a.b`: `b` may be a submodule or just a name
/// defined in `a`, and the longest-prefix rule picks whichever exists.
pub struct Python {
    root: PathBuf,
    roots: Vec<PathBuf>,
    language: Language,
    query: Query,
    dynamic: Query,
}

impl Python {
    pub fn new(root: &Path, roots: &[PathBuf]) -> Result<Self> {
        let language: Language = tree_sitter_python::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let roots = roots
            .iter()
            .map(|r| root.join(r))
            .filter(|r| r.is_dir())
            .collect();
        Ok(Self {
            root: root.to_path_buf(),
            roots,
            dynamic: Query::new(&language, DYNAMIC_QUERY)?,
            language,
            query,
        })
    }

    fn internal(&self, path: PathBuf) -> Target {
        match relative(&self.root, &path) {
            Some(rel) => Target::Internal(rel),
            None => Target::Unresolved,
        }
    }
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

/// The dotted name of an `import` target, ignoring `as` aliases.
fn imported_name<'a>(node: Node, source: &'a str) -> &'a str {
    match node.kind() {
        "aliased_import" => node
            .child_by_field_name("name")
            .map_or("", |n| text(n, source)),
        _ => text(node, source),
    }
}

fn has_interpolation(string: Node) -> bool {
    let mut cursor = string.walk();
    string
        .children(&mut cursor)
        .any(|c| c.kind() == "interpolation")
}

fn join(module: &str, name: &str) -> String {
    if module.ends_with('.') {
        format!("{module}{name}")
    } else {
        format!("{module}.{name}")
    }
}

fn specifiers(node: Node, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    let names: Vec<&str> = node
        .children_by_field_name("name", &mut cursor)
        .map(|n| imported_name(n, source))
        .collect();
    match node.kind() {
        "import_statement" => names.into_iter().map(str::to_string).collect(),
        _ => {
            let module: String = node
                .child_by_field_name("module_name")
                .map_or("", |n| text(n, source))
                .split_whitespace()
                .collect();
            if names.is_empty() {
                vec![module] // `from a import *`
            } else {
                names.into_iter().map(|n| join(&module, n)).collect()
            }
        }
    }
}

/// The module or package file for a dotted path under `base`, if it exists.
/// Directories without `__init__.py` count as namespace packages except at the
/// top level, where they would shadow standard-library and third-party names.
fn module_at(base: &Path, parts: &[&str], top_level: bool) -> Option<PathBuf> {
    let (last, dirs) = parts.split_last()?;
    let dir: PathBuf = dirs.iter().fold(base.to_path_buf(), |d, p| d.join(p));
    let package = dir.join(last);
    for candidate in [
        dir.join(format!("{last}.py")),
        dir.join(format!("{last}.pyi")),
        package.join("__init__.py"),
        package.join("__init__.pyi"),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    (!top_level && package.is_dir()).then_some(package)
}

/// Longest prefix of `parts` that resolves under `base`.
fn longest(base: &Path, parts: &[&str], absolute: bool) -> Option<PathBuf> {
    (1..=parts.len())
        .rev()
        .find_map(|k| module_at(base, &parts[..k], absolute && k == 1))
}

impl Adapter for Python {
    fn name(&self) -> &'static str {
        "python"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "py" || e == "pyi")
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        let tree = parse(&self.language, path, source)?;
        let mut imports = Vec::new();
        for node in captured_nodes(&self.query, &tree, source) {
            let line = node.start_position().row + 1;
            for specifier in specifiers(node, source) {
                imports.push(Import { specifier, line });
            }
        }
        Ok(imports)
    }

    fn dynamic_imports(&self, path: &Path, source: &str) -> Result<Vec<usize>> {
        dynamic_calls(
            &self.language,
            &self.dynamic,
            path,
            source,
            |callee, arg| {
                DYNAMIC_CALLS.contains(&callee)
                    && (arg.kind() != "string" || has_interpolation(arg))
            },
        )
    }

    fn resolve(&self, file: &Path, specifier: &str) -> Target {
        let rest = specifier.trim_start_matches('.');
        let level = specifier.len() - rest.len();
        let parts: Vec<&str> = rest.split('.').filter(|p| !p.is_empty()).collect();

        if level > 0 {
            let mut base = file.parent().unwrap_or(&self.root).to_path_buf();
            for _ in 1..level {
                base = match base.parent() {
                    Some(p) => p.to_path_buf(),
                    None => return Target::Unresolved,
                };
            }
            let found = longest(&base, &parts, false).or_else(|| {
                let init = base.join("__init__.py");
                Some(if init.is_file() { init } else { base.clone() })
            });
            return found.map_or(Target::Unresolved, |p| self.internal(p));
        }

        self.roots
            .iter()
            .find_map(|root| longest(root, &parts, true))
            .map_or_else(
                || Target::External(specifier.to_string()),
                |p| self.internal(p),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dynamic_imports() {
        let py = Python::new(Path::new("/nonexistent"), &[]).unwrap();
        let src = "import importlib\nimportlib.import_module('a.b')\nimportlib.import_module(name)\n__import__(f'pkg.{x}')\nimport_module(\"c\" + d)\nprint(name)\n";
        let got = py.dynamic_imports(Path::new("x.py"), src).unwrap();
        assert_eq!(got, [3, 4, 5]);
    }

    #[test]
    fn extracts_all_import_forms() {
        let py = Python::new(Path::new("/nonexistent"), &[]).unwrap();
        let src = "import os, a.b as ab\nfrom c.d import e, f as g\nfrom . import h\nfrom ..i import j\nfrom k import *\nfrom __future__ import annotations\n";
        let got: Vec<_> = py
            .imports(Path::new("x.py"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        let expected = [
            ("os", 1),
            ("a.b", 1),
            ("c.d.e", 2),
            ("c.d.f", 2),
            (".h", 3),
            ("..i.j", 4),
            ("k", 5),
        ];
        assert_eq!(
            got,
            expected
                .iter()
                .map(|(s, l)| (s.to_string(), *l))
                .collect::<Vec<_>>()
        );
    }
}
