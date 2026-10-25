use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, parse, relative};

const QUERY: &str = "(using_directive) @node";

/// C#. A namespace is not tied to a directory, so internal namespaces are
/// indexed by the `namespace` declarations in the project.
///
/// `using A.B;` resolves to the directory of the first file (in path order)
/// that declares `A.B`, so keep a namespace inside one component.
/// `using static A.B.Type;` and `using Alias = A.B.Type;` resolve to
/// `Type.cs` next to the namespace when it exists. Types used by their fully
/// qualified name without a `using` are not seen.
pub struct CSharp {
    language: Language,
    query: Query,
    /// Namespace -> directories that declare it, relative to the root.
    namespaces: BTreeMap<String, BTreeSet<PathBuf>>,
    /// Every `.cs` file of the project.
    files: BTreeSet<PathBuf>,
}

impl CSharp {
    pub fn new(root: &Path) -> Result<Self> {
        let language: Language = tree_sitter_c_sharp::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut namespaces: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
        let mut files = BTreeSet::new();
        for entry in crate::scan::walker(root).build().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "cs") {
                continue;
            }
            let Some(rel) = relative(root, path) else {
                continue;
            };
            if let Ok(source) = std::fs::read_to_string(path) {
                let dir = rel.parent().unwrap_or(Path::new("")).to_path_buf();
                for namespace in declared_namespaces(&source) {
                    namespaces.entry(namespace).or_default().insert(dir.clone());
                }
            }
            files.insert(rel);
        }
        Ok(Self {
            language,
            query,
            namespaces,
            files,
        })
    }
}

/// Names after `namespace`, read line by line without a full parse.
fn declared_namespaces(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("namespace")?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// `global using static A.B.C;` -> `A.B.C`; `using X = A.B.C;` -> `A.B.C`.
fn specifier(directive: &str) -> String {
    let body = directive
        .trim()
        .trim_end_matches(';')
        .trim()
        .trim_start_matches("global")
        .trim()
        .trim_start_matches("using")
        .trim();
    let body = body.split_once('=').map_or(body, |(_, target)| target);
    body.split_whitespace()
        .filter(|w| *w != "static" && *w != "unsafe")
        .collect()
}

impl Adapter for CSharp {
    fn name(&self) -> &'static str {
        "csharp"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "cs")
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

    fn resolve(&self, _file: &Path, specifier: &str) -> Target {
        let parts: Vec<&str> = specifier.split('.').collect();
        for k in (1..=parts.len()).rev() {
            let Some(dir) = self
                .namespaces
                .get(&parts[..k].join("."))
                .and_then(|d| d.first())
            else {
                continue;
            };
            if let Some(next) = parts.get(k) {
                let file = dir.join(format!("{next}.cs"));
                if self.files.contains(&file) {
                    return Target::Internal(file);
                }
            }
            return Target::Internal(dir.clone());
        }
        Target::External(specifier.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_every_using_form() {
        let cs = CSharp::new(Path::new("/nonexistent")).unwrap();
        let src = "global using System;\nusing static Acme.Util.Helpers;\nusing Alias = Acme.Models.User;\nusing Acme.Models;\nnamespace Acme.Shop { class A { void F() { using var x = G(); } } }\n";
        let got: Vec<_> = cs
            .imports(Path::new("A.cs"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            [
                ("System".to_string(), 1),
                ("Acme.Util.Helpers".to_string(), 2),
                ("Acme.Models.User".to_string(), 3),
                ("Acme.Models".to_string(), 4),
            ]
        );
    }

    #[test]
    fn reads_namespaces() {
        assert_eq!(
            declared_namespaces("namespace Acme.Shop;\nnamespace Other {\n}\nnamespaceX Y;"),
            ["Acme.Shop", "Other"]
        );
    }

    #[test]
    fn resolves_namespaces_and_types() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (path, text) in [
            (
                "Models/User.cs",
                "namespace Acme.Models;\npublic class User {}\n",
            ),
            ("Util/Helpers.cs", "namespace Acme.Util\n{\n}\n"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let cs = CSharp::new(root).unwrap();
        let go = |spec: &str| cs.resolve(Path::new("A.cs"), spec);
        assert_eq!(go("Acme.Models"), Target::Internal("Models".into()));
        assert_eq!(
            go("Acme.Models.User"),
            Target::Internal("Models/User.cs".into())
        );
        assert_eq!(
            go("Acme.Util.Helpers"),
            Target::Internal("Util/Helpers.cs".into())
        );
        assert_eq!(go("Acme.Util.Missing"), Target::Internal("Util".into()));
        assert_eq!(go("System.Linq"), Target::External("System.Linq".into()));
    }
}
