use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, parse, relative};

const QUERY: &str = "(import) @node";

/// Modifiers that may precede the keyword of a top-level declaration.
const MODIFIERS: &[&str] = &[
    "public",
    "private",
    "internal",
    "protected",
    "data",
    "sealed",
    "enum",
    "abstract",
    "open",
    "final",
    "annotation",
    "inline",
    "value",
    "suspend",
    "const",
    "lateinit",
    "expect",
    "actual",
    "tailrec",
    "operator",
    "infix",
    "external",
];

const KEYWORDS: &[&str] = &[
    "class",
    "interface",
    "object",
    "typealias",
    "fun",
    "val",
    "var",
];

/// Kotlin. Declarations are indexed by the `package` of their file, so source
/// roots and files that hold several classes need no configuration.
///
/// `import a.b.C`, `import a.b.C.Nested` and `import a.b.topLevelFun` resolve to
/// the file that declares the longest matching top-level name (`C`, found by
/// scanning the declarations at the start of lines, else by a file named `C.kt`);
/// `import a.b.*` to the first directory of the package. Names used by their
/// fully qualified name without an import are not seen.
///
/// A name that more than one file declares is ambiguous: the import is taken
/// for the copy in the component of the importing file, and for the first file
/// by path when that component declares none.
pub struct Kotlin<'a> {
    language: Language,
    query: Query,
    /// `package.Name` of a top-level declaration -> the files that declare it,
    /// relative to the root, in slashes.
    declarations: BTreeMap<String, BTreeSet<String>>,
    /// Package -> directories holding it, relative to the root, in slashes.
    packages: BTreeMap<String, BTreeSet<String>>,
    /// The components of the project: a name several files declare is taken
    /// for the copy in the component of the file that imports it.
    map: &'a crate::components::ComponentMap,
    root: &'a Path,
}

impl<'a> Kotlin<'a> {
    pub fn new(root: &'a Path, map: &'a crate::components::ComponentMap) -> Result<Self> {
        let language: Language = tree_sitter_kotlin_ng::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut declarations: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in crate::relpath::walker(root).build().flatten() {
            let path = entry.path();
            if !super::registry::is(path, &super::registry::KOTLIN) {
                continue;
            }
            let (Some(rel), Some(stem)) = (relative(root, path), path.file_stem()) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            let package = package_of(&source).unwrap_or_default();
            let qualify = |name: &str| {
                if package.is_empty() {
                    name.to_string()
                } else {
                    format!("{package}.{name}")
                }
            };
            for name in top_level_names(&source) {
                declarations
                    .entry(qualify(&name))
                    .or_default()
                    .insert(crate::relpath::slash(&rel));
            }
            declarations
                .entry(qualify(&stem.to_string_lossy()))
                .or_default()
                .insert(crate::relpath::slash(&rel));
            if let Some(dir) = rel.parent() {
                packages
                    .entry(package)
                    .or_default()
                    .insert(crate::relpath::slash(dir));
            }
        }
        Ok(Self {
            language,
            query,
            declarations,
            packages,
            map,
            root,
        })
    }

    /// The path of the file that imports, relative to the root, in slashes.
    fn importer(&self, file: &Path) -> Option<String> {
        relative(self.root, file).map(|rel| crate::relpath::slash(&rel))
    }
}

/// The `package` header, read line by line without a full parse.
fn package_of(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("package")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let name = rest.trim().trim_end_matches(';').trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// Names of declarations that start at the beginning of a line, which is where
/// top-level ones are written: `data class User(`, `fun String.slug()`.
fn top_level_names(source: &str) -> Vec<String> {
    source
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let mut words = line.split_whitespace().peekable();
            while words
                .next_if(|w| MODIFIERS.contains(w) || w.starts_with('@'))
                .is_some()
            {}
            let keyword = words.next()?;
            if !KEYWORDS.contains(&keyword) {
                return None;
            }
            let word = loop {
                let word = words.next()?;
                // `fun <T> name`, `fun interface Name`, `companion object` are skipped over.
                if !word.starts_with('<') && word != "interface" {
                    break word;
                }
            };
            // `List<T>.second(` -> `List.second` -> `second`
            let head = word.split(['(', ':', '=', '{']).next()?;
            let mut depth = 0;
            let plain: String = head
                .chars()
                .filter(|c| match c {
                    '<' => {
                        depth += 1;
                        false
                    }
                    '>' => {
                        depth -= 1;
                        false
                    }
                    _ => depth == 0,
                })
                .collect();
            let name = plain.rsplit('.').next()?.to_string();
            (!name.is_empty()).then(|| name.trim_matches('`').to_string())
        })
        .collect()
}

/// `import a.b.C as D` -> `a.b.C`
fn specifier(declaration: &str) -> String {
    let body = declaration
        .trim()
        .trim_start_matches("import")
        .trim_end_matches(';')
        .trim();
    body.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

impl Adapter for Kotlin<'_> {
    fn name(&self) -> &'static str {
        "kotlin"
    }

    fn handles(&self, path: &Path) -> bool {
        super::registry::is(path, &super::registry::KOTLIN)
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
        let importer = self.importer(file);
        let name = specifier.trim_end_matches(".*");
        if specifier.ends_with(".*")
            && let Some(dirs) = self.packages.get(name)
            && let Some(dir) = self
                .map
                .choose(dirs.iter().map(String::as_str), importer.as_deref())
        {
            return Target::Internal(super::Unit::from(dir));
        }
        let parts: Vec<&str> = name.split('.').collect();
        for k in (1..=parts.len()).rev() {
            if let Some(files) = self.declarations.get(&parts[..k].join("."))
                && let Some(found) = self
                    .map
                    .choose(files.iter().map(String::as_str), importer.as_deref())
            {
                return Target::Internal(super::Unit::from(found));
            }
        }
        Target::External(specifier.to_string())
    }

    fn ambiguity(&self, specifier: &str) -> Option<super::Ambiguous> {
        let name = specifier.trim_end_matches(".*");
        let (key, files) = if specifier.ends_with(".*") {
            (name.to_string(), self.packages.get(name)?)
        } else {
            let parts: Vec<&str> = name.split('.').collect();
            let mut found = None;
            for k in (1..=parts.len()).rev() {
                if let Some(files) = self.declarations.get(&parts[..k].join(".")) {
                    found = Some((parts[..k].join("."), files));
                    break;
                }
            }
            found?
        };
        if !self.map.spans_places(files.iter().map(String::as_str)) {
            return None;
        }
        Some(super::Ambiguous {
            name: key,
            files: files.iter().cloned().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The components of a policy, for the resolution of a name.
    fn components(yaml: &str) -> crate::components::ComponentMap {
        let config: crate::config::Config = serde_norway::from_str(yaml).unwrap();
        crate::components::ComponentMap::new(&config).unwrap()
    }

    #[test]
    fn reads_package_and_top_level_names() {
        assert_eq!(
            package_of("// c\npackage com.acme.shop\n\nimport a.B").as_deref(),
            Some("com.acme.shop")
        );
        assert_eq!(package_of("class A"), None);
        let source = "package p\n\nimport a.B\n\n@Serializable\ndata class User(val id: Int)\n\
                      internal fun <T> List<T>.second(): T = this[1]\nfun String.slug() = this\n\
                      object Registry {\n    fun inner() {}\n}\nfun interface Handler { fun run() }\n\
                      typealias Id = Long\nconst val LIMIT = 3\n";
        assert_eq!(
            top_level_names(source),
            [
                "User", "second", "slug", "Registry", "Handler", "Id", "LIMIT"
            ]
        );
    }

    #[test]
    fn extracts_imports() {
        let map = components("version: 1\ncomponents:\n  app: { in: app }\n");
        let kt = Kotlin::new(Path::new("/nonexistent"), &map).unwrap();
        let src = "package p\n\nimport com.acme.model.User\nimport com.acme.util.*\nimport com.acme.x.Y as Z\n\nclass A\n";
        let got: Vec<_> = kt
            .imports(Path::new("A.kt"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            [
                ("com.acme.model.User".to_string(), 3),
                ("com.acme.util.*".to_string(), 4),
                ("com.acme.x.Y".to_string(), 5),
            ]
        );
    }

    #[test]
    fn resolves_declarations_and_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (path, text) in [
            (
                "model/Models.kt",
                "package com.acme.model\n\ndata class User(val id: Int)\nclass Order\n",
            ),
            (
                "util/Strings.kt",
                "package com.acme.util\n\nfun String.slug() = this\n",
            ),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let map = components("version: 1\ncomponents:\n  app: { in: app }\n");
        let kt = Kotlin::new(root, &map).unwrap();
        let go = |spec: &str| kt.resolve(Path::new("A.kt"), spec);
        assert_eq!(
            go("com.acme.model.User"),
            Target::Internal("model/Models.kt".into())
        );
        assert_eq!(
            go("com.acme.model.Order.Item"),
            Target::Internal("model/Models.kt".into())
        );
        assert_eq!(
            go("com.acme.util.slug"),
            Target::Internal("util/Strings.kt".into())
        );
        assert_eq!(
            go("com.acme.util.Strings"),
            Target::Internal("util/Strings.kt".into())
        );
        assert_eq!(go("com.acme.util.*"), Target::Internal("util".into()));
        assert_eq!(
            go("kotlin.collections.List"),
            Target::External("kotlin.collections.List".into())
        );
    }

    /// A name that two components declare is taken for the copy in the
    /// component of the file that imports it, and never for the copy of
    /// another component, whichever file the reader saw first.
    #[test]
    fn a_name_two_components_declare_is_taken_for_the_copy_of_the_importing_component() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // The copy of the other component comes first, so that a reader that
        // takes the first file it sees takes the wrong one.
        for (path, text) in [
            (
                "other/User.kt",
                "package com.acme.model\n\ndata class User(val id: Int)\n",
            ),
            (
                "model/User.kt",
                "package com.acme.model\n\ndata class User(val id: Int)\ndata class Order\n",
            ),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let map =
            components("version: 1\ncomponents:\n  model: { in: model }\n  other: { in: other }\n");
        let kt = Kotlin::new(root, &map).unwrap();
        assert_eq!(
            kt.resolve(&root.join("model/Use.kt"), "com.acme.model.User"),
            Target::Internal("model/User.kt".into())
        );
        assert_eq!(
            kt.resolve(&root.join("other/Use.kt"), "com.acme.model.User"),
            Target::Internal("other/User.kt".into())
        );
        // A file that belongs to no component takes the first file by path.
        assert_eq!(
            kt.resolve(&root.join("nowhere/Use.kt"), "com.acme.model.User"),
            Target::Internal("model/User.kt".into())
        );
        // The name that two components declare is said.
        let ambiguous = kt
            .ambiguity("com.acme.model.User")
            .expect("the name two components declare");
        assert_eq!(ambiguous.name, "com.acme.model.User");
        assert_eq!(ambiguous.files, ["model/User.kt", "other/User.kt"]);
        // A name only one component declares is not ambiguous.
        assert!(kt.ambiguity("com.acme.model.Order").is_none());
    }

    /// A name that a component declares and a file outside every component
    /// declares is ambiguous too: the copy outside is a place of its own, and
    /// which one an import means is not written down in the code.
    #[test]
    fn a_name_declared_inside_and_outside_a_component_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (path, text) in [
            (
                "model/User.kt",
                "package com.acme.model\n\ndata class User(val id: Int)\n",
            ),
            (
                "tools/User.kt",
                "package com.acme.model\n\ndata class User(val id: Int)\n",
            ),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let map = components("version: 1\ncomponents:\n  model: { in: model }\n");
        let kt = Kotlin::new(root, &map).unwrap();
        let ambiguous = kt
            .ambiguity("com.acme.model.User")
            .expect("the name two places declare");
        assert_eq!(ambiguous.name, "com.acme.model.User");
        assert_eq!(ambiguous.files, ["model/User.kt", "tools/User.kt"]);
    }
}
