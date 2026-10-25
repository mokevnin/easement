use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Node, Query};

use super::{Adapter, Import, Target, captured_nodes, parse, relative};

const QUERY: &str = "
[
  (namespace_use_declaration)
  (require_expression)
  (require_once_expression)
  (include_expression)
  (include_once_expression)
] @node";

/// PHP. Namespaced names resolve through the PSR-4 maps of every
/// `composer.json` (`autoload` and `autoload-dev`): `use App\Models\User;` is
/// the file `User.php` under the directory mapped to the longest matching
/// namespace prefix, or the directory itself for a namespace.
///
/// `use function` and `use const` point at the directory of their namespace,
/// since a function can live in any file. `require` and `include` of a string
/// literal, optionally after `__DIR__ .`, resolve relative to the file. Classes
/// used by their fully qualified name without a `use` are not seen.
pub struct Php {
    root: PathBuf,
    language: Language,
    query: Query,
    /// Namespace prefix with a trailing `\` -> directory, relative to the root.
    psr4: Vec<(String, PathBuf)>,
    /// Every file and directory of the project.
    paths: BTreeSet<PathBuf>,
}

impl Php {
    pub fn new(root: &Path) -> Result<Self> {
        let language: Language = tree_sitter_php::LANGUAGE_PHP.into();
        let query = Query::new(&language, QUERY)?;
        let mut paths = BTreeSet::new();
        let mut manifests = Vec::new();
        for entry in crate::scan::walker(root).build().flatten() {
            let Some(rel) = relative(root, entry.path()) else {
                continue;
            };
            if entry.file_name() == "composer.json" {
                manifests.push((rel.clone(), entry.path().to_path_buf()));
            }
            for ancestor in rel.ancestors().filter(|a| !a.as_os_str().is_empty()) {
                paths.insert(ancestor.to_path_buf());
            }
        }
        let mut psr4 = Vec::new();
        for (rel, path) in manifests {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let dir = rel.parent().unwrap_or(Path::new(""));
            psr4.extend(psr4_maps(&text, dir));
        }
        psr4.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        Ok(Self {
            root: root.to_path_buf(),
            language,
            query,
            psr4,
            paths,
        })
    }

    /// The file or directory for a namespaced name, trying the longest prefix first.
    fn namespaced(&self, name: &str, namespace_only: bool) -> Option<PathBuf> {
        let name = name.trim_start_matches('\\');
        let full = format!("{name}\\");
        for (prefix, dir) in &self.psr4 {
            let Some(rest) = full.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let rest = rest.trim_end_matches('\\');
            let mut path = dir.clone();
            path.extend(rest.split('\\').filter(|p| !p.is_empty()));
            let file = path.with_extension("php");
            if !namespace_only && self.paths.contains(&file) {
                return Some(file);
            }
            if self.paths.contains(&path) {
                return Some(path);
            }
            // `use App\Models\User\Nested` where only `User.php` exists.
            let mut shorter = path;
            while shorter.pop() && shorter.starts_with(dir) {
                let file = shorter.with_extension("php");
                if !namespace_only && self.paths.contains(&file) {
                    return Some(file);
                }
            }
        }
        None
    }
}

/// `autoload.psr-4` and `autoload-dev.psr-4`: `{"App\\": "src/"}`, a directory
/// or a list of them, resolved against the directory of the manifest.
fn psr4_maps(manifest: &str, dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return Vec::new();
    };
    let mut maps = Vec::new();
    for section in ["autoload", "autoload-dev"] {
        let Some(map) = json[section]["psr-4"].as_object() else {
            continue;
        };
        for (prefix, target) in map {
            let dirs = match target {
                serde_json::Value::String(one) => vec![one.as_str()],
                serde_json::Value::Array(many) => many.iter().filter_map(|v| v.as_str()).collect(),
                _ => Vec::new(),
            };
            for target in dirs {
                let prefix = if prefix.is_empty() || prefix.ends_with('\\') {
                    prefix.clone()
                } else {
                    format!("{prefix}\\")
                };
                maps.push((prefix, normalize(&dir.join(target.trim_end_matches('/')))));
            }
        }
    }
    maps
}

/// Removes `.` and resolves `..` lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

/// The specifiers of a `use` declaration: `A\B`, with `function ` or `const `
/// in front for those kinds; a group `A\{B, C as D}` expands to `A\B` and `A\C`.
fn use_specifiers(node: Node, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    // `function` and `const` sit on the declaration, or on the clause in `use function A\b;`.
    let kind_of = |node: Node| -> &'static str {
        let mut cursor = node.walk();
        let found = node.children(&mut cursor).find_map(|c| match c.kind() {
            "function" => Some("function "),
            "const" => Some("const "),
            _ => None,
        });
        found.unwrap_or("")
    };
    let declared = kind_of(node);
    let clause_name = |clause: Node| -> String {
        let mut cursor = clause.walk();
        let name = clause
            .named_children(&mut cursor)
            .find(|n| matches!(n.kind(), "qualified_name" | "name" | "namespace_name"));
        name.map_or_else(String::new, |n| {
            text(n, source).split_whitespace().collect::<String>()
        })
    };
    let mut specifiers = Vec::new();
    let prefix = children
        .iter()
        .find(|c| c.kind() == "namespace_name")
        .map(|c| text(*c, source).trim_matches('\\').to_string());
    for child in &children {
        match child.kind() {
            "namespace_use_clause" => {
                let kind = if declared.is_empty() {
                    kind_of(*child)
                } else {
                    declared
                };
                specifiers.push(format!(
                    "{kind}{}",
                    clause_name(*child).trim_start_matches('\\')
                ));
            }
            "namespace_use_group" => {
                let mut cursor = child.walk();
                for clause in child.named_children(&mut cursor) {
                    if clause.kind() == "namespace_use_clause" {
                        let kind = if declared.is_empty() {
                            kind_of(clause)
                        } else {
                            declared
                        };
                        let name = clause_name(clause);
                        let full = match &prefix {
                            Some(p) => format!("{p}\\{}", name.trim_start_matches('\\')),
                            None => name,
                        };
                        specifiers.push(format!("{kind}{full}"));
                    }
                }
            }
            _ => {}
        }
    }
    specifiers
}

/// `require 'a.php'` and `include __DIR__ . '/a.php'` -> `include:a.php`; anything
/// computed is left out.
fn include_specifier(node: Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let argument = node.named_children(&mut cursor).next()?;
    let literal = |n: Node| -> Option<String> {
        let mut cursor = n.walk();
        let content = n
            .named_children(&mut cursor)
            .find(|c| c.kind() == "string_content")?;
        Some(text(content, source).to_string())
    };
    match argument.kind() {
        "string" | "encapsed_string" => literal(argument).map(|p| format!("include:{p}")),
        "binary_expression" => {
            let left = argument.child_by_field_name("left")?;
            let right = argument.child_by_field_name("right")?;
            (text(left, source) == "__DIR__" && right.kind() == "string")
                .then(|| literal(right))
                .flatten()
                .map(|p| format!("include:{}", p.trim_start_matches('/')))
        }
        _ => None,
    }
}

impl Adapter for Php {
    fn name(&self) -> &'static str {
        "php"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "php")
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        let tree = parse(&self.language, path, source)?;
        let mut imports = Vec::new();
        for node in captured_nodes(&self.query, &tree, source) {
            let line = node.start_position().row + 1;
            let specifiers = if node.kind() == "namespace_use_declaration" {
                use_specifiers(node, source)
            } else {
                include_specifier(node, source).into_iter().collect()
            };
            imports.extend(
                specifiers
                    .into_iter()
                    .map(|specifier| Import { specifier, line }),
            );
        }
        Ok(imports)
    }

    fn resolve(&self, file: &Path, specifier: &str) -> Target {
        if let Some(path) = specifier.strip_prefix("include:") {
            let found = relative(&self.root, file)
                .map(|rel| normalize(&rel.parent().unwrap_or(Path::new("")).join(path)))
                .filter(|candidate| self.paths.contains(candidate));
            return found.map_or_else(|| Target::External(path.to_string()), Target::Internal);
        }
        let (name, namespace_only) = match specifier.split_once(' ') {
            Some((_, name)) => (name, true),
            None => (specifier, false),
        };
        // A function or constant lives in its namespace, whatever file it is in.
        let lookup = if namespace_only {
            name.rsplit_once('\\')
                .map_or(name, |(namespace, _)| namespace)
        } else {
            name
        };
        match self.namespaced(lookup, namespace_only) {
            Some(path) => Target::Internal(path),
            None => Target::External(specifier.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_uses_groups_and_includes() {
        let php = Php::new(Path::new("/nonexistent")).unwrap();
        let src = "<?php\nnamespace App\\Shop;\nuse App\\Models\\User;\nuse App\\Models\\{Order, Cart as C};\nuse function App\\Util\\helper;\nuse const App\\X\\Y;\nclass A {}\nrequire_once 'lib/x.php';\ninclude __DIR__ . '/y.php';\ninclude $dynamic;\n";
        let got: Vec<_> = php
            .imports(Path::new("A.php"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            [
                ("App\\Models\\User".to_string(), 3),
                ("App\\Models\\Order".to_string(), 4),
                ("App\\Models\\Cart".to_string(), 4),
                ("function App\\Util\\helper".to_string(), 5),
                ("const App\\X\\Y".to_string(), 6),
                ("include:lib/x.php".to_string(), 8),
                ("include:y.php".to_string(), 9),
            ]
        );
    }

    #[test]
    fn reads_psr4_maps() {
        let maps = psr4_maps(
            r#"{"autoload":{"psr-4":{"App\\":"src/","Lib":["lib/","vendor-lib/"]}},"autoload-dev":{"psr-4":{"Tests\\":"tests/"}}}"#,
            Path::new("pkg"),
        );
        assert!(maps.contains(&("App\\".to_string(), PathBuf::from("pkg/src"))));
        assert!(maps.contains(&("Lib\\".to_string(), PathBuf::from("pkg/vendor-lib"))));
        assert!(maps.contains(&("Tests\\".to_string(), PathBuf::from("pkg/tests"))));
    }

    #[test]
    fn resolves_psr4_names_namespaces_and_includes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (path, text) in [
            (
                "composer.json",
                r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
            ),
            (
                "src/Models/User.php",
                "<?php\nnamespace App\\Models;\nclass User {}\n",
            ),
            ("src/Util/helpers.php", "<?php\n"),
            ("lib/x.php", "<?php\n"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let php = Php::new(root).unwrap();
        let go = |file: &str, spec: &str| php.resolve(&root.join(file), spec);
        let internal = |p: &str| Target::Internal(PathBuf::from(p));
        assert_eq!(
            go("src/A.php", "App\\Models\\User"),
            internal("src/Models/User.php")
        );
        assert_eq!(go("src/A.php", "App\\Models"), internal("src/Models"));
        assert_eq!(
            go("src/A.php", "App\\Models\\User\\Role"),
            internal("src/Models/User.php")
        );
        assert_eq!(
            go("src/A.php", "function App\\Util\\helper"),
            internal("src/Util")
        );
        assert_eq!(
            go("src/A.php", "include:../lib/x.php"),
            internal("lib/x.php")
        );
        assert_eq!(
            go("src/A.php", "Illuminate\\Support\\Str"),
            Target::External("Illuminate\\Support\\Str".into())
        );
    }
}
