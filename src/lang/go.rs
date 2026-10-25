use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, query_imports, relative};

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
