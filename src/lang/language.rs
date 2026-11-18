//! What a language is to easement: the type of its entry in the registry, whose
//! table is `registry.rs`, and the types its data and hooks speak.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::parts::{Doubt, Found};
use super::{Adapter, Cause, Import};
use crate::packages::manifest::Manifest;
use crate::relpath;
use crate::scip::symbol::{Module, Name, Parsed, Qualifier};

/// Finds the files a file reaches that its index may not report as uses, from
/// its text, its path and the project's files.
pub type UnindexedUses = fn(&str, &str, &BTreeSet<&str>) -> Vec<UnindexedUse>;

/// A use of a file that an index may not report, where it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnindexedUse {
    /// The 1-based line of its specifier.
    pub line: usize,
    /// The specifier as written, without quotes.
    pub specifier: String,
    /// The file it reaches.
    pub to: String,
}

/// What easement reads of the imports a file writes, to compare them with what its
/// index resolved.
#[derive(Debug)]
pub struct WrittenImports {
    /// The imports a file writes, from its path and text, by line and module as
    /// spelled and resolved by nothing, each with its form.
    pub list: fn(&str, &str) -> Vec<Import>,
    /// What the report says of each cause of an unresolved import in this
    /// language: what usually brings it about, and how to close it.
    pub explained: &'static [Explained],
    /// What a specifier spells when the index takes it to no source target, read
    /// from its text alone. `None` when nothing is read from it, and such an import
    /// is an evidence gap.
    pub spelled: Option<fn(&str) -> Spelled>,
}

/// What brings about one cause of an unresolved import in a language, and how to
/// close it, for the report.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Explained {
    pub cause: Cause,
    /// What usually brings it about, said after the cause itself.
    pub because: &'static str,
    /// How to close it; `None` when it closes as every unresolved import does.
    pub close: Option<&'static str>,
}

/// What the text of a specifier spells, without resolving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spelled {
    /// The paths, relative to the file that writes it, of the files it may name,
    /// each as the language reads it: a query (`?raw`) left off, no extension or
    /// `index` guessed; Python's `.x` is `x.py` or the package `x/__init__.py`.
    /// It names the one of them that exists, and none when not exactly one does.
    Paths(Vec<String>),
    /// A package, by its name (`some-pkg` of `some-pkg/style.css`).
    Package(String),
    /// Neither: an alias of the build (`#lib/x`, `$lib/x`), a scheme (`virtual:x`).
    Neither,
}

/// Builds the adapter of a language whose source is read by a tree-sitter
/// adapter, not by an indexer.
pub type AdapterBuilder = fn(&Path, &[PathBuf]) -> anyhow::Result<Box<dyn Adapter>>;

/// What reads the files of a language: the SCIP indexer that writes their index,
/// or the tree-sitter adapter that reads their imports. A language has exactly one.
#[derive(Debug)]
pub enum Reader {
    Indexer(&'static Indexer),
    Adapter(AdapterBuilder),
}

/// The SCIP indexer of a language: how to run it and what its index depends on.
/// `indexer.rs` runs it; the code of its hooks sits in `lang/<language>.rs`.
#[derive(Debug)]
pub struct Indexer {
    pub binary: &'static str,
    /// Files that mark a directory the indexer runs in.
    pub markers: &'static [&'static str],
    /// Run only in the outermost marked directories: a Cargo workspace holds its members.
    pub outermost: bool,
    /// Run at the project root when no marker is found (Python projects often have none).
    pub fallback_root: bool,
    /// Arguments that write the index to the given file.
    pub args: fn(&Path) -> Vec<String>,
    pub install: &'static str,
    /// The mise tool that installs the indexer, for the `[tools]` of a `mise.toml`.
    pub mise: Option<&'static str>,
    /// Why it cannot run on Windows, with the upstream cause; `None` when it can.
    /// There easement does not start it: its files are uncovered with this reason.
    /// Removed by hand once the pinned indexer runs there (the Windows CI run shows it).
    pub not_on_windows: Option<&'static str>,
    /// For an indexer that has no output option: the file it writes in its working directory.
    pub writes_in_cwd: Option<&'static str>,
    /// The argument that makes it print its version, for `doctor`; `None` for an
    /// indexer that takes any argument for a request to index (scip-php).
    pub version_flag: Option<&'static str>,
    /// What has to be in place before the indexer runs: (project root, directory, shown name).
    pub needs: fn(&Path, &Path, &str) -> anyhow::Result<()>,
    /// Which project files under the index directory the cache key reads (the `/`-separated
    /// path): what the indexer could see. Another language's sources, docs and assets are left
    /// out, so a change to them reuses the index. When in doubt, a file is in.
    pub inputs: fn(&Language, &str) -> bool,
    /// Which inherited environment variables change what the indexer writes
    /// (`GOFLAGS`, `PYTHONPATH`); their values are in the cache key.
    pub env: fn(&str) -> bool,
    /// What to tell whoever runs it when a failed run says something known.
    pub hints: &'static [Hint],
    /// The scheme its index writes its symbols in (`scip-go`, `rust-analyzer`).
    pub scheme: &'static str,
    /// How its symbols are read into the shape every language shares.
    pub symbols: Symbols,
}

/// How the symbols of an index are read into the shape every language shares
/// (`scip::symbol::Name`).
#[derive(Debug)]
pub enum Symbols {
    /// Modules are namespaces, and a name at their top level is the first
    /// descriptor past them (Go, Python, Rust, TypeScript). `qualifier` settles what
    /// the language reads differently and gives the qualifier its names are called
    /// by; `external` is the external module a symbol belongs to, in the spelling
    /// of its imports and of `forbid_external`.
    Modules {
        qualifier: fn(&mut Module<'_>) -> Qualifier,
        external: fn(&Parsed<'_>) -> String,
    },
    /// A namespace is no module; a class is what an import names, and a type nested
    /// in it is reached through it (Java, C#, PHP). `separator` joins a namespace;
    /// `constructor` is the member that stands for its class, as the indexer records
    /// the class the code writes only as that member; `owned` is whether the package
    /// of a symbol tells the project's own types from those of its dependencies.
    Classes {
        separator: &'static str,
        constructor: Option<&'static str>,
        owned: bool,
    },
    /// Read whole by a hook of the language (Ruby).
    Own(fn(&str, &Parsed<'_>) -> Name),
}

/// A known cause of a failed run of an indexer, told by what the run printed.
#[derive(Debug)]
pub struct Hint {
    /// Text the output of the failed run contains.
    pub output: &'static str,
    /// What it means and how to fix it, said after the output.
    pub hint: &'static str,
}

/// Nothing has to be in place before the indexer runs.
pub fn nothing_needed(_: &Path, _: &Path, _: &str) -> anyhow::Result<()> {
    Ok(())
}

/// Every file the project owns: for an indexer whose build reads files of its own choosing
/// (Maven, Gradle, Cargo, MSBuild, Bundler, Composer, Python's environment). The directories
/// its toolchain fills with code not the project's own are left out: build output (`target`)
/// is made from the files keyed, and an install (`vendor`) from a lock file.
pub fn every_own_file(language: &Language, path: &str) -> bool {
    !language.is_foreign(path)
}

/// Whether `name` starts with one of `prefixes`: how the `env` of an indexer
/// names a family of variables (`MAVEN_`, `DOTNET_`).
pub fn starts_with_any(name: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| name.starts_with(prefix))
}

/// A setting of an indexer, as the policy writes it: `tags: [race]`.
/// The value of a variable in the environment an indexer runs in.
pub type Inherited<'a> = &'a dyn Fn(&str) -> Option<String>;

#[derive(Debug)]
pub struct Setting {
    pub key: &'static str,
    /// What a value has to be, said to whoever wrote another.
    pub value: &'static str,
    pub valid: fn(&str) -> bool,
    /// The variable the indexer reads the values from, and its value over
    /// what the indexer would inherit.
    pub env: fn(&[String], Inherited) -> (&'static str, String),
}

/// What the index of a language gives of the types a type implements or extends.
#[derive(Debug)]
pub enum Relations {
    /// The relations its code declares (`extends`, `implements`, a base class),
    /// read from the index. `unrelated` finds the declarations the index gives
    /// none for, which are an evidence gap. `anonymous` says why the index gives a
    /// class no type, when it gives only its methods relations to the methods they
    /// implement (a Java anonymous class): such a class is an evidence gap too.
    Declared {
        unrelated: Option<Unrelated>,
        anonymous: Option<&'static str>,
    },
    /// Relations by the shape of a type alone, which no code declares (a Go type
    /// that has the methods of an interface): no implementation is read from them.
    /// `because` says why, as a clause.
    Structural { because: &'static str },
    /// No relations at all; `because` says why, as a clause.
    None { because: &'static str },
}

impl Relations {
    /// Whether implementations are read from the index of the language.
    pub fn declared(&self) -> bool {
        matches!(self, Relations::Declared { .. })
    }
}

/// Declarations of a language whose index gives no relations although the code
/// may declare some, and why.
#[derive(Debug)]
pub struct Unrelated {
    /// The spans of such declarations in a file, from its path and text.
    pub find: fn(&str, &str) -> Vec<UnrelatedSpan>,
    /// Why the index gives them none, as a clause.
    pub because: &'static str,
}

/// The lines of a declaration whose index gives no relations, 1-based and
/// inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrelatedSpan {
    pub lines: std::ops::RangeInclusive<usize>,
    /// Whether it counts only when it writes a type alias: a clause whose other
    /// names the index relates, but not the names an alias stands for.
    pub through_alias: bool,
}

/// How a language writes a comment, in which a suppression is written.
#[derive(Debug)]
pub struct Comments {
    /// What opens a comment that runs to the end of its line (`//`, `#`).
    pub line: &'static [&'static str],
    /// What opens and what closes a block comment (`/*` and `*/`).
    pub block: &'static [(&'static str, &'static str)],
}

/// Which modules are the standard library of a language, for `$std`.
#[derive(Debug)]
pub enum Std {
    /// Those whose first segment, cut at the language's `module_separators`, is
    /// one of these: `os` of `os.path`, `std` of `std::fmt`.
    Roots(&'static [&'static str]),
    /// Those the function takes, by the whole module: Go's paths without a
    /// domain, the JDK's packages.
    Rule(fn(&str) -> bool),
}

/// The parts of a language's source files that a dependency belongs to instead of
/// the file, and what easement says when it cannot tell them.
#[derive(Debug)]
pub struct Parts {
    /// Finds them in a source, from its path and text.
    pub find: fn(&str, &str) -> Found,
    /// Their names, as `part:` names them.
    pub names: &'static [&'static str],
    /// The one that only a test build has: a file only test code declares is
    /// in it as a whole.
    pub test: &'static str,
    /// Why the test code of a file easement cannot read is judged as the file's.
    pub unreadable: Doubt,
    /// Why a file that test code declares, which the project does not have, is
    /// not judged as test code.
    pub missing_file: Doubt,
}

#[derive(Debug)]
pub struct Language {
    pub name: &'static str,
    /// Extensions of its source files.
    pub extensions: &'static [&'static str],
    /// How it writes a comment.
    pub comments: Comments,
    /// Suffixes of its declaration files (`d.ts`), which declare the types of code
    /// written elsewhere: one is the module its name says without the suffix, no
    /// import has to reach it, and an index need not hold it to cover the project.
    pub declarations: &'static [&'static str],
    /// Globs of its test files, which `tests: auto` puts into the `tests` component.
    pub tests: &'static [&'static str],
    /// Globs of files that `tests` matches but that are not test files.
    pub not_tests: &'static [&'static str],
    /// Whether its compiler builds the files of a directory together, as Go does a
    /// package: reaching one of them reaches the others, but not its test files.
    pub whole_directory: bool,
    /// Which modules are its standard library, for `$std`; `None` when easement
    /// cannot tell.
    pub std: Option<Std>,
    /// Extensions of the files `unused` judges: those whose import graph shows
    /// every use of another file. Empty when it judges none.
    pub unused: &'static [&'static str],
    /// Files its toolchain or a convention runs without an import, which
    /// `unused` counts as reached.
    pub conventional: &'static [&'static str],
    /// The manifest the `manifests` rule checks its imports against.
    pub manifest: Option<&'static Manifest>,
    /// Whether easement reads verified uses of names from its files, which
    /// `forbid_symbol` and `public_symbols` judge. A symbol rule over a language
    /// without them is an evidence gap.
    pub symbols: bool,
    /// Whether its index gives each method and function the lines it spans, which
    /// `impact --diff` maps the changed lines onto. Without them a changed file is
    /// one whole, and an evidence gap says so.
    pub definition_ranges: bool,
    /// What its index gives of the types a type implements or extends, which the
    /// `implementation` rule judges. A language without declared relations is an
    /// evidence gap for that rule.
    pub relations: Relations,
    /// The parts of a source file that a dependency belongs to instead of the
    /// file, for example the code of a test build. `None` when the file is one unit.
    pub parts: Option<Parts>,
    /// A file the language runs when any module below its directory is imported
    /// (Python's `__init__.py`): `unused` counts it as reached from that module.
    pub runs_on_import: Option<&'static str>,
    /// Languages whose files its files use without an import, with why: beside
    /// its files, `unused` cannot judge theirs.
    pub uses_unimported: &'static [(&'static Language, &'static str)],
    /// Whether the text of a file makes it a program of its own (Go's
    /// `package main`): `unused` counts it as an entry.
    pub program: Option<fn(&str) -> bool>,
    /// The files a file reaches that its index may not report as uses, from its
    /// text, its path and the project's files: `unused` follows them too.
    pub unindexed_uses: Option<UnindexedUses>,
    /// The imports the text of a file writes, compared with its index: the index
    /// must take each to a source target, a file of the project or an external
    /// package, or its dependency is read from its text or is an evidence gap.
    /// `None` when easement cannot list them, and nothing is cross-checked.
    pub written_imports: Option<WrittenImports>,
    /// What reads its files: its indexer, or the adapter that reads them when no
    /// indexer does.
    pub reader: Reader,
    /// Directories its toolchain fills with code that is not the project's own,
    /// wherever they lie under its files: third-party installs (`vendor`),
    /// fixtures the toolchain ignores (`testdata`), build output (`target`). Its
    /// indexer neither has to cover their files nor runs on a marker in them,
    /// and a manifest of its kind in them is no package of the project. They
    /// are still walked: another language's code may sit in a directory of that
    /// name (Rails keeps its scripts in `bin/`).
    pub foreign_dirs: &'static [&'static str],
    /// Directories its toolchain fills with third-party code (`node_modules`)
    /// that is no language's project code, which easement never walks. Unlike
    /// `foreign_dirs`, the walk skips them for every language: a stray `.py`
    /// in `node_modules` is not the project's either. A symbol whose path runs
    /// into one is of the package installed there, named as its manifest's
    /// `package` names it, whatever package the index credits it to.
    pub third_party: &'static [&'static str],
    /// What its indexer may be run with, under `indexers.<name>` of the policy.
    pub settings: &'static [Setting],
    /// Where its module names are cut into parents (`/` of `lodash/fp`, `::` of
    /// `serde::de`): a pattern of a parent covers what is below it.
    pub module_separators: &'static [&'static str],
    /// What joins a type and a member of it in a name as `forbid_symbol` writes it
    /// (`.` of `db.Client.Query`, `::` of `db::client::Client.query`), as its index
    /// is read.
    pub member_separator: &'static str,
}

impl Language {
    /// The first segment of one of its modules, cut at its `module_separators`:
    /// `os` of `os.path`, `std` of `std::fmt`, the module itself when uncut.
    pub fn first_segment<'a>(&self, module: &'a str) -> &'a str {
        let cut = self
            .module_separators
            .iter()
            .filter_map(|separator| module.find(separator))
            .min()
            .unwrap_or(module.len());
        &module[..cut]
    }

    /// Whether `module` belongs to its standard library, as its entry says.
    pub fn is_std(&self, module: &str) -> bool {
        match &self.std {
            Some(Std::Roots(roots)) => roots.contains(&self.first_segment(module)),
            Some(Std::Rule(rule)) => rule(module),
            None => false,
        }
    }

    /// Its indexer, when an indexer reads its files.
    pub fn indexer(&self) -> Option<&Indexer> {
        match &self.reader {
            Reader::Indexer(indexer) => Some(indexer),
            Reader::Adapter(_) => None,
        }
    }

    /// Its adapter, when a tree-sitter adapter reads its files instead of an indexer.
    pub fn adapter(&self) -> Option<AdapterBuilder> {
        match self.reader {
            Reader::Adapter(build) => Some(build),
            Reader::Indexer(_) => None,
        }
    }

    /// What its module paths drop from the end of a file name: each declaration
    /// suffix, then each extension, all with their dot (`.d.ts` before `.ts`).
    pub(crate) fn module_suffixes(&self) -> impl Iterator<Item = String> + '_ {
        module_suffixes(self.declarations, self.extensions)
    }

    /// The module path of one of its files: `file` without its declaration suffix
    /// or else its extension (`src/a.d.ts` and `src/a.ts` are `src/a`).
    pub fn module_path<'a>(&self, file: &'a str) -> &'a str {
        module_path_in(file, self.declarations, self.extensions)
    }

    /// Whether `file` lies in one of its `foreign_dirs`, at any depth: its code
    /// is not the project's own.
    pub fn is_foreign(&self, file: &str) -> bool {
        relpath::parent(file)
            .split('/')
            .any(|dir| self.foreign_dirs.contains(&dir))
    }

    /// Whether `file` is one of its declaration files (`a.d.ts`).
    pub fn declares(&self, file: &str) -> bool {
        self.declarations.iter().any(|suffix| {
            file.strip_suffix(suffix)
                .and_then(|stem| stem.strip_suffix('.'))
                .is_some_and(names_a_file)
        })
    }
}

/// Whether `stem`, a path cut before a dot, still ends in a name: not `""` of
/// `.env`, nor `src/` of `src/.env`.
pub(crate) fn names_a_file(stem: &str) -> bool {
    !stem.is_empty() && !stem.ends_with('/')
}

/// The suffixes a module path drops: each declaration suffix, then each extension,
/// with its dot.
fn module_suffixes<'a>(
    declarations: &'a [&'a str],
    extensions: &'a [&'a str],
) -> impl Iterator<Item = String> + 'a {
    declarations
        .iter()
        .chain(extensions)
        .map(|suffix| format!(".{suffix}"))
}

/// The module path of `file`, written with these declaration suffixes and
/// extensions: `file` without its declaration suffix or else its extension
/// (`src/a.d.ts` and `src/a.ts` are `src/a`).
pub fn module_path_in<'a>(file: &'a str, declarations: &[&str], extensions: &[&str]) -> &'a str {
    module_suffixes(declarations, extensions)
        .find_map(|suffix| file.strip_suffix(&suffix).filter(|stem| names_a_file(stem)))
        .unwrap_or(file)
}

/// Whether `path` has one of `extensions`.
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension))
}
