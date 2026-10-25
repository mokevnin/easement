use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::{Language, Node};

use super::{Adapter, Import, Target, parse, relative};

/// Rust. `use` declarations and `mod x;` are resolved through the module tree
/// of the crate that owns the file: `crate::`, `self::` and `super::` walk it,
/// and the first segment may name another crate of the workspace. Everything
/// else, including `std`, is external.
///
/// A path resolves to the deepest module file it passes through, so
/// `use crate::db::Pool` points at `db.rs` or `db/mod.rs`. Items re-exported
/// through another module are attributed to the module that is named.
pub struct Rust {
    root: PathBuf,
    language: Language,
    /// Crates by their name with `-` written as `_`, the way code refers to them.
    crates: BTreeMap<String, Crate>,
    /// Every `.rs` file of the project. Module files are looked up here rather
    /// than on disk, so `Pool` never finds `pool.rs` on a case-insensitive file system.
    files: BTreeSet<PathBuf>,
}

#[derive(Debug, Clone)]
struct Crate {
    /// Directory of the Cargo.toml, relative to the project root.
    dir: PathBuf,
    /// `src/lib.rs`, else `src/main.rs`, relative to the project root.
    root_file: PathBuf,
}

impl Rust {
    pub fn new(root: &Path) -> Result<Self> {
        let mut files = BTreeSet::new();
        let mut manifests = Vec::new();
        for entry in crate::scan::walker(root).build().flatten() {
            let path = entry.path();
            let Some(rel) = relative(root, path) else {
                continue;
            };
            match path.file_name().and_then(|n| n.to_str()) {
                Some("Cargo.toml") => manifests.push((rel, path.to_path_buf())),
                Some(name) if name.ends_with(".rs") => {
                    files.insert(rel);
                }
                _ => {}
            }
        }
        let mut crates = BTreeMap::new();
        for (rel, path) in manifests {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let dir = rel.parent().unwrap_or(Path::new("")).to_path_buf();
            let Some(name) = package_name(&text) else {
                continue; // a virtual workspace manifest
            };
            let root_file = ["src/lib.rs", "src/main.rs"]
                .iter()
                .map(|f| dir.join(f))
                .find(|f| files.contains(f));
            if let Some(root_file) = root_file {
                crates.insert(name.replace('-', "_"), Crate { dir, root_file });
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            language: tree_sitter_rust::LANGUAGE.into(),
            crates,
            files,
        })
    }

    fn crate_of(&self, file: &Path) -> Option<&Crate> {
        self.crates
            .values()
            .filter(|c| file.starts_with(&c.dir))
            .max_by_key(|c| c.dir.components().count())
    }
}

/// `name` of the `[package]` table, read line by line.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some(rest) = line.strip_prefix("name")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}::{name}")
    }
}

/// Every path a `use` argument stands for: `a::{b, c::d, self}` is `a::b`,
/// `a::c::d` and `a`.
fn flatten(node: Node, source: &str, prefix: &str, out: &mut Vec<String>) {
    let path = |n: Node| text(n, source).split_whitespace().collect::<String>();
    match node.kind() {
        "scoped_use_list" => {
            let prefix = join(
                prefix,
                &node
                    .child_by_field_name("path")
                    .map_or_else(String::new, path),
            );
            if let Some(list) = node.child_by_field_name("list") {
                flatten(list, source, &prefix, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                flatten(child, source, prefix, out);
            }
        }
        "use_as_clause" => {
            if let Some(inner) = node.child_by_field_name("path") {
                flatten(inner, source, prefix, out);
            }
        }
        "use_wildcard" => {
            let mut cursor = node.walk();
            let inner = node.named_children(&mut cursor).next();
            out.push(join(prefix, &inner.map_or_else(String::new, path)));
        }
        "self" => out.push(prefix.to_string()),
        _ => out.push(join(prefix, &path(node))),
    }
}

/// Rewrites a path found inside `depth` inline modules so that it is relative
/// to the module of the file; `None` when it only refers to the inline modules.
fn relative_to_file(path: &str, depth: usize) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    if depth == 0 {
        return Some(path.to_string());
    }
    let mut segments = path.split("::");
    match segments.next()? {
        "self" => None,
        "super" => {
            let mut up = 1;
            let mut rest: Vec<&str> = Vec::new();
            for segment in segments {
                if segment == "super" && rest.is_empty() {
                    up += 1;
                } else {
                    rest.push(segment);
                }
            }
            if up <= depth {
                return None;
            }
            let mut out = vec!["super"; up - depth];
            out.extend(rest);
            Some(out.join("::"))
        }
        _ => Some(path.to_string()),
    }
}

fn collect(node: Node, source: &str, depth: usize, out: &mut Vec<Import>) {
    let line = node.start_position().row + 1;
    match node.kind() {
        "use_declaration" => {
            let mut paths = Vec::new();
            if let Some(argument) = node.child_by_field_name("argument") {
                flatten(argument, source, "", &mut paths);
            }
            out.extend(paths.iter().filter_map(|p| {
                relative_to_file(p, depth).map(|specifier| Import { specifier, line })
            }));
            return;
        }
        "mod_item" => {
            let name = node.child_by_field_name("name").map(|n| text(n, source));
            match (node.child_by_field_name("body"), name) {
                (Some(body), _) => {
                    collect(body, source, depth + 1, out);
                    return;
                }
                (None, Some(name)) if depth == 0 => {
                    out.push(Import {
                        specifier: format!("self::{name}"),
                        line,
                    });
                    return;
                }
                _ => return,
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, depth, out);
    }
}

/// Where the children of the module in `file` live: `mod x;` there is `x.rs`
/// or `x/mod.rs`. A root file or a `mod.rs` owns its own directory, any other
/// file owns the directory named like it.
fn children_dir(file: &Path, is_root: bool) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new("")).to_path_buf();
    if is_root || file.file_name().is_some_and(|n| n == "mod.rs") {
        parent
    } else {
        parent.join(file.file_stem().unwrap_or_default())
    }
}

impl Rust {
    /// A crate root, or a file outside `src/` that is a root of its own
    /// (integration tests, examples, build scripts).
    fn is_root(&self, file: &Path) -> bool {
        match self.crate_of(file) {
            Some(c) => {
                c.root_file == file
                    || !file.starts_with(c.dir.join("src"))
                    || file.starts_with(c.dir.join("src/bin"))
            }
            None => true,
        }
    }

    fn module_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        [
            dir.join(format!("{name}.rs")),
            dir.join(name).join("mod.rs"),
        ]
        .into_iter()
        .find(|c| self.files.contains(c))
    }

    /// The module that holds `file`, if the tree says so.
    fn parent_module(&self, file: &Path) -> Option<PathBuf> {
        if self.is_root(file) {
            return None;
        }
        let krate = self.crate_of(file)?;
        let holder = if file.file_name().is_some_and(|n| n == "mod.rs") {
            file.parent()?.parent()?
        } else {
            file.parent()?
        };
        if holder == krate.root_file.parent()? {
            return Some(krate.root_file.clone());
        }
        self.module_file(holder.parent()?, holder.file_name()?.to_str()?)
    }
}

impl Adapter for Rust {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "rs")
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        let tree = parse(&self.language, path, source)?;
        let mut imports = Vec::new();
        collect(tree.root_node(), source, 0, &mut imports);
        Ok(imports)
    }

    fn resolve(&self, file: &Path, specifier: &str) -> Target {
        let Some(file) = relative(&self.root, file) else {
            return Target::Unresolved;
        };
        let mut segments = specifier.split("::").filter(|s| !s.is_empty()).peekable();
        let Some(&first) = segments.peek() else {
            return Target::Unresolved;
        };
        let own = self.crate_of(&file);
        let is_root = self.is_root(&file);
        let mut current = match first {
            "crate" => {
                segments.next();
                match own {
                    Some(c)
                        if file.starts_with(c.dir.join("src"))
                            && !file.starts_with(c.dir.join("src/bin")) =>
                    {
                        c.root_file.clone()
                    }
                    _ => file.clone(),
                }
            }
            "self" => {
                segments.next();
                file.clone()
            }
            "super" => {
                let mut module = file.clone();
                while segments.peek() == Some(&"super") {
                    segments.next();
                    match self.parent_module(&module) {
                        Some(parent) => module = parent,
                        None => return Target::Unresolved,
                    }
                }
                module
            }
            name => {
                let local = self.module_file(&children_dir(&file, is_root), name);
                match (local, self.crates.get(name)) {
                    (Some(_), _) => file.clone(),
                    (None, Some(krate)) => {
                        segments.next();
                        krate.root_file.clone()
                    }
                    (None, None) => return Target::External(specifier.to_string()),
                }
            }
        };
        for segment in segments {
            let is_root = self.is_root(&current);
            match self.module_file(&children_dir(&current, is_root), segment) {
                Some(next) => current = next,
                None => break,
            }
        }
        Target::Internal(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    fn specifiers(source: &str) -> Vec<(String, usize)> {
        Rust::new(Path::new("/nonexistent"))
            .unwrap()
            .imports(Path::new("x.rs"), source)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect()
    }

    #[test]
    fn extracts_use_trees_and_mod_declarations() {
        let got = specifiers(
            "use std::collections::HashMap;
use crate::{a::b, c::{self, d as e}, f::*};
mod g;
mod h { use super::i; use super::super::j; use self::k; }
fn f() { use crate::l; }
",
        );
        let names: Vec<&str> = got.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "std::collections::HashMap",
                "crate::a::b",
                "crate::c",
                "crate::c::d",
                "crate::f",
                "self::g",
                "super::j",
                "crate::l",
            ]
        );
        assert_eq!(got[7].1, 5);
    }

    #[test]
    fn reads_the_package_name() {
        assert_eq!(
            package_name(
                "[workspace]\nmembers = []\n[package]\nname = \"my-app\"\nversion = \"1\"\n"
            ),
            Some("my-app".to_string())
        );
        assert_eq!(package_name("[workspace]\nname = \"x\"\n"), None);
    }

    #[test]
    fn resolves_through_the_module_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            &[
                ("Cargo.toml", "[package]\nname = \"app\"\n"),
                ("src/lib.rs", "mod api;\nmod db;\n"),
                ("src/api.rs", "mod routes;\n"),
                ("src/api/routes.rs", ""),
                ("src/db/mod.rs", "mod pool;\n"),
                ("src/db/pool.rs", ""),
                ("tests/it.rs", ""),
                ("util/Cargo.toml", "[package]\nname = \"my-util\"\n"),
                ("util/src/lib.rs", "mod text;\n"),
                ("util/src/text.rs", ""),
            ],
        );
        let rust = Rust::new(root).unwrap();
        let go = |file: &str, spec: &str| rust.resolve(&root.join(file), spec);
        let internal = |p: &str| Target::Internal(PathBuf::from(p));

        assert_eq!(
            go("src/api/routes.rs", "crate::db::pool::Pool"),
            internal("src/db/pool.rs")
        );
        assert_eq!(
            go("src/api/routes.rs", "crate::db::Pool"),
            internal("src/db/mod.rs")
        );
        assert_eq!(
            go("src/api/routes.rs", "crate::Config"),
            internal("src/lib.rs")
        );
        assert_eq!(
            go("src/api/routes.rs", "super::Thing"),
            internal("src/api.rs")
        );
        assert_eq!(
            go("src/api/routes.rs", "super::super::db"),
            internal("src/db/mod.rs")
        );
        assert_eq!(
            go("src/db/pool.rs", "super::Helper"),
            internal("src/db/mod.rs")
        );
        assert_eq!(
            go("src/api.rs", "self::routes"),
            internal("src/api/routes.rs")
        );
        assert_eq!(
            go("src/api.rs", "routes::handler"),
            internal("src/api/routes.rs")
        );
        assert_eq!(go("src/lib.rs", "api"), internal("src/api.rs"));
        assert_eq!(
            go("tests/it.rs", "app::db::Pool"),
            internal("src/db/mod.rs")
        );
        assert_eq!(
            go("src/lib.rs", "my_util::text::wrap"),
            internal("util/src/text.rs")
        );
        assert_eq!(
            go("src/api.rs", "serde::Deserialize"),
            Target::External("serde::Deserialize".into())
        );
        assert_eq!(go("src/lib.rs", "super::x"), Target::Unresolved);
    }
}
