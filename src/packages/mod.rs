//! The `packages` key: a manifest is a checked file, and each dependency it
//! declares is a dependency of it, like one a source file writes. A
//! dev-dependency is test code, so `tests: auto` takes it.

mod cargo;
mod go;
mod npm;

pub(crate) use npm::npm_name;

use std::collections::BTreeSet;
use std::path::Path;

use cargo::Workspaces;

use crate::check::Unjudged;
use crate::lang::Dep;
use crate::lang::parts::Part;
use crate::lang::registry::{self, Manifest};
use crate::lang::{Target, Unit};

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
use crate::relpath;

/// Where a declared dependency comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A package of the project, by the path of its directory as written.
    Path(String),
    /// `workspace = true`: what the workspace manifest declares under the name.
    Workspace,
    /// A package by its name: a package of the project that goes by it, else a
    /// third-party one.
    Package,
    /// A registry or a repository: a third-party package.
    Registry,
    /// The manifest itself: an item of a feature that names another feature of it.
    Own,
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
    /// The table it is in, when that is not the plain dependencies.
    pub part: Option<Part>,
    /// The `cfg` of the `[target.<cfg>.dependencies]` table it is in, when it is in one.
    pub target: Option<String>,
    /// The feature of the package it reaches, when it is not the package itself.
    pub feature: Option<String>,
    /// Whether the table declares it `optional = true`: a feature turns it on.
    pub optional: bool,
}

impl Declared {
    /// A dependency of the plain tables, with none of the features of Cargo.
    pub fn new(line: usize, name: &str, package: &str, source: Source) -> Self {
        Self {
            line,
            name: name.to_string(),
            package: package.to_string(),
            source,
            part: None,
            target: None,
            feature: None,
            optional: false,
        }
    }
}

/// The dependencies a manifest of `kind` declares, with the line of each.
/// `None` for a manifest that cannot be read, which is not one that declares nothing.
pub fn declared(kind: Manifest, text: &str) -> Option<Vec<Declared>> {
    match kind {
        Manifest::Cargo => cargo::declared(text),
        Manifest::Npm => npm::declared(text),
        Manifest::Go => go::declared(text),
    }
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

/// The one place that tells the kinds of manifest apart: each method asks the
/// module of its kind.
impl Manifest {
    /// The package an external module belongs to: `lodash` for `lodash/fp`,
    /// the crate of a Rust path. A Go module path stays whole: [`Manifest::covers`]
    /// matches it by prefix.
    pub fn package(self, module: &str) -> String {
        match self {
            Manifest::Npm => npm::package(module),
            Manifest::Go => module.to_string(),
            Manifest::Cargo => cargo::package(module),
        }
    }

    /// The package a specifier names when it is spelled as one of this kind,
    /// read from its text alone: `some-pkg` of `some-pkg/style.css`, `@scope/pkg`
    /// of `@scope/pkg/x`. `None` for anything else (a path, `#lib/x`, `virtual:x`),
    /// and for every specifier of a kind whose imports are not spelled so.
    pub fn named_by(self, specifier: &str) -> Option<String> {
        match self {
            Manifest::Npm => npm::named_by(specifier),
            Manifest::Go | Manifest::Cargo => None,
        }
    }

    /// What the files under a manifest with `text` may import.
    pub fn declares(self, text: &str) -> Declares {
        match self {
            Manifest::Npm => npm::importable(text),
            Manifest::Go => go::importable(text),
            Manifest::Cargo => cargo::importable(text),
        }
    }

    /// Whether a manifest that declares `names` covers `package`.
    pub fn covers(self, names: &BTreeSet<String>, package: &str) -> bool {
        match self {
            Manifest::Go => go::covers(names, package),
            Manifest::Npm | Manifest::Cargo => names.contains(package),
        }
    }

    /// The packages `source`, the text of `file`, names in an import. The index names the package
    /// that defines a symbol, which differs from the one the code imports for a
    /// re-export (`serde` re-exports `serde_core`), a global type
    /// (`NodeJS.Timeout`) or a Node builtin; only the imported one has to be
    /// declared. A Go import is the package path itself.
    pub fn named_in_source(self, file: &str, source: &str) -> Named {
        match self {
            Manifest::Npm => Named::These(npm::named_in_source(file, source)),
            Manifest::Cargo => Named::These(cargo::named_in_source(source)),
            Manifest::Go => Named::Any,
        }
    }

    /// What reads the name a manifest gives its package, which a dependency of
    /// another manifest of the project can name: `name` of `package.json`, `module`
    /// of `go.mod`. A Cargo dependency names a path or the workspace instead.
    fn namer(self) -> Option<fn(&str) -> Option<String>> {
        match self {
            Manifest::Npm => Some(npm::name),
            Manifest::Go => Some(go::name),
            Manifest::Cargo => None,
        }
    }

    /// How an import spells the external `package`: Rust spells a crate with `_`.
    fn spelled_in_import(self, package: String) -> String {
        match self {
            Manifest::Cargo => package.replace('-', "_"),
            Manifest::Npm | Manifest::Go => package,
        }
    }

    /// The files of the project a manifest in `dir` names as what its package runs
    /// or exports, which `unused` counts as entries.
    pub fn entries(self, text: &str, dir: &str, files: &BTreeSet<&str>) -> Vec<String> {
        match self {
            Manifest::Npm => npm::entries(text, dir, files),
            Manifest::Go | Manifest::Cargo => Vec::new(),
        }
    }
}

/// The dependencies the manifests among `files` declare.
pub fn declared_by(
    root: &Path,
    files: &[String],
    read_as_source: impl Fn(&str) -> bool,
) -> Declarations {
    let text_of = |file: &str| std::fs::read_to_string(root.join(file)).ok();
    let mut found = Declarations::default();
    let names = Names::of(files, &text_of);
    let mut workspaces = Workspaces::new(&text_of);
    found.problems.extend(names.duplicates.iter().cloned());
    for file in files {
        let Some(kind) = registry::manifest_of(file) else {
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
        let Some(list) = declared(kind, &text) else {
            found.problems.push(
                Unjudged::new(format!(
                    "{file}: is not valid {}, so what it declares is not checked",
                    kind.file_name()
                ))
                .in_files([file.clone()]),
            );
            continue;
        };
        for declared in list {
            let (resolved, problem) = resolve(file, &declared, files, &mut workspaces, &names);
            found.problems.extend(problem);
            let from = match &declared.part {
                Some(part) => Unit::in_part(Path::new(file), part.clone()),
                None => Unit::from(file.as_str()),
            };
            let (specifier, target) = match (resolved, &declared.feature) {
                (Resolved::Internal(path), feature) => {
                    let unit = match feature.as_deref().filter(|f| *f != "default") {
                        Some(feature) => Unit::in_part(
                            Path::new(&path),
                            Part {
                                name: format!("feature/{feature}"),
                                kind: None,
                            },
                        ),
                        None => Unit::from(path.as_str()),
                    };
                    // A package that names itself (`path = "."`) is no dependency.
                    if unit == from {
                        continue;
                    }
                    (path, Target::Internal(unit))
                }
                // A feature of a third-party package is the package's own business.
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
    by_name: std::collections::BTreeMap<(Manifest, String), String>,
    /// Names two manifests of a kind declare.
    pub duplicates: Vec<Unjudged>,
}

impl Names {
    pub fn of(files: &[String], read: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut names = Self::default();
        let mut sorted: Vec<&String> = files.iter().collect();
        sorted.sort();
        for file in sorted {
            let Some((kind, namer)) =
                registry::manifest_of(file).and_then(|kind| Some((kind, kind.namer()?)))
            else {
                continue;
            };
            // Its toolchain skips the directory, and so does the name of a package.
            if kind.is_foreign(file) {
                continue;
            }
            let Some(name) = read(file).and_then(|text| namer(&text)) else {
                continue;
            };
            match names.by_name.get(&(kind, name.clone())) {
                Some(first) => names.duplicates.push(
                    Unjudged::new(format!(
                        "two `{}` go by `{name}`, {first} and {file}: a dependency on it is taken for the first",
                        kind.file_name()
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

    fn get(&self, kind: Manifest, name: &str) -> Option<&String> {
        self.by_name.get(&(kind, name.to_string()))
    }
}

/// Resolves what the manifest at `manifest` declares as `declared`. `files` are
/// the project's files; `workspaces` are the workspace manifests above them.
fn resolve(
    manifest: &str,
    declared: &Declared,
    files: &[String],
    workspaces: &mut Workspaces,
    names: &Names,
) -> (Resolved, Option<Unjudged>) {
    let Some(kind) = registry::manifest_of(manifest) else {
        return (Resolved::External(declared.package.clone()), None);
    };
    let internal = |dir: &str, path: &str| {
        let target = relpath::resolve(dir, path).map(|dir| manifest_in(kind, &dir))?;
        files.contains(&target).then_some(target)
    };
    let dir = relpath::parent(manifest);
    let mut package = declared.package.clone();
    let mut problem = None;
    let found = match &declared.source {
        Source::Path(path) => internal(dir, path),
        Source::Package => names
            .get(kind, &declared.package)
            .filter(|file| file.as_str() != manifest)
            .cloned(),
        Source::Workspace => match workspaces.dependency(dir, &declared.name) {
            Ok(Some(entry)) => {
                if let Some(real) = entry.package {
                    package = real;
                }
                entry.path.and_then(|path| internal(&entry.dir, &path))
            }
            Ok(None) => None,
            Err(workspace) => {
                problem = Some(
                    Unjudged::new(format!(
                        "{manifest}: `{}` is `workspace = true`, but {workspace} cannot be read, so it is taken for an external crate",
                        declared.name
                    ))
                    .in_files([manifest.to_string()]),
                );
                None
            }
        },
        Source::Own => Some(manifest.to_string()),
        Source::Registry => None,
    };
    let resolved = found.map_or_else(
        || Resolved::External(kind.spelled_in_import(package)),
        Resolved::Internal,
    );
    (resolved, problem)
}

/// The line, from 1, of the byte `offset` of `text`: where a parser's span starts.
fn line_at(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// The manifest of `kind` of the package in `dir`.
pub(super) fn manifest_in(kind: Manifest, dir: &str) -> String {
    relpath::join(dir, kind.file_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::parts::Kind;

    #[test]
    fn names_the_package_of_a_module() {
        assert_eq!(npm_name("@types/lodash"), "lodash");
        assert_eq!(npm_name("@types/babel__core"), "@babel/core");
        assert_eq!(npm_name("react"), "react");
        assert_eq!(Manifest::Npm.package("lodash/fp"), "lodash");
        assert_eq!(
            Manifest::Npm.package("@mantine/core/styles.css"),
            "@mantine/core"
        );
        assert_eq!(Manifest::Cargo.package("serde_json::Value"), "serde_json");
    }

    fn importable(kind: Manifest, text: &str) -> BTreeSet<String> {
        match kind.declares(text) {
            Declares::Package(names) => names,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_what_each_manifest_lets_its_files_import() {
        let npm = importable(
            Manifest::Npm,
            r#"{"name": "app", "dependencies": {"react": "1"}, "devDependencies": {"@types/node": "1", "@types/babel__core": "1"}}"#,
        );
        for name in ["app", "react", "node", "@babel/core"] {
            assert!(npm.contains(name), "{name}");
        }
        let go = importable(
            Manifest::Go,
            "module ex.com/m\n\ngo 1.22\n\nrequire github.com/lib/pq v1.10.0\n\nrequire (\n\tgolang.org/x/sync v0.1.0 // indirect\n\tgithub.com/a/b v1.0.0\n)\n",
        );
        assert!(Manifest::Go.covers(&go, "github.com/lib/pq"));
        assert!(Manifest::Go.covers(&go, "golang.org/x/sync/errgroup"));
        assert!(Manifest::Go.covers(&go, "ex.com/m/internal/x"));
        assert!(!Manifest::Go.covers(&go, "github.com/a/bc"));
        let cargo = importable(
            Manifest::Cargo,
            "[package]\nname = \"my-app\"\n\n[dependencies]\nserde = \"1\"\nserde-json = { version = \"1\" }\njson5 = { package = \"json-five\", version = \"1\" }\n\n[dev-dependencies.insta]\nversion = \"1\"\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n",
        );
        for name in [
            "my_app",
            "serde",
            "serde_json",
            "insta",
            "libc",
            "json5",
            "json_five",
        ] {
            assert!(cargo.contains(name), "{name}");
        }
    }

    /// `[workspace.dependencies]` sets the versions members take with
    /// `workspace = true`; the package at the root may not import one it does not
    /// list itself, as `cargo build` says.
    #[test]
    fn a_workspace_dependency_is_importable_only_where_a_package_lists_it() {
        let root = importable(
            Manifest::Cargo,
            "[package]\nname = \"ws\"\n\n[workspace]\nmembers = [\"a\"]\n\n[workspace.dependencies]\nserde = \"1\"\n\n[dependencies]\nlog = \"0.4\"\n\n[features]\nextra = []\n",
        );
        assert_eq!(root, ["log", "ws"].map(str::to_string).into());
        let member = importable(
            Manifest::Cargo,
            "[package]\nname = \"a\"\n\n[dependencies]\nserde = { workspace = true }\n",
        );
        assert_eq!(member, ["a", "serde"].map(str::to_string).into());
    }

    #[test]
    fn a_marker_or_a_broken_manifest_declares_no_package() {
        assert_eq!(
            Manifest::Npm.declares(r#"{"type": "module"}"#),
            Declares::Marker
        );
        assert_eq!(Manifest::Npm.declares("{ not json"), Declares::Unreadable);
        assert_eq!(
            Manifest::Cargo.declares("[dependencies\n"),
            Declares::Unreadable
        );
        // A block that never closes: what follows it may be anything.
        assert_eq!(
            Manifest::Go.declares("module ex.com/m\n\nrequire (\n\tgithub.com/a/b v1.0.0\n"),
            Declares::Unreadable
        );
    }

    #[test]
    fn an_import_written_in_a_comment_or_a_string_names_no_package() {
        let ts = "// import a from \"ghost\";\nconst s = \"import 'phantom'\";\n\
                  /* require(\"spirit\") */\nimport b from \"real\";\nexport * from \"re\";\n\
                  import c = require(\"old\");\nconst d = await import(\"lazy\");\n";
        let named = Manifest::Npm.named_in_source("a.ts", ts);
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
        let named = Manifest::Npm.named_in_source("a.ts", ts);
        assert!(named.names("pkg"));
        assert!(!named.names("node"));
        assert!(!named.names("b"));
        // A path names a file of the project, not the package `.` or ``.
        let relative = "import { h } from \"./helper\";\nimport x from \"/abs/x\";\n";
        let named = Manifest::Npm.named_in_source("a.ts", relative);
        assert!(!named.names("."));
        assert!(!named.names(""));
        let rs = "use serde::de::Error;\nextern crate libc;\nfn f() { other::x(); }\n";
        let named = Manifest::Cargo.named_in_source("a.rs", rs);
        assert!(named.names("serde"));
        assert!(named.names("libc"));
        assert!(!named.names("serde_core"));
        assert!(
            !Manifest::Cargo
                .named_in_source("a.rs", "use my::serde::x;")
                .names("serde")
        );
    }

    fn found(text: &str) -> Vec<(usize, String, String, Source, Option<String>)> {
        declared(Manifest::Cargo, text)
            .unwrap_or_default()
            .into_iter()
            .map(|d| {
                (
                    d.line,
                    d.name,
                    d.package,
                    d.source,
                    d.part.map(|part| part.name),
                )
            })
            .collect()
    }

    #[test]
    fn a_cargo_manifest_declares_a_dependency_on_the_line_of_its_name() {
        let text = concat!(
            "[package]\n",                                            // 1
            "name = \"chunk-store\"\n",                               // 2
            "\n",                                                     // 3
            "[dependencies]\n",                                       // 4
            "storage-api = { workspace = true }\n",                   // 5
            "serde_json = \"1\"\n",                                   // 6
            "local = { path = \"../local\" }\n",                      // 7
            "json = { package = \"serde_json\", version = \"1\" }\n", // 8
            "\n",                                                     // 9
            "[dev-dependencies]\n",                                   // 10
            "testkit = { workspace = true }\n",                       // 11
            "\n",                                                     // 12
            "[build-dependencies]\n",                                 // 13
            "cc = \"1\"\n",                                           // 14
            "\n",                                                     // 15
            "[target.'cfg(unix)'.dependencies]\n",                    // 16
            "libc = \"0.2\"\n",                                       // 17
            "\n",                                                     // 18
            "[dependencies.regex]\n",                                 // 19
            "version = \"1\"\n",                                      // 20
        );
        let s = |x: &str| x.to_string();
        assert_eq!(
            found(text),
            [
                (
                    5,
                    s("storage-api"),
                    s("storage-api"),
                    Source::Workspace,
                    None
                ),
                (6, s("serde_json"), s("serde_json"), Source::Registry, None),
                (7, s("local"), s("local"), Source::Path(s("../local")), None),
                (8, s("json"), s("serde_json"), Source::Registry, None),
                (
                    11,
                    s("testkit"),
                    s("testkit"),
                    Source::Workspace,
                    Some(s("dev-dependencies"))
                ),
                (
                    14,
                    s("cc"),
                    s("cc"),
                    Source::Registry,
                    Some(s("build-dependencies"))
                ),
                (17, s("libc"), s("libc"), Source::Registry, None),
                (19, s("regex"), s("regex"), Source::Registry, None),
            ]
        );
        let kinds: Vec<_> = declared(Manifest::Cargo, text)
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.part.and_then(|part| part.kind))
            .collect();
        assert_eq!(kinds[4], Some(Kind::Test));
        assert_eq!(kinds[5], None);
    }

    #[test]
    fn a_manifest_that_is_no_toml_is_unread_not_empty() {
        assert_eq!(declared(Manifest::Cargo, "[dependencies\nx = "), None);
        assert_eq!(
            declared(Manifest::Cargo, "[package]\nname = \"a\"\n"),
            Some(Vec::new())
        );
    }

    #[test]
    fn the_same_name_in_two_tables_is_two_dependencies() {
        let text =
            "[dependencies]\nlibc = \"0.2\"\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n";
        let lines: Vec<_> = found(text).iter().map(|d| d.0).collect();
        assert_eq!(lines, [2, 5]);
    }

    #[test]
    fn the_lines_of_a_multi_line_table_are_no_dependencies() {
        let text = "[dependencies]\nfoo = {\n    version = \"1\",\n    path = \"../foo\",\n}\nversion = \"2\"\n";
        let names: Vec<_> = found(text).iter().map(|d| (d.0, d.1.clone())).collect();
        assert_eq!(names, [(2, "foo".to_string()), (6, "version".to_string())]);
    }

    fn dep(name: &str, source: Source) -> Declared {
        Declared {
            line: 1,
            name: name.to_string(),
            package: name.to_string(),
            source,
            part: None,
            target: None,
            feature: None,
            optional: false,
        }
    }

    fn project() -> (Vec<String>, impl Fn(&str) -> Option<String>) {
        let manifests: Vec<(&str, &str)> = vec![
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nstorage-api = { path = \"crates/storage-api\" }\nserde = \"1\"\n",
            ),
            (
                "crates/storage-api/Cargo.toml",
                "[package]\nname = \"storage-api\"\n",
            ),
            (
                "crates/chunk-store/Cargo.toml",
                "[package]\nname = \"chunk-store\"\n",
            ),
            ("crates/local/Cargo.toml", "[package]\nname = \"local\"\n"),
        ];
        let files = manifests.iter().map(|(p, _)| p.to_string()).collect();
        let text = move |path: &str| {
            manifests
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        (files, text)
    }

    #[test]
    fn a_path_dependency_is_the_manifest_of_a_package_of_the_project() {
        let (files, read) = project();
        let got = resolve(
            "crates/chunk-store/Cargo.toml",
            &dep("local", Source::Path("../local".into())),
            &files,
            &mut Workspaces::new(&read),
            &Names::default(),
        )
        .0;
        assert_eq!(got, Resolved::Internal("crates/local/Cargo.toml".into()));
        // A path with no manifest of the project there names nothing of it.
        let missing = resolve(
            "crates/chunk-store/Cargo.toml",
            &dep("gone", Source::Path("../gone".into())),
            &files,
            &mut Workspaces::new(&read),
            &Names::default(),
        )
        .0;
        assert_eq!(missing, Resolved::External("gone".into()));
    }

    #[test]
    fn a_workspace_dependency_is_what_the_workspace_manifest_declares() {
        let (files, read) = project();
        let from = "crates/chunk-store/Cargo.toml";
        assert_eq!(
            resolve(
                from,
                &dep("storage-api", Source::Workspace),
                &files,
                &mut Workspaces::new(&read),
                &Names::default()
            )
            .0,
            Resolved::Internal("crates/storage-api/Cargo.toml".into())
        );
        assert_eq!(
            resolve(
                from,
                &dep("serde", Source::Workspace),
                &files,
                &mut Workspaces::new(&read),
                &Names::default()
            )
            .0,
            Resolved::External("serde".into())
        );
    }

    /// Every crate of a large workspace takes most of its dependencies from it: its
    /// manifests are read once a run, however many dependencies ask.
    #[test]
    fn a_manifest_is_read_once_however_many_workspace_dependencies_ask() {
        let (files, read) = project();
        let reads = std::cell::RefCell::new(Vec::new());
        let counted = |path: &str| {
            reads.borrow_mut().push(path.to_string());
            read(path)
        };
        let mut workspaces = Workspaces::new(&counted);
        for (from, name) in [
            ("crates/chunk-store/Cargo.toml", "storage-api"),
            ("crates/chunk-store/Cargo.toml", "serde"),
            ("crates/local/Cargo.toml", "storage-api"),
            ("crates/local/Cargo.toml", "missing"),
        ] {
            resolve(
                from,
                &dep(name, Source::Workspace),
                &files,
                &mut workspaces,
                &Names::default(),
            );
        }
        let mut reads = reads.into_inner();
        reads.sort();
        assert_eq!(
            reads,
            [
                "Cargo.toml",
                "crates/Cargo.toml",
                "crates/chunk-store/Cargo.toml",
                "crates/local/Cargo.toml",
            ]
        );
    }

    #[test]
    fn a_renamed_workspace_dependency_is_spelled_as_its_package() {
        let manifests = [(
            "Cargo.toml",
            "[workspace]\n\n[workspace.dependencies]\nhttp = { package = \"real-http\", version = \"1\" }\n",
        )];
        let read = |path: &str| {
            manifests
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.to_string())
        };
        let files = vec!["Cargo.toml".to_string(), "crates/a/Cargo.toml".to_string()];
        let (got, problem) = resolve(
            "crates/a/Cargo.toml",
            &dep("http", Source::Workspace),
            &files,
            &mut Workspaces::new(&read),
            &Names::default(),
        );
        assert_eq!(got, Resolved::External("real_http".into()));
        assert_eq!(problem, None);
    }

    #[test]
    fn a_workspace_manifest_that_cannot_be_read_is_said() {
        let read = |path: &str| (path == "Cargo.toml").then(|| "[workspace\n".to_string());
        let files = vec!["Cargo.toml".to_string(), "crates/a/Cargo.toml".to_string()];
        let (got, problem) = resolve(
            "crates/a/Cargo.toml",
            &dep("api", Source::Workspace),
            &files,
            &mut Workspaces::new(&read),
            &Names::default(),
        );
        assert_eq!(got, Resolved::External("api".into()));
        let problem = problem.unwrap();
        assert!(problem.reason.contains("Cargo.toml"), "{problem:?}");
    }

    #[test]
    fn a_third_party_package_is_spelled_as_an_import_spells_it() {
        let (files, read) = project();
        let mut renamed = dep("json", Source::Registry);
        renamed.package = "serde-json".into();
        assert_eq!(
            resolve(
                "crates/local/Cargo.toml",
                &renamed,
                &files,
                &mut Workspaces::new(&read),
                &Names::default()
            )
            .0,
            Resolved::External("serde_json".into())
        );
    }

    /// (line, name, the part it leaves, the feature it reaches, where it comes from)
    type Row = (usize, String, Option<String>, Option<String>, Source);

    fn edges_of(text: &str) -> Vec<Row> {
        declared(Manifest::Cargo, text)
            .unwrap()
            .into_iter()
            .map(|d| (d.line, d.name, d.part.map(|p| p.name), d.feature, d.source))
            .collect()
    }

    #[test]
    fn features_declare_what_they_turn_on_and_an_optional_dependency_belongs_to_it() {
        let text = concat!(
            "[features]\n",                                                 // 1
            "default = [\"fake\"]\n",                                       // 2
            "fake = [\"storage-api/fake\", \"dep:serde\", \"other\"]\n",    // 3
            "other = [\"tls?/rustls\"]\n",                                  // 4
            "\n",                                                           // 5
            "[dependencies]\n",                                             // 6
            "storage-api = { workspace = true, features = [\"extra\"] }\n", // 7
            "serde = { version = \"1\", optional = true }\n",               // 8
            "tls = { version = \"1\", optional = true }\n",                 // 9
            "gzip = { version = \"1\", optional = true }\n",                // 10
        );
        let s = |x: &str| Some(x.to_string());
        let n = |x: &str| x.to_string();
        assert_eq!(
            edges_of(text),
            [
                // `default` is the package itself.
                (2, n("fake"), None, s("fake"), Source::Own),
                (
                    3,
                    n("storage-api"),
                    s("feature/fake"),
                    None,
                    Source::Workspace
                ),
                (
                    3,
                    n("storage-api"),
                    s("feature/fake"),
                    s("fake"),
                    Source::Workspace
                ),
                (3, n("serde"), s("feature/fake"), None, Source::Registry),
                (3, n("other"), s("feature/fake"), s("other"), Source::Own),
                // A weak item reaches the feature of `tls`, not `tls`.
                (
                    4,
                    n("tls"),
                    s("feature/other"),
                    s("rustls"),
                    Source::Registry
                ),
                (7, n("storage-api"), None, None, Source::Workspace),
                (7, n("storage-api"), None, s("extra"), Source::Workspace),
                // `serde` is named by `dep:serde`, so it has no feature of its own;
                // `tls` and `gzip` have the implicit one.
                (9, n("tls"), s("feature/tls"), None, Source::Registry),
                (10, n("gzip"), s("feature/gzip"), None, Source::Registry),
            ]
        );
    }

    /// (line, the part it leaves, what it reaches and the part of it) for each dependency.
    fn edge_list(files: &[(&str, &str)]) -> Vec<(usize, Option<String>, String, Option<String>)> {
        let dir = tempfile::TempDir::new().unwrap();
        let mut names = Vec::new();
        for (path, text) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
            names.push(path.to_string());
        }
        let found = declared_by(dir.path(), &names, |_| true);
        assert!(found.problems.is_empty(), "{:?}", found.problems);
        found
            .deps
            .into_iter()
            .filter(|dep| dep.file.to_str() == Some("crates/telegram/Cargo.toml"))
            .map(|dep| {
                let (key, part) = match &dep.target {
                    Target::Internal(unit) => (unit.key(), unit.part_name().map(str::to_string)),
                    Target::External(module) => (module.clone(), None),
                };
                (
                    dep.line,
                    dep.from.part_name().map(str::to_string),
                    key,
                    part,
                )
            })
            .collect()
    }

    #[test]
    fn a_feature_item_reaches_the_feature_of_another_manifest_of_the_project() {
        let s = |x: &str| Some(x.to_string());
        let n = |x: &str| x.to_string();
        let got = edge_list(&[
            (
                "Cargo.toml",
                "[workspace]\n\n[workspace.dependencies]\nstorage-api = { path = \"crates/storage-api\" }\n",
            ),
            (
                "crates/storage-api/Cargo.toml",
                "[package]\nname = \"storage-api\"\n",
            ),
            (
                "crates/telegram/Cargo.toml",
                concat!(
                    "[features]\n",                                      // 1
                    "default = [\"fake\"]\n",                            // 2
                    "fake = [\"storage-api/fake\", \"serde/derive\"]\n", // 3
                    "\n",                                                // 4
                    "[dependencies]\n",                                  // 5
                    "storage-api = { workspace = true }\n",              // 6
                    "serde = \"1\"\n",                                   // 7
                ),
            ),
        ]);
        let kit = n("crates/storage-api/Cargo.toml");
        let own = n("crates/telegram/Cargo.toml");
        assert_eq!(
            got,
            [
                // `default` is the package itself, and `fake` a feature of the manifest.
                (2, None, own, s("feature/fake")),
                (3, s("feature/fake"), kit.clone(), None),
                (3, s("feature/fake"), kit.clone(), s("feature/fake")),
                // An external package has no part of the project to reach.
                (3, s("feature/fake"), n("serde"), None),
                (6, None, kit, None),
                (7, None, n("serde"), None),
            ]
        );
    }

    #[test]
    fn default_as_a_target_is_the_package_itself() {
        let own = "crates/telegram/Cargo.toml".to_string();
        let got = edge_list(&[(
            "crates/telegram/Cargo.toml",
            "[features]\ndefault = []\nfull = [\"default\"]\n",
        )]);
        assert_eq!(got, [(3, Some("feature/full".to_string()), own, None)]);
    }

    #[test]
    fn a_literal_string_is_an_item_too() {
        let rows = edges_of(
            "[features]\nfake = ['dep:serde']\n\n[dependencies]\nserde = { version = \"1\", optional = true }\n",
        );
        assert_eq!(
            rows,
            [(
                2,
                "serde".to_string(),
                Some("feature/fake".to_string()),
                None,
                Source::Registry
            )]
        );
    }

    #[test]
    fn an_item_takes_what_the_dependencies_declare_not_the_dev_ones() {
        let text = concat!(
            "[dev-dependencies]\n",
            "x = { path = \"../x\" }\n",
            "\n",
            "[dependencies]\n",
            "x = \"1\"\n",
            "\n",
            "[features]\n",
            "f = [\"x/g\"]\n",
        );
        let items: Vec<_> = edges_of(text).into_iter().filter(|r| r.0 == 8).collect();
        let f = Some("feature/f".to_string());
        assert_eq!(
            items,
            [
                (8, "x".to_string(), f.clone(), None, Source::Registry),
                (
                    8,
                    "x".to_string(),
                    f,
                    Some("g".to_string()),
                    Source::Registry
                ),
            ]
        );
    }

    #[test]
    fn an_item_that_names_no_dependency_declares_none() {
        let rows = edges_of("[features]\nf = [\"typo/g\", \"dep:typo\"]\n");
        assert!(rows.is_empty(), "{rows:?}");
    }

    /// Resolves each dependency a manifest of a project declares.
    fn resolved(
        kind: Manifest,
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
        let names = Names::of(&files, &read);
        let mut workspaces = Workspaces::new(&read);
        let text = read(manifest).unwrap();
        declared(kind, &text)
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    resolve(manifest, d, &files, &mut workspaces, &names).0,
                )
            })
            .collect()
    }

    #[test]
    fn an_npm_dependency_is_the_package_json_that_goes_by_its_name_or_its_path() {
        let got = resolved(
            Manifest::Npm,
            "packages/web/package.json",
            &[
                ("packages/api/package.json", "{\"name\": \"@acme/api\"}"),
                ("packages/local/package.json", "{\"name\": \"local\"}"),
                (
                    "packages/web/package.json",
                    "{\"name\": \"web\", \"dependencies\": {\"@acme/api\": \"workspace:*\", \"lodash\": \"^4\", \"loc\": \"file:../local\"}}",
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
            ]
        );
    }

    #[test]
    fn a_go_module_is_the_go_mod_that_declares_its_path_or_replaces_it() {
        let got = resolved(
            Manifest::Go,
            "app/go.mod",
            &[
                (
                    "app/go.mod",
                    "module example.com/app\n\nrequire (\n\texample.com/lib v1.0.0\n\tgithub.com/lib/pq v1.10.0\n\texample.com/moved v1.0.0\n)\n\nreplace example.com/moved => ../moved\n",
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
        let names = Names::of(&files, &read);
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
            Manifest::Npm,
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
        let names = Names::of(&files, &read);
        assert!(names.duplicates.is_empty(), "{:?}", names.duplicates);
    }
}
