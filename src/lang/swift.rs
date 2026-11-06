use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, parse, relative};

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
///
/// A module that more than one component declares is ambiguous: the import is
/// taken for the copy in the component of the importing file, and for the
/// first directory by path when that component declares none.
pub struct Swift<'a> {
    language: Language,
    query: Query,
    /// Module name -> the directories that hold it, relative to the root, in
    /// slashes.
    modules: BTreeMap<String, BTreeSet<String>>,
    /// The components of the project: a module several components declare is
    /// taken for the copy in the component of the file that imports it.
    map: &'a crate::components::ComponentMap,
    root: &'a Path,
}

impl<'a> Swift<'a> {
    pub fn new(root: &'a Path, map: &'a crate::components::ComponentMap) -> Result<Self> {
        let language: Language = tree_sitter_swift::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut modules: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in crate::relpath::walker(root).build().flatten() {
            if entry.file_name() != "Package.swift" {
                continue;
            }
            let Some(package) =
                relative(root, entry.path()).and_then(|rel| rel.parent().map(Path::to_path_buf))
            else {
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
                    .insert(crate::relpath::slash(&package.join("Sources").join(name)));
            }
        }
        Ok(Self {
            language,
            query,
            modules,
            map,
            root,
        })
    }

    /// The path of the file that imports, relative to the root, in slashes.
    fn importer(&self, file: &Path) -> Option<String> {
        relative(self.root, file).map(|rel| crate::relpath::slash(&rel))
    }
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

impl Adapter for Swift<'_> {
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
            })
            .collect())
    }

    fn resolve(&self, file: &Path, specifier: &str) -> Target {
        let module = specifier.split('.').next().unwrap_or(specifier);
        let importer = self.importer(file);
        match self.modules.get(module) {
            Some(dirs) => match self
                .map
                .choose(dirs.iter().map(String::as_str), importer.as_deref())
            {
                Some(dir) => Target::Internal(super::Unit::from(dir)),
                None => Target::External(specifier.to_string()),
            },
            None => Target::External(specifier.to_string()),
        }
    }

    fn ambiguity(&self, specifier: &str) -> Option<super::Ambiguous> {
        let module = specifier.split('.').next().unwrap_or(specifier);
        let dirs = self.modules.get(module)?;
        if !self.map.spans_places(dirs.iter().map(String::as_str)) {
            return None;
        }
        Some(super::Ambiguous {
            name: module.to_string(),
            files: dirs.iter().cloned().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The components of a policy, for the resolution of a module.
    fn components(yaml: &str) -> crate::components::ComponentMap {
        let config: crate::config::Config = serde_norway::from_str(yaml).unwrap();
        crate::components::ComponentMap::new(&config).unwrap()
    }

    #[test]
    fn extracts_every_import_form() {
        let map = components("version: 1\ncomponents:\n  app: { in: app }\n");
        let swift = Swift::new(Path::new("/nonexistent"), &map).unwrap();
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
        let map = components("version: 1\ncomponents:\n  app: { in: app }\n");
        let swift = Swift::new(root, &map).unwrap();
        let go = |spec: &str| swift.resolve(Path::new("A.swift"), spec);
        assert_eq!(go("Core"), Target::Internal("Sources/Core".into()));
        assert_eq!(go("Core.User"), Target::Internal("Sources/Core".into()));
        assert_eq!(
            go("Net"),
            Target::Internal("Packages/Net/Sources/Net".into())
        );
        assert_eq!(go("Foundation"), Target::External("Foundation".into()));
    }

    /// A module that two components declare is taken for the copy in the
    /// component of the file that imports it.
    #[test]
    fn a_module_two_components_declare_is_taken_for_the_copy_of_the_importing_component() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // The copy of the other component comes first, so that a reader that
        // takes the first file it sees takes the wrong one.
        for (path, text) in [
            ("other/Package.swift", "// swift-tools-version: 5.9\n"),
            ("other/Sources/Core/User.swift", ""),
            ("app/Package.swift", "// swift-tools-version: 5.9\n"),
            ("app/Sources/Core/User.swift", ""),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let map =
            components("version: 1\ncomponents:\n  app: { in: app }\n  other: { in: other }\n");
        let swift = Swift::new(root, &map).unwrap();
        assert_eq!(
            swift.resolve(&root.join("app/Use.swift"), "Core"),
            Target::Internal("app/Sources/Core".into())
        );
        assert_eq!(
            swift.resolve(&root.join("other/Use.swift"), "Core"),
            Target::Internal("other/Sources/Core".into())
        );
        // A file that belongs to no component takes the first directory by path.
        assert_eq!(
            swift.resolve(&root.join("nowhere/Use.swift"), "Core"),
            Target::Internal("app/Sources/Core".into())
        );
        let ambiguous = swift
            .ambiguity("Core")
            .expect("the module two components declare");
        assert_eq!(ambiguous.name, "Core");
        assert_eq!(ambiguous.files, ["app/Sources/Core", "other/Sources/Core"]);
        // A module only one component declares is not ambiguous.
        assert!(swift.ambiguity("Net").is_none());
    }
}
