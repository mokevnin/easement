use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, parse};

const QUERY: &str = "(import_declaration) @node";

/// Words that may sit between `import` and the module name: `import struct A.B`.
const KINDS: &[&str] = &[
    "typealias",
    "struct",
    "class",
    "enum",
    "protocol",
    "let",
    "var",
    "func",
];

/// Swift. An import names a module, and the modules of the project are the
/// directories under `Sources/` next to a `Package.swift` (SwiftPM layout):
/// `import Core` resolves to the directory `Sources/Core`. Every other module,
/// `Foundation` included, is external. `import struct Core.User` is the module `Core`.
pub struct Swift {
    language: Language,
    query: Query,
    /// Module name -> the directories that hold it, relative to the root.
    modules: BTreeMap<String, BTreeSet<String>>,
}

impl Swift {
    /// The adapter over the packages whose `Package.swift` is among `files`,
    /// relative to `root`.
    pub fn new(root: &Path, files: &[PathBuf]) -> Result<Self> {
        let language: Language = tree_sitter_swift::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut modules: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for rel in files {
            if rel.file_name().is_none_or(|name| name != "Package.swift") {
                continue;
            }
            let Some(package) = rel.parent().map(Path::to_path_buf) else {
                continue;
            };
            let Ok(sources) = std::fs::read_dir(root.join(&package).join("Sources")) else {
                continue;
            };
            for dir in sources.flatten().filter(|d| d.path().is_dir()) {
                let name = dir.file_name().to_string_lossy().to_string();
                modules
                    .entry(name.clone())
                    .or_default()
                    .insert(crate::relpath::slash(
                        package.join("Sources").join(name).as_path(),
                    ));
            }
        }
        Ok(Self {
            language,
            query,
            modules,
        })
    }
}

/// The adapter of the Swift files, for the registry.
pub fn adapter(root: &Path, files: &[PathBuf]) -> Result<Box<dyn Adapter>> {
    Ok(Box::new(Swift::new(root, files)?))
}

/// `@testable import struct Acme.Models.User` -> `Acme.Models.User`
fn specifier(declaration: &str) -> String {
    let words: Vec<&str> = declaration
        .split_whitespace()
        .skip_while(|w| *w != "import")
        .skip(1)
        .collect();
    let name = match words.first() {
        Some(first) if KINDS.contains(first) => words.get(1),
        other => other,
    };
    name.map(|n| n.trim_end_matches(';').to_string())
        .unwrap_or_default()
}

impl Adapter for Swift {
    fn name(&self) -> &'static str {
        "swift"
    }

    fn handles(&self, path: &Path) -> bool {
        super::registry::is(path, &super::registry::SWIFT)
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        let tree = parse(&self.language, path, source)?;
        Ok(captured_nodes(&self.query, &tree, source)
            .into_iter()
            .map(|node| Import {
                specifier: specifier(node.utf8_text(source.as_bytes()).unwrap_or_default()),
                line: node.start_position().row + 1,
                form: super::Form::Static,
            })
            .collect())
    }

    fn resolve(&self, _file: &Path, specifier: &str) -> super::Resolution {
        let module = specifier.split('.').next().unwrap_or(specifier);
        let Some(dirs) = self.modules.get(module) else {
            return super::Resolution {
                target: Target::External(specifier.to_string()),
                candidates: Vec::new(),
                ambiguity: None,
            };
        };
        // A module of the map is a directory that holds it: the set is never
        // empty, and its paths come in the order of the paths.
        let candidates: Vec<String> = dirs.iter().cloned().collect();
        let target = Target::Internal(super::Unit::from(
            candidates
                .first()
                .expect("a module of the map holds a directory")
                .clone(),
        ));
        let many = candidates.len() > 1;
        super::Resolution {
            target,
            candidates: if many { candidates } else { Vec::new() },
            ambiguity: if many { Some(module.to_string()) } else { None },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_every_import_form() {
        let swift = Swift::new(Path::new("/nonexistent"), &[]).unwrap();
        let src = "import Foundation\nimport struct Acme.Models.User\n@testable import Core\nclass A {}\n";
        let got: Vec<_> = swift
            .imports(Path::new("A.swift"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            [
                ("Foundation".to_string(), 1),
                ("Acme.Models.User".to_string(), 2),
                ("Core".to_string(), 3),
            ]
        );
    }

    #[test]
    fn resolves_modules_of_the_package() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for path in [
            "Package.swift",
            "Sources/Core/User.swift",
            "Sources/App/main.swift",
            "Packages/Net/Package.swift",
            "Packages/Net/Sources/Net/Client.swift",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let swift = Swift::new(root, &crate::lang::walked(root)).unwrap();
        let go = |spec: &str| swift.resolve(Path::new("A.swift"), spec).target;
        assert_eq!(go("Core"), Target::Internal("Sources/Core".into()));
        assert_eq!(go("Core.User"), Target::Internal("Sources/Core".into()));
        assert_eq!(
            go("Net"),
            Target::Internal("Packages/Net/Sources/Net".into())
        );
        assert_eq!(go("Foundation"), Target::External("Foundation".into()));
    }

    #[test]
    fn a_module_two_packages_hold_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for path in [
            "Package.swift",
            "Sources/Core/User.swift",
            "Packages/Net/Package.swift",
            "Packages/Net/Sources/Core/Client.swift",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let swift = Swift::new(root, &crate::lang::walked(root)).unwrap();
        let resolution = swift.resolve(Path::new("A.swift"), "Core");
        assert_eq!(
            resolution.target,
            Target::Internal("Packages/Net/Sources/Core".into())
        );
        assert_eq!(
            resolution.candidates,
            ["Packages/Net/Sources/Core", "Sources/Core"]
        );
        assert_eq!(resolution.ambiguity.as_deref(), Some("Core"));
    }
}
