use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Query};

use std::collections::HashMap;

use tree_sitter::Node;

use super::{Adapter, Import, SymbolUse, Target, parse, query_imports, relative};

const QUERY: &str = "(import_spec path: (_) @spec)";

/// Go: an import path names a package directory. Paths under a module declared
/// by any `go.mod` in the project are internal; everything else is external.
pub struct Go {
    root: PathBuf,
    language: Language,
    query: Query,
    /// (module path, module directory), longest module path first.
    modules: Vec<(String, PathBuf)>,
}

impl Go {
    pub fn new(root: &Path) -> Result<Self> {
        let language: Language = tree_sitter_go::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut modules = Vec::new();
        for entry in crate::scan::walker(root).build().flatten() {
            if entry.file_name() == "go.mod" {
                let text = std::fs::read_to_string(entry.path())?;
                if let Some(module) = module_path(&text) {
                    let dir = entry.path().parent().unwrap_or(root).to_path_buf();
                    modules.push((module, dir));
                }
            }
        }
        modules.sort_by_key(|(m, _)| std::cmp::Reverse(m.len()));
        Ok(Self {
            root: root.to_path_buf(),
            language,
            query,
            modules,
        })
    }
}

/// The name a package is usually imported by: the last path element, skipping
/// a major-version suffix (`/v2`) and the `.v3` of `gopkg.in/yaml.v3`.
fn package_name(path: &str) -> &str {
    let mut parts = path.rsplit('/');
    let last = parts.next().unwrap_or(path);
    let last = match parts.next() {
        Some(previous)
            if last.len() > 1
                && last.starts_with('v')
                && last[1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            previous
        }
        _ => last,
    };
    last.split('.').next().unwrap_or(last)
}

fn walk<'t>(node: Node<'t>, visit: &mut impl FnMut(Node<'t>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, visit);
    }
}

fn module_path(go_mod: &str) -> Option<String> {
    go_mod.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("module")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let module = rest.trim().trim_matches('"');
        (!module.is_empty()).then(|| module.to_string())
    })
}

impl Adapter for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "go")
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        query_imports(&self.language, &self.query, path, source)
    }

    /// `pkg.Name` in expressions and types, where `pkg` is the name an import
    /// gives (its alias, or the package name from its path). A local variable
    /// that shadows an import name is not told apart.
    fn symbol_uses(&self, path: &Path, source: &str) -> Result<Vec<SymbolUse>> {
        let tree = parse(&self.language, path, source)?;
        let bytes = source.as_bytes();
        let text = |node: Node| node.utf8_text(bytes).unwrap_or_default().to_string();
        let mut names: HashMap<String, String> = HashMap::new();
        walk(tree.root_node(), &mut |node| {
            if node.kind() != "import_spec" {
                return;
            }
            let Some(spec) = node.child_by_field_name("path") else {
                return;
            };
            let specifier = text(spec).trim_matches(['"', '`']).to_string();
            let local = match node.child_by_field_name("name").map(text) {
                Some(alias) if alias == "_" || alias == "." => return,
                Some(alias) => alias,
                None => package_name(&specifier).to_string(),
            };
            names.insert(local, specifier);
        });
        let mut uses = Vec::new();
        walk(tree.root_node(), &mut |node| {
            let (qualifier, member) = match node.kind() {
                "selector_expression" => (
                    node.child_by_field_name("operand")
                        .filter(|n| n.kind() == "identifier"),
                    node.child_by_field_name("field"),
                ),
                "qualified_type" => (
                    node.child_by_field_name("package"),
                    node.child_by_field_name("name"),
                ),
                _ => return,
            };
            if let (Some(qualifier), Some(member)) = (qualifier, member)
                && let Some(specifier) = names.get(&text(qualifier))
            {
                uses.push(SymbolUse {
                    specifier: specifier.clone(),
                    name: text(member),
                    line: member.start_position().row + 1,
                });
            }
        });
        Ok(uses)
    }

    fn resolve(&self, _file: &Path, specifier: &str) -> Target {
        for (module, dir) in &self.modules {
            let rest = if specifier == module {
                Some("")
            } else {
                specifier
                    .strip_prefix(module.as_str())
                    .and_then(|r| r.strip_prefix('/'))
            };
            if let Some(rest) = rest {
                let target = dir.join(rest);
                return match relative(&self.root, &target) {
                    Some(rel) if target.is_dir() => Target::Internal(rel),
                    _ => Target::Unresolved,
                };
            }
        }
        Target::External(specifier.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_module_path() {
        assert_eq!(
            module_path("// c\nmodule example.com/app\n\ngo 1.25\n").as_deref(),
            Some("example.com/app")
        );
        assert_eq!(module_path("modulex foo"), None);
    }

    #[test]
    fn finds_symbols_used_through_imports() {
        let go = Go::new(Path::new("/nonexistent")).unwrap();
        let src = "package h
import (
  \"fmt\"
  store \"example.com/app/db\"
  \"example.com/app/api/v2\"
  _ \"example.com/app/drivers\"
)
func f(c *store.Client) {
  fmt.Println(api.Scoped(1), c.Tx, store.Raw)
}
";
        let got: Vec<_> = go
            .symbol_uses(Path::new("h.go"), src)
            .unwrap()
            .into_iter()
            .map(|u| (u.specifier, u.name, u.line))
            .collect();
        let want = |s: &str, n: &str, l| (s.to_string(), n.to_string(), l);
        assert_eq!(
            got,
            vec![
                want("example.com/app/db", "Client", 8),
                want("fmt", "Println", 9),
                want("example.com/app/api/v2", "Scoped", 9),
                want("example.com/app/db", "Raw", 9),
            ]
        );
    }

    #[test]
    fn package_names_skip_version_suffixes() {
        assert_eq!(package_name("example.com/app/db"), "db");
        assert_eq!(package_name("example.com/app/api/v2"), "api");
        assert_eq!(package_name("gopkg.in/yaml.v3"), "yaml");
        assert_eq!(package_name("fmt"), "fmt");
    }

    #[test]
    fn extracts_single_and_grouped_imports() {
        let go = Go::new(Path::new("/nonexistent")).unwrap();
        let src = "package main\nimport \"fmt\"\nimport (\n  b \"example.com/app/internal/b\"\n  _ `example.com/app/c`\n)\n";
        let got: Vec<_> = go
            .imports(Path::new("main.go"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("fmt".to_string(), 2),
                ("example.com/app/internal/b".to_string(), 4),
                ("example.com/app/c".to_string(), 5),
            ]
        );
    }
}
