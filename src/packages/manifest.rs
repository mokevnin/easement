//! What a kind of manifest is to easement: the type of its entry in the registry,
//! whose table is `lang/registry.rs`, and the types its data and hooks speak.

use std::collections::BTreeSet;

use crate::check::Unjudged;
use crate::lang::parts::Part;
use crate::relpath;

/// A kind of manifest: the file that declares the external modules of a language's
/// files, which the `manifests` rule reads, and the dependencies of a package of the
/// project, which `packages` judges. Data, and hooks for what a rule needs done
/// differently, whose code sits in `packages/<kind>.rs`; a hook left `None` does
/// what every kind does unless it says otherwise.
pub struct Manifest {
    /// The name of its file (`package.json`), which tells a manifest of this kind.
    pub file_name: &'static str,
    /// The names of the parts its dependencies can belong to, as `part:` names them.
    pub part_names: &'static [&'static str],
    /// The dependencies a manifest declares, each with its line, the project's own
    /// resolved to the directory of their package; `None` for a manifest that cannot
    /// be read, which is not one that declares nothing.
    pub declared: fn(&ManifestFile) -> Option<Listed>,
    /// What the files under a manifest with this text may import.
    pub declares: fn(&str) -> Declares,
    /// The package an external module belongs to (`lodash` of `lodash/fp`); `None`
    /// when the module is its own package, as a Go module path is, which
    /// [`Manifest::covers`] matches by prefix.
    pub package: Option<fn(&str) -> String>,
    /// Whether a manifest that declares these names covers a package; `None` when
    /// it covers the ones it names.
    pub covers: Option<fn(&BTreeSet<String>, &str) -> bool>,
    /// The packages a source file, by its path and text, names in an import; `None`
    /// when every package the index names is one the file imports (a Go import is
    /// the package path itself).
    pub named_in_source: Option<fn(&str, &str) -> BTreeSet<String>>,
    /// The name a manifest gives its package, which a dependency of another manifest
    /// of the project can name (`name` of `package.json`, `module` of `go.mod`);
    /// `None` when a dependency names a path instead.
    pub namer: Option<fn(&str) -> Option<String>>,
    /// How an import spells an external package (Rust writes a crate with `_`);
    /// `None` when it spells it as the manifest does.
    pub spelled_in_import: Option<fn(&str) -> String>,
    /// The files of the project a manifest in a directory names as what its package
    /// runs or exports, which `unused` counts as entries, from its text, its
    /// directory and the project's files; `None` when it names none. The hook gives
    /// `None` for one that cannot be parsed, whose package may export any file.
    pub entries: Option<EntriesOf>,
    /// The frameworks a manifest with this text depends on, which load files of its
    /// package by convention; `None` when no framework is told by it.
    pub frameworks: Option<fn(&str) -> Vec<&'static Framework>>,
}

/// Finds the files of the project a manifest names as what its package runs or
/// exports, from its text, its directory and the project's files; `None` for one
/// that cannot be parsed.
pub type EntriesOf = fn(&str, &str, &BTreeSet<&str>) -> Option<Vec<String>>;

/// A manifest a kind reads: its path, its text, and what the other files of the
/// project hold, which a manifest may take dependencies from (a Cargo workspace).
pub struct ManifestFile<'a> {
    pub path: &'a str,
    pub text: &'a str,
    /// The text of a file of the project, by its path; `None` when it cannot be read.
    pub read: &'a dyn Fn(&str) -> Option<String>,
}

/// A kind of manifest shows as the name of its file.
impl std::fmt::Debug for Manifest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Manifest").field(&self.file_name).finish()
    }
}

/// One kind of manifest is told from another by the name of its file.
impl PartialEq for Manifest {
    fn eq(&self, other: &Self) -> bool {
        self.file_name == other.file_name
    }
}

impl Eq for Manifest {}

impl PartialOrd for Manifest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Manifest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.file_name.cmp(other.file_name)
    }
}

impl Manifest {
    /// The dependencies a manifest declares, see [`Manifest::declared`].
    pub fn declared(&self, file: &ManifestFile) -> Option<Listed> {
        (self.declared)(file)
    }

    /// What the files under a manifest with `text` may import.
    pub fn declares(&self, text: &str) -> Declares {
        (self.declares)(text)
    }

    /// The package an external module belongs to: `lodash` for `lodash/fp`, the
    /// crate of a Rust path; a module of a kind without packages of its own whole.
    pub fn package(&self, module: &str) -> String {
        self.package
            .map_or_else(|| module.to_string(), |package| package(module))
    }

    /// Whether a manifest that declares `names` covers `package`.
    pub fn covers(&self, names: &BTreeSet<String>, package: &str) -> bool {
        self.covers
            .map_or_else(|| names.contains(package), |covers| covers(names, package))
    }

    /// The packages `source`, the text of `file`, names in an import. The index names
    /// the package that defines a symbol, which differs from the one the code imports
    /// for a re-export (`serde` re-exports `serde_core`), a global type
    /// (`NodeJS.Timeout`) or a Node builtin; only the imported one has to be declared.
    pub fn named_in_source(&self, file: &str, source: &str) -> Named {
        self.named_in_source
            .map_or(Named::Any, |named| Named::These(named(file, source)))
    }

    /// The name a manifest with `text` gives its package, when its kind names one.
    pub fn name_in(&self, text: &str) -> Option<String> {
        self.namer.and_then(|namer| namer(text))
    }

    /// How an import spells the external `package`.
    pub fn spelled_in_import(&self, package: String) -> String {
        match self.spelled_in_import {
            Some(spell) => spell(&package),
            None => package,
        }
    }

    /// The files of the project a manifest in `dir` names as what its package runs
    /// or exports; `None` for one that cannot be parsed.
    pub fn entries(&self, text: &str, dir: &str, files: &BTreeSet<&str>) -> Option<Vec<String>> {
        match self.entries {
            Some(entries) => entries(text, dir, files),
            None => Some(Vec::new()),
        }
    }

    /// The frameworks a manifest with `text` depends on.
    pub fn frameworks(&self, text: &str) -> Vec<&'static Framework> {
        self.frameworks
            .map_or_else(Vec::new, |frameworks| frameworks(text))
    }
}

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

/// The line, from 1, of the byte `offset` of `text`: where a parser's span starts.
pub(crate) fn line_at(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// The manifest of `kind` of the package in `dir`.
pub(crate) fn manifest_in(kind: &Manifest, dir: &str) -> String {
    relpath::join(dir, kind.file_name)
}

/// A framework that loads files of a package by convention.
pub struct Framework {
    pub name: &'static str,
    /// The dependency, in `dependencies` or `devDependencies`, that tells it.
    pub dependency: &'static str,
    /// The files it loads, as globs relative to the directory of the package.
    pub entries: &'static [&'static str],
    /// Its config files, as globs relative to the directory of the package.
    pub configs: &'static [&'static str],
    /// The settings of a config file that move its conventions.
    pub moves: &'static [Setting],
}

/// A setting of a framework's config file: its name as the framework's docs
/// spell it, and the key that writes it, found in the text as a whole word. A
/// key found where it means something else only leaves files unjudged.
pub struct Setting {
    pub name: &'static str,
    pub key: &'static str,
}
