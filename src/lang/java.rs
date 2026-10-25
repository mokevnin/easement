use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, dynamic_calls, parse, relative};

const QUERY: &str = "(import_declaration) @node";

/// `Class.forName(x)`: the class is named at run time.
const DYNAMIC_QUERY: &str = r#"
(method_invocation
  object: (identifier) @fn
  name: (identifier) @name
  arguments: (argument_list . (_) @arg)
  (#eq? @fn "Class")
  (#eq? @name "forName"))
"#;

/// Java. Internal classes are indexed by their `package` declaration, so source
/// roots (`src/main/java`, multi-module builds) need no configuration.
///
/// `import a.b.C`, `import static a.b.C.m` and nested classes resolve to the file
/// of the longest matching class name; `import a.b.*` to the package directory.
/// References by fully qualified name without an import are not seen.
pub struct Java {
    language: Language,
    query: Query,
    dynamic: Query,
    /// Fully qualified top-level class name -> file, relative to the root.
    classes: BTreeMap<String, PathBuf>,
    /// Package -> directories holding it, relative to the root.
    packages: BTreeMap<String, BTreeSet<PathBuf>>,
}

impl Java {
    pub fn new(root: &Path) -> Result<Self> {
        let language: Language = tree_sitter_java::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut classes = BTreeMap::new();
        let mut packages: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
        for entry in crate::scan::walker(root).build().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "java") {
                continue;
            }
            let (Some(rel), Some(stem)) = (relative(root, path), path.file_stem()) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            let package = package_of(&source).unwrap_or_default();
            let stem = stem.to_string_lossy();
            let fqn = if package.is_empty() {
                stem.to_string()
            } else {
                format!("{package}.{stem}")
            };
            if let Some(dir) = rel.parent() {
                packages
                    .entry(package)
                    .or_default()
                    .insert(dir.to_path_buf());
            }
            classes.insert(fqn, rel);
        }
        Ok(Self {
            dynamic: Query::new(&language, DYNAMIC_QUERY)?,
            language,
            query,
            classes,
            packages,
        })
    }
}

/// The `package` declaration, read line by line without a full parse.
fn package_of(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("package")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let name = rest.trim().strip_suffix(';')?.trim();
        Some(name.to_string())
    })
}

/// `import static a.b.C.*;` -> `a.b.C.*`
fn specifier(declaration: &str) -> String {
    declaration
        .trim()
        .trim_start_matches("import")
        .trim_end_matches(';')
        .split_whitespace()
        .filter(|w| *w != "static")
        .collect()
}

impl Adapter for Java {
    fn name(&self) -> &'static str {
        "java"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "java")
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        let tree = parse(&self.language, path, source)?;
        Ok(captured_nodes(&self.query, &tree, source)
            .into_iter()
            .map(|node| Import {
                specifier: specifier(node.utf8_text(source.as_bytes()).unwrap_or_default()),
                line: node.start_position().row + 1,
            })
            .collect())
    }

    fn dynamic_imports(&self, path: &Path, source: &str) -> Result<Vec<usize>> {
        dynamic_calls(&self.language, &self.dynamic, path, source, |_, _| true)
    }

    fn resolve(&self, _file: &Path, specifier: &str) -> Target {
        let name = specifier.trim_end_matches(".*");
        let parts: Vec<&str> = name.split('.').collect();
        for k in (1..=parts.len()).rev() {
            if let Some(file) = self.classes.get(&parts[..k].join(".")) {
                return Target::Internal(file.clone());
            }
        }
        if specifier.ends_with(".*")
            && let Some(dir) = self.packages.get(name).and_then(|d| d.first())
        {
            return Target::Internal(dir.clone());
        }
        Target::External(specifier.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_package() {
        assert_eq!(
            package_of("/* x */\npackage com.acme.shop;\nimport a.B;").as_deref(),
            Some("com.acme.shop")
        );
        assert_eq!(package_of("class A {}"), None);
    }

    #[test]
    fn finds_class_for_name() {
        let java = Java::new(Path::new("/nonexistent")).unwrap();
        let src = "class A {\n  void f(String n) throws Exception {\n    Class.forName(n);\n    Other.forName(n);\n  }\n}\n";
        let got = java.dynamic_imports(Path::new("A.java"), src).unwrap();
        assert_eq!(got, [3]);
    }

    #[test]
    fn extracts_imports() {
        let java = Java::new(Path::new("/nonexistent")).unwrap();
        let src = "package p;\n\nimport java.util.List;\nimport static com.acme.Util.helper;\nimport com.acme.model.*;\n\nclass A {}\n";
        let got: Vec<_> = java
            .imports(Path::new("A.java"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("java.util.List".to_string(), 3),
                ("com.acme.Util.helper".to_string(), 4),
                ("com.acme.model.*".to_string(), 5),
            ]
        );
    }
}
