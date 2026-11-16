use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, tree};

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

/// How a top-level name is declared: a type, which only one place may hold, or
/// a function or property, which overloads and extensions may repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclKind {
    Type,
    Member,
}

/// Kotlin. Declarations are indexed by the `package` of their file, so source
/// roots and files that hold several classes need no configuration.
///
/// `import a.b.C`, `import a.b.C.Nested` and `import a.b.topLevelFun` resolve to
/// the file that declares the longest matching top-level name (`C`, found by
/// scanning the declarations at the start of lines, else by a file named `C.kt`);
/// `import a.b.*` to the first directory of the package. Names used by their
/// fully qualified name without an import are not seen.
pub struct Kotlin {
    language: Language,
    query: Query,
    /// `package.Name` of a top-level declaration -> file, relative to the root,
    /// and how it is declared there.
    declarations: BTreeMap<String, BTreeMap<String, DeclKind>>,
    /// Package -> directories holding it, relative to the root.
    packages: BTreeMap<String, BTreeSet<String>>,
}

impl Kotlin {
    /// The adapter over the Kotlin files among `files`, relative to `root`.
    pub fn new(root: &Path, files: &[PathBuf]) -> Result<Self> {
        let language: Language = tree_sitter_kotlin_ng::LANGUAGE.into();
        let query = Query::new(&language, QUERY)?;
        let mut declarations: BTreeMap<String, BTreeMap<String, DeclKind>> = BTreeMap::new();
        let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for rel in files {
            if !super::registry::is(rel, &super::registry::KOTLIN) {
                continue;
            }
            let Some(stem) = rel.file_stem() else {
                continue;
            };
            // Declarations are read whatever the encoding of the file. One that cannot
            // be read declares nothing known; the scan reports it as unreadable.
            let Ok(bytes) = std::fs::read(root.join(rel)) else {
                continue;
            };
            let source = String::from_utf8_lossy(&bytes);
            let package = package_of(&source).unwrap_or_default();
            let qualify = |name: &str| {
                if package.is_empty() {
                    name.to_string()
                } else {
                    format!("{package}.{name}")
                }
            };
            for (name, kind) in top_level_declarations(&source) {
                declarations
                    .entry(qualify(&name))
                    .or_default()
                    .entry(crate::relpath::slash(rel))
                    .or_insert(kind);
            }
            // A file may be imported by its name alone, so two files of one
            // name in one package are two places an import may mean.
            declarations
                .entry(qualify(&stem.to_string_lossy()))
                .or_default()
                .entry(crate::relpath::slash(rel))
                .or_insert(DeclKind::Type);
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
        })
    }

    /// The files that declare the longest name of `name` the project declares,
    /// each with how it declares it.
    fn declared(&self, name: &str) -> Option<&BTreeMap<String, DeclKind>> {
        let parts: Vec<&str> = name.split('.').collect();
        for k in (1..=parts.len()).rev() {
            if let Some(files) = self.declarations.get(&parts[..k].join(".")) {
                return Some(files);
            }
        }
        None
    }
}

/// The adapter of the Kotlin files, for the registry.
pub fn adapter(root: &Path, files: &[PathBuf]) -> Result<Box<dyn Adapter>> {
    Ok(Box::new(Kotlin::new(root, files)?))
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
/// top-level ones are written: `data class User(`, `fun String.slug()`, each
/// with how it is declared.
fn top_level_declarations(source: &str) -> Vec<(String, DeclKind)> {
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
            let mut kind = match keyword {
                "class" | "interface" | "object" | "typealias" => DeclKind::Type,
                "fun" | "val" | "var" => DeclKind::Member,
                _ => return None,
            };
            // `fun interface Handler` declares a type.
            if keyword == "fun" && words.peek().is_some_and(|w| *w == "interface") {
                kind = DeclKind::Type;
            }
            // `fun interface Name` is skipped over.
            words.next_if(|w| *w == "interface");
            let rest = words.collect::<Vec<_>>().join(" ");
            // `fun <reified T> name`: the type parameters, spaces and all, come first.
            let rest = match rest.strip_prefix('<') {
                Some(_) => after_type_parameters(&rest)?,
                None => &rest,
            };
            // The name with its receiver, `Map<K, V>.firstKey`: up to what follows it,
            // where a space inside `<…>` is no end.
            let mut depth = 0;
            let mut previous = ' ';
            let head: String = rest
                .trim_start()
                .chars()
                .take_while(|&c| {
                    match c {
                        '<' => depth += 1,
                        '>' if previous != '-' => depth -= 1,
                        _ => {}
                    }
                    previous = c;
                    depth > 0 || !(c.is_whitespace() || matches!(c, '(' | ':' | '=' | '{'))
                })
                .collect();
            // `List<T>.second` -> `List.second` -> `second`
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
            (!name.is_empty()).then(|| (name.trim_matches('`').to_string(), kind))
        })
        .collect()
}

/// What follows the type parameters `text` starts with: `<K, V> Map` -> ` Map`.
fn after_type_parameters(text: &str) -> Option<&str> {
    let mut depth = 0;
    let mut previous = ' ';
    for (at, c) in text.char_indices() {
        // The `>` of a function type's `->` closes nothing.
        let arrow = previous == '-';
        previous = c;
        match c {
            '<' => depth += 1,
            '>' if arrow => {}
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[at + 1..]);
                }
            }
            _ => {}
        }
    }
    None
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

impl Adapter for Kotlin {
    fn name(&self) -> &'static str {
        "kotlin"
    }

    fn handles(&self, path: &Path) -> bool {
        super::registry::is(path, &super::registry::KOTLIN)
    }

    fn imports(&self, _path: &Path, source: &str) -> Result<Vec<Import>> {
        let tree = tree(&self.language, source);
        Ok(captured_nodes(&self.query, &tree, source)
            .into_iter()
            .map(|node| {
                let text = node.utf8_text(source.as_bytes()).unwrap_or_default();
                Import::at(node, specifier(text), super::Form::Static)
            })
            .collect())
    }

    fn resolve(&self, _file: &Path, specifier: &str) -> super::Resolution {
        let name = specifier.trim_end_matches(".*");
        // One pass over the declarations of the project: the files that may
        // declare the name, and whether an import of it may mean more than one
        // place that should hold one declaration.
        let (candidates, ambiguous): (Vec<String>, bool) = if specifier.ends_with(".*") {
            // A wildcard import means every copy the package holds: several
            // directories of one package (main and test source sets) are no
            // duplicate declaration.
            match self.packages.get(name) {
                Some(dirs) => (dirs.iter().cloned().collect(), false),
                None => match self.declared(name) {
                    Some(files) => (files.keys().cloned().collect(), false),
                    None => (Vec::new(), false),
                },
            }
        } else {
            match self.declared(name) {
                Some(files) => (
                    files.keys().cloned().collect(),
                    // A type one place should hold, declared in several files,
                    // is a real duplicate. Overloads and extensions of one
                    // function or property are not.
                    files.len() > 1 && files.values().any(|kind| *kind == DeclKind::Type),
                ),
                None => (Vec::new(), false),
            }
        };
        let target = candidates
            .first()
            .cloned()
            .map(super::Unit::from)
            .map(Target::Internal)
            .unwrap_or_else(|| Target::External(specifier.to_string()));
        let many = candidates.len() > 1;
        super::Resolution {
            target,
            candidates: if many { candidates } else { Vec::new() },
            ambiguity: ambiguous.then(|| name.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Type parameters may hold spaces (`reified`, bounds, a list), and so may
    /// the receiver of an extension: the name is what follows them.
    #[test]
    fn type_parameters_and_a_receiver_with_spaces_are_no_name() {
        let source = "inline fun <reified T> decode(text: String): T = TODO()\n\
                      fun <K, V> Map<K, V>.firstKey(): K = keys.first()\n\
                      fun <T : Comparable<T>> largest(a: T, b: T) = maxOf(a, b)\n\
                      fun <F : (Int) -> Unit> each(f: F) = f(1)\n";
        assert_eq!(
            top_level_declarations(source)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["decode", "firstKey", "largest", "each"]
        );
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
            top_level_declarations(source)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            [
                "User", "second", "slug", "Registry", "Handler", "Id", "LIMIT"
            ]
        );
        // A `fun` is repeatable, a type and its file-stem name are not.
        let kinds: Vec<_> = top_level_declarations(source)
            .into_iter()
            .map(|(_, kind)| kind)
            .collect();
        assert_eq!(
            kinds,
            [
                DeclKind::Type,
                DeclKind::Member,
                DeclKind::Member,
                DeclKind::Type,
                DeclKind::Type,
                DeclKind::Type,
                DeclKind::Member
            ]
        );
    }

    #[test]
    fn extracts_imports() {
        let kt = Kotlin::new(Path::new("/nonexistent"), &[]).unwrap();
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
        let kt = Kotlin::new(root, &crate::lang::walked(root)).unwrap();
        let go = |spec: &str| kt.resolve(Path::new("A.kt"), spec).target;
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

    /// One pass over the declarations gives both the resolution and the
    /// ambiguity: a type two files declare is resolved to the first by path and
    /// named as ambiguous at the same time.
    #[test]
    fn resolves_and_finds_the_ambiguity_in_one_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for path in ["one/Thing.kt", "two/Thing.kt"] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "package com.acme.thing\n\nclass Thing\n").unwrap();
        }
        let kt = Kotlin::new(root, &crate::lang::walked(root)).unwrap();
        let resolution = kt.resolve(Path::new("A.kt"), "com.acme.thing.Thing");
        assert_eq!(resolution.target, Target::Internal("one/Thing.kt".into()));
        assert_eq!(resolution.candidates, ["one/Thing.kt", "two/Thing.kt"]);
        assert_eq!(
            resolution.ambiguity.as_deref(),
            Some("com.acme.thing.Thing")
        );
    }
}
