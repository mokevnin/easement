//! The `packages` key: a manifest is a checked file, and each dependency it
//! declares is a dependency of it, like one a source file writes. A
//! dev-dependency is test code, so `tests: auto` takes it. What differs by kind
//! of manifest is a field of its entry in the registry
//! ([`crate::lang::registry::Manifest`]), whose code sits in `packages/<kind>.rs`.

pub(crate) mod cargo;
mod entries;
pub(crate) mod go;
pub(crate) mod npm;

pub use entries::{Framework, Setting, entries};

pub(crate) use npm::npm_name;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::check::Unjudged;
use crate::lang::Dep;
use crate::lang::parts::Part;
use crate::lang::registry::{self, Manifest, ManifestFile};
use crate::lang::{Target, Unit};
use crate::relpath;

/// The packages a source file names in its imports, read once per file.
pub enum Named {
    /// Every package the index names is one the file imports.
    Any,
    These(BTreeSet<String>),
}

impl Named {
    pub fn names(&self, package: &str) -> bool {
        match self {
            Named::Any => true,
            Named::These(names) => names.contains(package),
        }
    }
}

/// Where a declared dependency comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A package of the project, by the directory of its manifest relative to the
    /// root, as the kind of manifest resolved it (a path, a workspace's entry).
    Path(String),
    /// A package by its name: a package of the project that goes by it, else a
    /// third-party one.
    Package,
    /// A registry or a repository: a third-party package.
    Registry,
}

/// What of the package it names a dependency turns on, when not just the package: a
/// feature of it (Cargo's), which is the part `part` of its manifest, or the whole
/// package for the one that is (`default`). A third-party package's is its own
/// business, so a dependency that reaches one is none of the project's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    pub part: Option<Part>,
}

/// One dependency a manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// The line of the manifest that declares it, 1-based.
    pub line: usize,
    /// The name it is declared under.
    pub name: String,
    /// The package it is, which differs from the name for a renamed one.
    pub package: String,
    pub source: Source,
    /// The part of the manifest it is in, when that is not the plain dependencies.
    pub part: Option<Part>,
    /// What of the package it turns on, when not just the package.
    pub reaches: Option<Reach>,
}

impl Declared {
    /// A dependency of the plain dependencies, which reaches its package.
    pub fn new(line: usize, name: &str, package: &str, source: Source) -> Self {
        Self {
            line,
            name: name.to_string(),
            package: package.to_string(),
            source,
            part: None,
            reaches: None,
        }
    }
}

/// What a manifest lists: the dependencies it declares, and why some of them are
/// not judged as it declares them.
#[derive(Debug, Default, PartialEq)]
pub struct Listed {
    pub declared: Vec<Declared>,
    pub problems: Vec<Unjudged>,
}

impl From<Vec<Declared>> for Listed {
    fn from(declared: Vec<Declared>) -> Self {
        Self {
            declared,
            problems: Vec::new(),
        }
    }
}

/// The source of a dependency on the package in the directory `path`, written
/// relative to the manifest at `manifest`: that package, or a third-party one when
/// the path leaves the project.
pub(crate) fn path_source(manifest: &str, path: &str) -> Source {
    relpath::resolve(relpath::parent(manifest), path).map_or(Source::Registry, Source::Path)
}

/// What a manifest says the files under it may import, for the `manifests` rule.
#[derive(Debug, PartialEq, Eq)]
pub enum Declares {
    /// The names of the packages it declares, its own included.
    Package(BTreeSet<String>),
    /// It only marks its directory (`{"type": "module"}`): the package is above it.
    Marker,
    /// It cannot be read, so what the files under it import is not checked.
    Unreadable,
}

/// The dependencies the manifests among `files` declare.
pub fn declared_by(
    root: &Path,
    files: &[String],
    read_as_source: impl Fn(&str) -> bool,
) -> Declarations {
    let text_of = |file: &str| std::fs::read_to_string(root.join(file)).ok();
    declared_in(&registry::MANIFESTS, files, &text_of, read_as_source)
}

/// The dependencies the manifests of `kinds` among `files` declare, with `read`
/// giving the text of a file of the project. Each file is read once, however many
/// manifests ask for it: every crate of a large workspace takes most of its
/// dependencies from the workspace manifest.
fn declared_in(
    kinds: &[&'static Manifest],
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
    read_as_source: impl Fn(&str) -> bool,
) -> Declarations {
    let read_once: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
    let text_of = |file: &str| {
        if let Some(text) = read_once.borrow().get(file) {
            return text.clone();
        }
        let text = read(file);
        read_once
            .borrow_mut()
            .insert(file.to_string(), text.clone());
        text
    };
    let mut found = Declarations::default();
    let names = Names::of(kinds, files, &text_of);
    found.problems.extend(names.duplicates.iter().cloned());
    for file in files {
        let Some(kind) = kind_of(kinds, file) else {
            continue;
        };
        if !read_as_source(file) {
            continue;
        }
        found.manifests.push(file.clone());
        let Some(text) = text_of(file) else {
            found.problems.push(
                Unjudged::new(format!(
                    "{file}: cannot be read, so what it declares is not checked"
                ))
                .in_files([file.clone()]),
            );
            continue;
        };
        let manifest = ManifestFile {
            path: file,
            text: &text,
            read: &text_of,
        };
        let Some(listed) = kind.declared(&manifest) else {
            found.problems.push(
                Unjudged::new(format!(
                    "{file}: is not valid {}, so what it declares is not checked",
                    kind.file_name
                ))
                .in_files([file.clone()]),
            );
            continue;
        };
        found.problems.extend(listed.problems);
        for declared in listed.declared {
            let resolved = resolve(kind, file, &declared, files, &names);
            let from = match &declared.part {
                Some(part) => Unit::in_part(Path::new(file), part.clone()),
                None => Unit::from(file.as_str()),
            };
            let (specifier, target) = match (resolved, &declared.reaches) {
                (Resolved::Internal(path), reaches) => {
                    let unit = match reaches.as_ref().and_then(|reach| reach.part.clone()) {
                        Some(part) => Unit::in_part(Path::new(&path), part),
                        None => Unit::from(path.as_str()),
                    };
                    // A package that names itself (`path = "."`) is no dependency.
                    if unit == from {
                        continue;
                    }
                    (path, Target::Internal(unit))
                }
                // What a third-party package turns on is the package's own business.
                (Resolved::External(_), Some(_)) => continue,
                (Resolved::External(module), None) => (module.clone(), Target::External(module)),
            };
            found.deps.push(Dep {
                from,
                ..Dep::module(file, declared.line, specifier, target)
            });
        }
    }
    found
}

/// What the manifests of a project declare.
#[derive(Debug, Default)]
pub struct Declarations {
    /// The manifests that were read, which are checked files.
    pub manifests: Vec<String>,
    pub deps: Vec<Dep>,
    /// Why a manifest, or a dependency of one, was not judged as it declares.
    pub problems: Vec<Unjudged>,
}

/// What a declared dependency is, once resolved against the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The manifest of a package of the project, relative to the root.
    Internal(String),
    /// A third-party package, in the spelling of an import.
    External(String),
}

/// The packages of the project by the name they are known by, for the kinds of manifest
/// that name them: `package.json` by `name`, `go.mod` by `module`.
#[derive(Debug, Default)]
pub struct Names {
    by_name: std::collections::BTreeMap<(&'static Manifest, String), String>,
    /// Names two manifests of a kind declare.
    pub duplicates: Vec<Unjudged>,
}

impl Names {
    /// The packages of the project among `files` that the manifests of `kinds` name.
    fn of(
        kinds: &[&'static Manifest],
        files: &[String],
        read: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let mut names = Self::default();
        let mut sorted: Vec<&String> = files.iter().collect();
        sorted.sort();
        for file in sorted {
            let Some(kind) = kind_of(kinds, file) else {
                continue;
            };
            // Its toolchain skips the directory, and so does the name of a package.
            if kind.is_foreign(file) {
                continue;
            }
            let Some(name) = read(file).and_then(|text| kind.name_in(&text)) else {
                continue;
            };
            match names.by_name.get(&(kind, name.clone())) {
                Some(first) => names.duplicates.push(
                    Unjudged::new(format!(
                        "two `{}` go by `{name}`, {first} and {file}: a dependency on it is taken for the first",
                        kind.file_name
                    ))
                    .in_files([first.clone(), file.clone()]),
                ),
                None => {
                    names.by_name.insert((kind, name), file.clone());
                }
            }
        }
        names
    }

    fn get(&self, kind: &'static Manifest, name: &str) -> Option<&String> {
        self.by_name.get(&(kind, name.to_string()))
    }
}

/// Resolves `declared`, a dependency the manifest `manifest` of `kind` declares,
/// against the project's `files` and the packages that go by a name.
fn resolve(
    kind: &'static Manifest,
    manifest: &str,
    declared: &Declared,
    files: &[String],
    names: &Names,
) -> Resolved {
    let found = match &declared.source {
        Source::Path(dir) => {
            let target = manifest_in(kind, dir);
            files.contains(&target).then_some(target)
        }
        Source::Package => names
            .get(kind, &declared.package)
            .filter(|file| file.as_str() != manifest)
            .cloned(),
        Source::Registry => None,
    };
    found.map_or_else(
        || Resolved::External(kind.spelled_in_import(declared.package.clone())),
        Resolved::Internal,
    )
}

/// The line, from 1, of the byte `offset` of `text`: where a parser's span starts.
fn line_at(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// The kind among `kinds` the manifest `file` is, by its name.
fn kind_of(kinds: &[&'static Manifest], file: &str) -> Option<&'static Manifest> {
    let name = relpath::file_name(file);
    kinds.iter().copied().find(|kind| kind.file_name == name)
}

/// The manifest of `kind` of the package in `dir`.
pub(crate) fn manifest_in(kind: &Manifest, dir: &str) -> String {
    relpath::join(dir, kind.file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::registry::{CARGO_TOML, GO_MOD, PACKAGE_JSON};

    #[test]
    fn names_the_package_of_a_module() {
        assert_eq!(npm_name("@types/lodash"), "lodash");
        assert_eq!(npm_name("@types/babel__core"), "@babel/core");
        assert_eq!(npm_name("react"), "react");
        assert_eq!(PACKAGE_JSON.package("lodash/fp"), "lodash");
        assert_eq!(
            PACKAGE_JSON.package("@mantine/core/styles.css"),
            "@mantine/core"
        );
        assert_eq!(CARGO_TOML.package("serde_json::Value"), "serde_json");
        // A Go module path is its own package, which `covers` matches by prefix.
        assert_eq!(
            GO_MOD.package("github.com/lib/pq/oid"),
            "github.com/lib/pq/oid"
        );
    }

    pub(super) fn importable(kind: &Manifest, text: &str) -> BTreeSet<String> {
        match kind.declares(text) {
            Declares::Package(names) => names,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_what_each_manifest_lets_its_files_import() {
        let npm = importable(
            &PACKAGE_JSON,
            r#"{"name": "app", "dependencies": {"react": "1"}, "devDependencies": {"@types/node": "1", "@types/babel__core": "1"}}"#,
        );
        for name in ["app", "react", "node", "@babel/core"] {
            assert!(npm.contains(name), "{name}");
        }
        assert!(PACKAGE_JSON.covers(&npm, "react"));
        assert!(!PACKAGE_JSON.covers(&npm, "react-dom"));
        let go = importable(
            &GO_MOD,
            "module ex.com/m\n\ngo 1.22\n\nrequire github.com/lib/pq v1.10.0\n\nrequire (\n\tgolang.org/x/sync v0.1.0 // indirect\n\tgithub.com/a/b v1.0.0\n)\n",
        );
        assert!(GO_MOD.covers(&go, "github.com/lib/pq"));
        assert!(GO_MOD.covers(&go, "golang.org/x/sync/errgroup"));
        assert!(GO_MOD.covers(&go, "ex.com/m/internal/x"));
        assert!(!GO_MOD.covers(&go, "github.com/a/bc"));
    }

    #[test]
    fn a_marker_or_a_broken_manifest_declares_no_package() {
        assert_eq!(
            PACKAGE_JSON.declares(r#"{"type": "module"}"#),
            Declares::Marker
        );
        assert_eq!(PACKAGE_JSON.declares("{ not json"), Declares::Unreadable);
        assert_eq!(CARGO_TOML.declares("[dependencies\n"), Declares::Unreadable);
        // A block that never closes: what follows it may be anything.
        assert_eq!(
            GO_MOD.declares("module ex.com/m\n\nrequire (\n\tgithub.com/a/b v1.0.0\n"),
            Declares::Unreadable
        );
    }

    #[test]
    fn an_import_written_in_a_comment_or_a_string_names_no_package() {
        let ts = "// import a from \"ghost\";\nconst s = \"import 'phantom'\";\n\
                  /* require(\"spirit\") */\nimport b from \"real\";\nexport * from \"re\";\n\
                  import c = require(\"old\");\nconst d = await import(\"lazy\");\n";
        let named = PACKAGE_JSON.named_in_source("a.ts", ts);
        for package in ["real", "re", "old", "lazy"] {
            assert!(named.names(package), "{package} is imported");
        }
        for package in ["ghost", "phantom", "spirit"] {
            assert!(!named.names(package), "{package} is no import");
        }
    }

    #[test]
    fn only_a_package_the_source_imports_has_to_be_declared() {
        let ts =
            "import { a } from \"pkg/sub\";\nlet t: NodeJS.Timeout;\nimport fs from \"node:fs\";\n";
        let named = PACKAGE_JSON.named_in_source("a.ts", ts);
        assert!(named.names("pkg"));
        assert!(!named.names("node"));
        assert!(!named.names("b"));
        // A path names a file of the project, not the package `.` or ``.
        let relative = "import { h } from \"./helper\";\nimport x from \"/abs/x\";\n";
        let named = PACKAGE_JSON.named_in_source("a.ts", relative);
        assert!(!named.names("."));
        assert!(!named.names(""));
        let rs = "use serde::de::Error;\nextern crate libc;\nfn f() { other::x(); }\n";
        let named = CARGO_TOML.named_in_source("a.rs", rs);
        assert!(named.names("serde"));
        assert!(named.names("libc"));
        assert!(!named.names("serde_core"));
        assert!(
            !CARGO_TOML
                .named_in_source("a.rs", "use my::serde::x;")
                .names("serde")
        );
        // A Go import is the package path itself.
        assert!(GO_MOD.named_in_source("a.go", "").names("anything"));
    }

    fn dep(name: &str, source: Source) -> Declared {
        Declared::new(1, name, name, source)
    }

    fn cargo_project() -> Vec<String> {
        [
            "Cargo.toml",
            "crates/storage-api/Cargo.toml",
            "crates/chunk-store/Cargo.toml",
            "crates/local/Cargo.toml",
        ]
        .map(str::to_string)
        .to_vec()
    }

    #[test]
    fn a_path_dependency_is_the_manifest_of_a_package_of_the_project() {
        let files = cargo_project();
        let from = "crates/chunk-store/Cargo.toml";
        let got = resolve(
            &CARGO_TOML,
            from,
            &dep("local", Source::Path("crates/local".into())),
            &files,
            &Names::default(),
        );
        assert_eq!(got, Resolved::Internal("crates/local/Cargo.toml".into()));
        // A path with no manifest of the project there names nothing of it.
        let missing = resolve(
            &CARGO_TOML,
            from,
            &dep("gone", Source::Path("crates/gone".into())),
            &files,
            &Names::default(),
        );
        assert_eq!(missing, Resolved::External("gone".into()));
    }

    #[test]
    fn a_third_party_package_is_spelled_as_an_import_spells_it() {
        let mut renamed = dep("json", Source::Registry);
        renamed.package = "serde-json".into();
        assert_eq!(
            resolve(
                &CARGO_TOML,
                "crates/local/Cargo.toml",
                &renamed,
                &cargo_project(),
                &Names::default()
            ),
            Resolved::External("serde_json".into())
        );
    }

    /// Resolves each dependency a manifest of a project declares.
    fn resolved(
        kind: &'static Manifest,
        manifest: &str,
        project: &[(&str, &str)],
    ) -> Vec<(String, Resolved)> {
        let files: Vec<String> = project.iter().map(|(p, _)| p.to_string()).collect();
        let read = |path: &str| {
            project
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        let names = Names::of(&registry::MANIFESTS, &files, &read);
        let text = read(manifest).unwrap();
        let file = ManifestFile {
            path: manifest,
            text: &text,
            read: &read,
        };
        kind.declared(&file)
            .unwrap()
            .declared
            .iter()
            .map(|d| (d.name.clone(), resolve(kind, manifest, d, &files, &names)))
            .collect()
    }

    #[test]
    fn an_npm_dependency_is_the_package_json_that_goes_by_its_name_or_its_path() {
        let got = resolved(
            &PACKAGE_JSON,
            "packages/web/package.json",
            &[
                ("packages/api/package.json", "{\"name\": \"@acme/api\"}"),
                ("packages/local/package.json", "{\"name\": \"local\"}"),
                (
                    "packages/web/package.json",
                    "{\"name\": \"web\", \"dependencies\": {\"@acme/api\": \"workspace:*\", \"lodash\": \"^4\", \"loc\": \"file:../local\", \"web\": \"workspace:*\"}}",
                ),
            ],
        );
        let internal = |p: &str| Resolved::Internal(p.to_string());
        assert_eq!(
            got,
            [
                (
                    "@acme/api".to_string(),
                    internal("packages/api/package.json")
                ),
                (
                    "lodash".to_string(),
                    Resolved::External("lodash".to_string())
                ),
                ("loc".to_string(), internal("packages/local/package.json")),
                // A package that goes by the manifest's own name is not it.
                ("web".to_string(), Resolved::External("web".to_string())),
            ]
        );
    }

    #[test]
    fn a_go_module_is_the_go_mod_that_declares_its_path_or_replaces_it() {
        let got = resolved(
            &GO_MOD,
            "app/go.mod",
            &[
                (
                    "app/go.mod",
                    "module example.com/app\n\nrequire (\n\texample.com/lib v1.0.0\n\tgithub.com/lib/pq v1.10.0\n\texample.com/moved v1.0.0\n\texample.com/outside v1.0.0\n)\n\nreplace example.com/moved => ../moved\nreplace example.com/outside => ../../outside\n",
                ),
                ("lib/go.mod", "module example.com/lib\n"),
                ("moved/go.mod", "module example.com/moved\n"),
            ],
        );
        let internal = |p: &str| Resolved::Internal(p.to_string());
        assert_eq!(
            got,
            [
                ("example.com/lib".to_string(), internal("lib/go.mod")),
                (
                    "github.com/lib/pq".to_string(),
                    Resolved::External("github.com/lib/pq".to_string())
                ),
                ("example.com/moved".to_string(), internal("moved/go.mod")),
                // A directory outside the project is a module of someone else's.
                (
                    "example.com/outside".to_string(),
                    Resolved::External("example.com/outside".to_string())
                ),
            ]
        );
    }

    #[test]
    fn two_manifests_that_go_by_one_name_are_said() {
        let project = [
            ("a/package.json", "{\"name\": \"dup\"}"),
            ("b/package.json", "{\"name\": \"dup\"}"),
        ];
        let files: Vec<String> = project.iter().map(|(p, _)| p.to_string()).collect();
        let read = |path: &str| {
            project
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        let names = Names::of(&registry::MANIFESTS, &files, &read);
        assert_eq!(names.duplicates.len(), 1);
        assert!(names.duplicates[0].reason.contains("dup"));
        assert_eq!(
            names.duplicates[0].files,
            ["a/package.json", "b/package.json"]
        );
    }

    /// `testdata/` and `vendor/` are Go's: npm holds what is under them a package
    /// of the project like any other.
    #[test]
    fn a_package_json_under_test_data_or_vendor_goes_by_its_name() {
        let got = resolved(
            &PACKAGE_JSON,
            "web/package.json",
            &[
                ("pkg/testdata/package.json", "{\"name\": \"@acme/x\"}"),
                ("vendor/y/package.json", "{\"name\": \"@acme/y\"}"),
                (
                    "web/package.json",
                    "{\"name\": \"web\", \"dependencies\": {\"@acme/x\": \"*\", \"@acme/y\": \"*\"}}",
                ),
            ],
        );
        let internal = |name: &str, p: &str| (name.to_string(), Resolved::Internal(p.to_string()));
        assert_eq!(
            got,
            [
                internal("@acme/x", "pkg/testdata/package.json"),
                internal("@acme/y", "vendor/y/package.json"),
            ]
        );
    }

    #[test]
    fn a_vendored_or_test_data_manifest_does_not_count_as_a_package_of_the_project() {
        let project = [
            ("a/go.mod", "module example.com/a\n"),
            ("vendor/x/go.mod", "module example.com/a\n"),
            ("pkg/testdata/go.mod", "module example.com/a\n"),
        ];
        let files: Vec<String> = project.iter().map(|(p, _)| p.to_string()).collect();
        let read = |path: &str| {
            project
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        let names = Names::of(&registry::MANIFESTS, &files, &read);
        assert!(names.duplicates.is_empty(), "{:?}", names.duplicates);
    }

    /// A kind of manifest of the test's own: `deps.txt` lists one dependency a line,
    /// `name` (a package by its name), `name ../dir` (a path), `name ../dir part`
    /// (a part of that package it turns on), `ext +part` (a part of a third-party
    /// one); `name = n` names its package, and `see shared.txt` reads that file.
    fn fake_declared(file: &ManifestFile) -> Option<Listed> {
        let mut listed = Listed::default();
        for (i, line) in file.text.lines().enumerate() {
            let words: Vec<&str> = line.split_whitespace().collect();
            let (source, reaches) = match words.as_slice() {
                ["name", "=", _] => continue,
                ["see", other] => {
                    if (file.read)(other).is_none() {
                        listed
                            .problems
                            .push(Unjudged::new(format!("{other} cannot be read")));
                    }
                    continue;
                }
                [_] => (Source::Package, None),
                [_, path] if path.starts_with('+') => (
                    Source::Registry,
                    Some(Reach {
                        part: Some(Part {
                            name: path[1..].to_string(),
                            kind: None,
                        }),
                    }),
                ),
                [_, path] => (path_source(file.path, path), None),
                [_, path, part] => (
                    path_source(file.path, path),
                    Some(Reach {
                        part: Some(Part {
                            name: part.to_string(),
                            kind: None,
                        }),
                    }),
                ),
                _ => return None,
            };
            listed.declared.push(Declared {
                reaches,
                ..Declared::new(i + 1, words[0], words[0], source)
            });
        }
        Some(listed)
    }

    fn fake_name(text: &str) -> Option<String> {
        text.lines()
            .find_map(|line| line.strip_prefix("name = "))
            .map(str::to_string)
    }

    static DEPS_TXT: Manifest = Manifest {
        file_name: "deps.txt",
        part_names: &[],
        declared: fake_declared,
        declares: |_| Declares::Marker,
        package: None,
        named_by: None,
        covers: None,
        named_in_source: None,
        namer: Some(fake_name),
        spelled_in_import: None,
        entries: None,
        frameworks: None,
    };

    /// (file, line, what it reaches and the part of it) for each dependency.
    fn fake_edges(found: &Declarations) -> Vec<(String, usize, String, Option<String>)> {
        found
            .deps
            .iter()
            .map(|dep| {
                let (key, part) = match &dep.target {
                    Target::Internal(unit) => (unit.key(), unit.part_name().map(str::to_string)),
                    Target::External(module) => (module.clone(), None),
                };
                (crate::relpath::slash(&dep.file), dep.line, key, part)
            })
            .collect()
    }

    /// What `declared_by` does is the same for every kind of manifest: it resolves
    /// what the kind declares against the project, by path or by the name a package
    /// goes by, drops what a third-party package turns on, and says what the kind
    /// could not read.
    #[test]
    fn every_kind_of_manifest_is_resolved_against_the_project_alike() {
        let project = [
            (
                "a/deps.txt",
                "name = a\nb ../b\nb ../b fast\nc\next\next +fast\ngone ../gone\nsee shared.txt\n",
            ),
            ("b/deps.txt", "name = b\n"),
            ("c/deps.txt", "name = c\n"),
            ("d/deps.txt", "not ( a list\n"),
            ("e/deps.txt", "see nowhere.txt\n"),
            ("shared.txt", ""),
        ];
        let files: Vec<String> = project.iter().map(|(p, _)| p.to_string()).collect();
        let read = |path: &str| {
            project
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        let found = declared_in(&[&DEPS_TXT], &files, &read, |file| file != "c/deps.txt");
        let s = |x: &str| x.to_string();
        assert_eq!(
            fake_edges(&found),
            [
                (s("a/deps.txt"), 2, s("b/deps.txt"), None),
                (s("a/deps.txt"), 3, s("b/deps.txt"), Some(s("fast"))),
                (s("a/deps.txt"), 4, s("c/deps.txt"), None),
                (s("a/deps.txt"), 5, s("ext"), None),
                (s("a/deps.txt"), 7, s("gone"), None),
            ]
        );
        // A manifest the policy reads only as a target is no checked file.
        assert_eq!(
            found.manifests,
            ["a/deps.txt", "b/deps.txt", "d/deps.txt", "e/deps.txt"]
        );
        let reasons: Vec<&str> = found.problems.iter().map(|p| p.reason.as_str()).collect();
        assert_eq!(
            reasons,
            [
                "d/deps.txt: is not valid deps.txt, so what it declares is not checked",
                "nowhere.txt cannot be read",
            ]
        );
    }

    /// Every crate of a large workspace takes most of its dependencies from the
    /// workspace manifest: each file of the project is read once a run, however many
    /// manifests ask for it.
    #[test]
    fn a_file_is_read_once_however_many_manifests_ask_for_it() {
        let project = [
            ("a/deps.txt", "see shared.txt\nsee shared.txt\n"),
            ("b/deps.txt", "see shared.txt\n"),
            ("shared.txt", ""),
        ];
        let files: Vec<String> = ["a/deps.txt", "b/deps.txt"].map(str::to_string).to_vec();
        let reads = RefCell::new(Vec::new());
        let read = |path: &str| {
            reads.borrow_mut().push(path.to_string());
            project
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        declared_in(&[&DEPS_TXT], &files, &read, |_| true);
        let mut reads = reads.into_inner();
        reads.sort();
        assert_eq!(reads, ["a/deps.txt", "b/deps.txt", "shared.txt"]);
    }
}
