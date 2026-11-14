//! What easement knows of each language, told by the extensions of its files:
//! data, and hooks for what a rule needs done differently, whose code sits in
//! `lang/<language>.rs`. Adding a language is one entry here, plus an indexer in
//! `indexer::INDEXERS` or an adapter in this module. The rules ask this table,
//! never an extension or a language by name; `easement.yml` holds them to it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use super::parts::{self, Found};
use super::{Adapter, Cause, Import};
use crate::relpath;
use crate::stdlib;

/// The file that declares the external modules of a language's files, which
/// the `manifests` rule reads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Manifest {
    Npm,
    Go,
    Cargo,
}

/// Every kind of manifest.
pub const MANIFESTS: [Manifest; 3] = [Manifest::Npm, Manifest::Go, Manifest::Cargo];

impl Manifest {
    pub fn file_name(self) -> &'static str {
        match self {
            Manifest::Npm => "package.json",
            Manifest::Go => "go.mod",
            Manifest::Cargo => "Cargo.toml",
        }
    }

    /// Whether `file` lies in a directory that the toolchain of a language
    /// checked against this manifest fills with code not the project's own
    /// (Go's `vendor/`): a manifest there is no package of the project.
    pub fn is_foreign(self, file: &str) -> bool {
        LANGUAGES
            .iter()
            .filter(|language| language.manifest == Some(self))
            .any(|language| language.is_foreign(file))
    }

    /// The parts its dependencies can belong to, as `part:` names them.
    pub fn parts(self) -> &'static [&'static str] {
        match self {
            Manifest::Npm => &["devDependencies"],
            Manifest::Go => &[],
            Manifest::Cargo => &["feature/<name>", "dev-dependencies", "build-dependencies"],
        }
    }
}

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
    /// A path relative to the file that writes it, as it names a file: a query
    /// (`?raw`) left off, no extension or `index` added.
    Path(String),
    /// A package, by its name (`some-pkg` of `some-pkg/style.css`).
    Package(String),
    /// Neither: an alias of the build (`#lib/x`, `$lib/x`), a scheme (`virtual:x`).
    Neither,
}

/// Builds the adapter of a language whose source is read by a tree-sitter
/// adapter, not by an indexer.
pub type AdapterBuilder = fn(&Path, &[PathBuf]) -> anyhow::Result<Box<dyn Adapter>>;

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

#[derive(Debug)]
pub struct Language {
    pub name: &'static str,
    /// Extensions of its source files.
    pub extensions: &'static [&'static str],
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
    /// Whether a module belongs to its standard library, for `$std`; `None`
    /// when easement cannot tell.
    pub std: Option<fn(&str) -> bool>,
    /// Extensions of the files `unused` judges: those whose import graph shows
    /// every use of another file. Empty when it judges none.
    pub unused: &'static [&'static str],
    /// Files its toolchain or a convention runs without an import, which
    /// `unused` counts as reached.
    pub conventional: &'static [&'static str],
    /// The manifest the `manifests` rule checks its imports against.
    pub manifest: Option<Manifest>,
    /// Whether easement reads verified uses of names from its files, which
    /// `forbid_symbol` and `public_symbols` judge. A symbol rule over a language
    /// without them is an evidence gap.
    pub symbols: bool,
    /// Finds the parts of a source file that a dependency belongs to instead of the
    /// file, for example the code of a test build. `None` when the file is one unit.
    pub parts: Option<fn(&str, &str) -> Found>,
    /// The names of the parts `parts` finds, as `part:` names them.
    pub part_names: &'static [&'static str],
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
    /// The adapter that reads its files, when no indexer reads them: `None`
    /// for a language an indexer covers.
    pub adapter: Option<AdapterBuilder>,
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
    /// in `node_modules` is not the project's either.
    pub third_party: &'static [&'static str],
    /// What its indexer may be run with, under `indexers.<name>` of the policy.
    pub settings: &'static [Setting],
    /// Where its module names are cut into parents (`/` of `lodash/fp`, `::` of
    /// `serde::de`): a pattern of a parent covers what is below it.
    pub module_separators: &'static [&'static str],
}

const TYPESCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

pub static GO: Language = Language {
    name: "go",
    extensions: &["go"],
    declarations: &[],
    tests: &["**/*_test.go"],
    not_tests: &[],
    whole_directory: true,
    std: Some(stdlib::go),
    unused: &["go"],
    conventional: &[],
    manifest: Some(Manifest::Go),
    symbols: true,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: Some(super::go::program),
    unindexed_uses: None,
    written_imports: Some(WrittenImports {
        list: super::go::written_imports,
        explained: &[Explained {
            cause: Cause::NoTarget,
            because: "for example, a package that does not type-check, or an import cycle",
            close: None,
        }],
        spelled: None,
    }),
    adapter: None,
    foreign_dirs: &["vendor", "testdata"],
    third_party: &[],
    part_names: &[],
    module_separators: &["/"],
    settings: &[Setting {
        key: "tags",
        value: "a build tag (letters, digits, `_` and `.`)",
        valid: super::go::build_tag,
        env: super::go::tags_env,
    }],
};

/// TypeScript and JavaScript, which one indexer reads and the bundlers resolve alike.
pub static TYPESCRIPT: Language = Language {
    name: "typescript",
    extensions: TYPESCRIPT_EXTENSIONS,
    declarations: &["d.ts", "d.mts", "d.cts"],
    tests: &["**/*.test.*", "**/*.spec.*", "**/__tests__/**"],
    not_tests: &[],
    whole_directory: false,
    std: None,
    unused: TYPESCRIPT_EXTENSIONS,
    conventional: &[],
    manifest: Some(Manifest::Npm),
    symbols: true,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: Some(super::typescript::unindexed_uses),
    written_imports: Some(WrittenImports {
        list: super::typescript::written_imports,
        explained: super::typescript::EXPLAINED,
        spelled: Some(super::typescript::spelled),
    }),
    adapter: None,
    foreign_dirs: &[],
    third_party: &["node_modules"],
    part_names: &[],
    module_separators: &["/"],
    settings: &[],
};

pub static PYTHON: Language = Language {
    name: "python",
    extensions: &["py", "pyi"],
    declarations: &[],
    tests: &["**/test_*.py", "**/*_test.py"],
    not_tests: &[],
    whole_directory: false,
    std: Some(stdlib::python),
    // A stub (`.pyi`) is imported by type checkers, not by the program.
    unused: &["py"],
    conventional: &["**/conftest.py", "**/__main__.py", "**/setup.py"],
    manifest: None,
    symbols: true,
    parts: None,
    runs_on_import: Some("__init__.py"),
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: None,
    foreign_dirs: &[],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

pub static JAVA: Language = Language {
    name: "java",
    extensions: &["java"],
    declarations: &[],
    tests: &["**/*Test.java", "**/*Tests.java"],
    not_tests: &[],
    whole_directory: false,
    std: Some(stdlib::java),
    unused: &["java"],
    conventional: &["**/package-info.java", "**/module-info.java"],
    manifest: None,
    // Its indexer reports member uses, but none is verified yet.
    symbols: false,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: None,
    foreign_dirs: &["target", "build"],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

/// Read by imports alone (an adapter), so `unused` cannot judge it.
pub static KOTLIN: Language = Language {
    name: "kotlin",
    extensions: &["kt", "kts"],
    declarations: &[],
    tests: &["**/*Test.kt", "**/*Tests.kt"],
    not_tests: &[],
    whole_directory: false,
    std: Some(stdlib::kotlin),
    unused: &[],
    conventional: &[],
    manifest: None,
    symbols: false,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[(
        &JAVA,
        "Kotlin beside it uses Java classes without an import",
    )],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: Some(super::kotlin::adapter),
    foreign_dirs: &[],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

pub static CSHARP: Language = Language {
    name: "csharp",
    extensions: &["cs"],
    declarations: &[],
    tests: &["**/*Test.cs", "**/*Tests.cs"],
    not_tests: &[],
    whole_directory: false,
    std: Some(stdlib::csharp),
    unused: &["cs"],
    conventional: &[],
    manifest: None,
    // Its indexer reports member uses, but none is verified yet.
    symbols: false,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: None,
    foreign_dirs: &["bin", "obj"],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

pub static PHP: Language = Language {
    name: "php",
    extensions: &["php"],
    declarations: &[],
    tests: &["**/*Test.php"],
    not_tests: &[],
    whole_directory: false,
    std: Some(stdlib::php),
    unused: &["php"],
    conventional: &[],
    manifest: None,
    // Its indexer reports member uses, but none is verified yet.
    symbols: false,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: None,
    foreign_dirs: &["vendor"],
    third_party: &[],
    part_names: &[],
    module_separators: &["\\"],
    settings: &[],
};

/// Read by imports alone (an adapter), so `unused` cannot judge it.
pub static SWIFT: Language = Language {
    name: "swift",
    extensions: &["swift"],
    declarations: &[],
    tests: &["**/*Tests.swift"],
    not_tests: &[],
    whole_directory: false,
    std: Some(stdlib::swift),
    unused: &[],
    conventional: &[],
    manifest: None,
    symbols: false,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: Some(super::swift::adapter),
    foreign_dirs: &[],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

/// Ruby loads by convention (autoloading), so `unused` cannot judge it.
pub static RUBY: Language = Language {
    name: "ruby",
    extensions: &["rb"],
    declarations: &[],
    tests: &[],
    not_tests: &[],
    whole_directory: false,
    std: None,
    unused: &[],
    conventional: &[],
    manifest: None,
    symbols: true,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: None,
    foreign_dirs: &["vendor"],
    third_party: &[],
    part_names: &[],
    module_separators: &["::"],
    settings: &[],
};

pub static RUST: Language = Language {
    name: "rust",
    extensions: &["rs"],
    declarations: &[],
    // Cargo's integration tests, benchmarks and examples: targets built with the
    // dev-dependencies. A module of one of those names under `src/` is the crate's
    // own code, and `parts` takes it for tests when `#[cfg(test)]` declares it.
    tests: &[
        "**/tests/**/*.rs",
        "**/benches/**/*.rs",
        "**/examples/**/*.rs",
    ],
    not_tests: &[
        "**/src/**/tests/**/*.rs",
        "**/src/**/benches/**/*.rs",
        "**/src/**/examples/**/*.rs",
    ],
    whole_directory: false,
    std: Some(stdlib::rust),
    unused: &["rs"],
    conventional: &[
        "**/build.rs",
        "**/src/main.rs",
        "**/src/lib.rs",
        "**/src/bin/**",
        "**/examples/**",
        "**/benches/**",
        "**/tests/**",
    ],
    manifest: Some(Manifest::Cargo),
    symbols: true,
    parts: Some(parts::rust),
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    adapter: None,
    foreign_dirs: &[],
    third_party: &[],
    part_names: &["cfg(test)"],
    module_separators: &["::"],
    settings: &[],
};

/// Every language, in the order their test globs are listed.
pub static LANGUAGES: [&Language; 10] = [
    &GO,
    &TYPESCRIPT,
    &PYTHON,
    &JAVA,
    &KOTLIN,
    &CSHARP,
    &PHP,
    &SWIFT,
    &RUBY,
    &RUST,
];

/// The language of a file, by its extension.
pub fn of(file: &str) -> Option<&'static Language> {
    let ext = relpath::extension(file);
    LANGUAGES
        .into_iter()
        .find(|language| language.extensions.contains(&ext))
}

impl Language {
    /// What its module paths drop from the end of a file name: each declaration
    /// suffix, then each extension, all with their dot (`.d.ts` before `.ts`).
    fn module_suffixes(&self) -> impl Iterator<Item = String> + '_ {
        self.declarations
            .iter()
            .chain(self.extensions)
            .map(|suffix| format!(".{suffix}"))
    }

    /// The module path of one of its files: `file` without its declaration suffix
    /// or else its extension (`src/a.d.ts` and `src/a.ts` are `src/a`).
    pub fn module_path<'a>(&self, file: &'a str) -> &'a str {
        self.module_suffixes()
            .find_map(|suffix| file.strip_suffix(&suffix).filter(|stem| names_a_file(stem)))
            .unwrap_or(file)
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

/// The module path of a file: its path without the suffix its language drops, a
/// declaration suffix before an extension, so `src/a.d.ts` and `src/a.ts` are both
/// `src/a`. A file of no language loses a last extension of letters
/// (`contacts.vue` is `contacts`), so a file and a directory of one name are one
/// module; a name that does not split at a dot (`Makefile`, `.env`) is itself.
pub fn module_path(file: &str) -> &str {
    if let Some(language) = of(file) {
        return language.module_path(file);
    }
    match file.rsplit_once('.') {
        Some((stem, ext))
            if names_a_file(stem)
                && !ext.is_empty()
                && ext.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            stem
        }
        _ => file,
    }
}

/// Whether `written`, a module path as a policy may spell it, names the module
/// `module` with a suffix that `module_path` drops left on: whole (`pkg/foo.cjs`),
/// or cut at its last dot as a file name used to be (`src/types.d` of
/// `src/types.d.ts`). Such a spelling names nothing, and `module` is what it means.
pub fn spells_with_suffix(written: &str, module: &str) -> bool {
    let Some(rest) = written.strip_prefix(module).filter(|rest| !rest.is_empty()) else {
        return false;
    };
    LANGUAGES
        .iter()
        .flat_map(|language| language.module_suffixes())
        .any(|suffix| suffix == rest || suffix.rsplit_once('.').is_some_and(|(cut, _)| cut == rest))
}

/// Whether `stem`, a path cut before a dot, still ends in a name: not `""` of
/// `.env`, nor `src/` of `src/.env`.
fn names_a_file(stem: &str) -> bool {
    !stem.is_empty() && !stem.ends_with('/')
}

/// Whether `file` is a declaration file of its language (`a.d.ts`), which
/// declares the types of code written elsewhere.
pub fn is_declaration(file: &str) -> bool {
    of(file).is_some_and(|language| language.declares(file))
}

/// Whether a directory of this name holds the third-party code of a language's
/// toolchain, which easement never walks.
pub fn is_third_party(dir: &str) -> bool {
    LANGUAGES
        .iter()
        .any(|language| language.third_party.contains(&dir))
}

/// The kind of manifest a file is, by its name.
pub fn manifest_of(file: &str) -> Option<Manifest> {
    let name = crate::relpath::file_name(file);
    MANIFESTS
        .into_iter()
        .find(|manifest| manifest.file_name() == name)
}

/// The settings of the indexers the policy writes, by language: `go: { tags: [race] }`.
pub type IndexerSettings =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, Vec<String>>>;

/// Whether every setting the policy writes is one its indexer takes, with a value it
/// takes; the error says what was meant.
pub fn check_settings(settings: &IndexerSettings) -> Result<(), String> {
    for (name, given) in settings {
        let Some(language) = LANGUAGES.iter().find(|language| language.name == name) else {
            let known: Vec<&str> = LANGUAGES.iter().map(|language| language.name).collect();
            return Err(format!(
                "indexers: `{name}` is no language easement indexes; one of: {}",
                known.join(", ")
            ));
        };
        for (key, values) in given {
            let Some(setting) = language.settings.iter().find(|s| s.key == key) else {
                let known: Vec<&str> = language.settings.iter().map(|s| s.key).collect();
                return Err(match known.is_empty() {
                    true => format!("indexers.{name}: the {name} indexer takes no setting"),
                    false => format!(
                        "indexers.{name}: `{key}` is no setting of the {name} indexer; one of: {}",
                        known.join(", ")
                    ),
                });
            };
            if let Some(bad) = values.iter().find(|value| !(setting.valid)(value)) {
                return Err(format!(
                    "indexers.{name}.{key}: `{bad}` is not {}",
                    setting.value
                ));
            }
        }
    }
    Ok(())
}

/// The variables the indexer of `language` runs with for the settings the policy
/// writes for it.
pub fn settings_env(
    language: &Language,
    settings: &IndexerSettings,
    inherited: Inherited,
) -> Vec<(&'static str, String)> {
    let Some(given) = settings.get(language.name) else {
        return Vec::new();
    };
    language
        .settings
        .iter()
        .filter_map(|setting| {
            let values = given.get(setting.key).filter(|values| !values.is_empty())?;
            Some((setting.env)(values, inherited))
        })
        .collect()
}

/// Where the module names a file imports are cut into parents: as its language
/// cuts them, a manifest as the language it serves, and at every language's
/// separators for a file of no language.
fn module_separators(file: &str) -> &'static [&'static str] {
    of(file)
        .or_else(|| {
            let manifest = manifest_of(file)?;
            LANGUAGES
                .into_iter()
                .find(|language| language.manifest == Some(manifest))
        })
        .map_or(every_module_separator(), |language| {
            language.module_separators
        })
}

/// Every separator some language cuts its module names at, for a name that
/// carries no language.
fn every_module_separator() -> &'static [&'static str] {
    static EVERY: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        let every: BTreeSet<&'static str> = LANGUAGES
            .iter()
            .flat_map(|language| language.module_separators.iter().copied())
            .collect();
        every.into_iter().collect()
    });
    &EVERY
}

/// The parents of the module `name` that `file` imports, outermost first, cut
/// at the separators `module_separators` gives for `file`: `serde` of
/// `serde::de`, `lodash` of `lodash/fp`.
pub fn module_parents<'a>(file: &str, name: &'a str) -> impl Iterator<Item = &'a str> {
    parents(module_separators(file), name)
}

/// Whether the module `name` sits below `parent`, cut at a separator of some
/// language: `App` nests `App\Domain`, `I18n` nests `I18n::Base`, and `App`
/// nests neither itself nor `ApplicationX\Order`.
pub fn nests(parent: &str, name: &str) -> bool {
    parents(every_module_separator(), name).any(|above| above == parent)
}

/// The parents of the module `name`, outermost first, cut at `separators`.
fn parents<'a>(
    separators: &'static [&'static str],
    name: &'a str,
) -> impl Iterator<Item = &'a str> {
    let mut cuts: Vec<usize> = separators
        .iter()
        .flat_map(|separator| name.match_indices(separator).map(|(i, _)| i))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.into_iter().map(move |i| &name[..i])
}

/// Where the parts that `part:` names come from, as a reader is told it.
pub fn part_sources() -> String {
    let quoted = |names: &[&str]| -> String {
        names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let manifests: Vec<&str> = MANIFESTS.iter().flat_map(|m| m.parts()).copied().collect();
    let mut sources = vec![format!(
        "the manifests `packages` reads ({})",
        quoted(&manifests)
    )];
    sources.extend(
        LANGUAGES
            .iter()
            .filter(|language| language.parts.is_some())
            .map(|language| format!("{} source ({})", language.name, quoted(language.part_names))),
    );
    sources.join(" and ")
}

/// Whether the file at `path` is of `language`.
pub fn is(path: &std::path::Path, language: &Language) -> bool {
    path.to_str()
        .and_then(of)
        .is_some_and(|found| std::ptr::eq(found, language))
}

/// The test globs of every language, which `tests: auto` covers.
pub fn test_patterns() -> Vec<&'static str> {
    LANGUAGES
        .iter()
        .flat_map(|language| language.tests.iter().copied())
        .collect()
}

/// The files built together, as a Go package is: for a `whole_directory`
/// language, its files of one directory but its test files. Every other file
/// is built on its own and is in no group.
pub fn built_together<'a>(files: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut groups: std::collections::BTreeMap<(&str, &str), Vec<&'a str>> =
        std::collections::BTreeMap::new();
    let mut tests: std::collections::HashMap<&str, Option<globset::GlobSet>> =
        std::collections::HashMap::new();
    for file in files {
        let Some(language) = of(file).filter(|language| language.whole_directory) else {
            continue;
        };
        let is_test = tests
            .entry(language.name)
            .or_insert_with(|| {
                let mut set = globset::GlobSetBuilder::new();
                for glob in language.tests {
                    set.add(globset::Glob::new(glob).ok()?);
                }
                set.build().ok()
            })
            .as_ref()
            .is_none_or(|set| set.is_match(file));
        if !is_test {
            groups
                .entry((relpath::parent(file), language.name))
                .or_default()
                .push(file);
        }
    }
    groups.into_values().collect()
}

/// Whether the file or directory `path` is test code by its name or place: a
/// test glob of a language matches it, and no glob of the files those take
/// that are not tests. `tests: auto` gives such a file to its `tests`.
pub fn is_test_file(path: &str) -> bool {
    let matches = |globs: Vec<&str>| {
        globs.into_iter().any(|glob| {
            [glob.to_string(), format!("{glob}/**")].iter().any(|g| {
                globset::GlobBuilder::new(g)
                    .literal_separator(true)
                    .build()
                    .is_ok_and(|g| g.compile_matcher().is_match(path))
            })
        })
    };
    matches(test_patterns()) && !matches(not_test_patterns())
}

/// The files the test globs match that are not tests, which `tests: auto` gives up.
pub fn not_test_patterns() -> Vec<&'static str> {
    LANGUAGES
        .iter()
        .flat_map(|language| language.not_tests.iter().copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module nests another only across a separator some language cuts at (#108).
    #[test]
    fn a_module_nests_only_what_sits_below_it_at_a_separator() {
        for (parent, name) in [
            ("App", "App\\Domain"),
            ("I18n", "I18n::Base"),
            ("java", "java.util"),
            ("lodash", "lodash/fp"),
        ] {
            assert!(nests(parent, name), "{parent} nests {name}");
        }
        for (parent, name) in [
            ("App", "ApplicationX\\Order"),
            ("App", "App"),
            ("App", "Ap"),
        ] {
            assert!(!nests(parent, name), "{parent} does not nest {name}");
        }
    }

    /// A language whose files an adapter reads may declare a name in several
    /// files; `unused` judges only the files of the languages an indexer
    /// covers, so it never sees those candidates.
    #[test]
    fn a_language_an_adapter_reads_is_not_judged_by_unused() {
        for language in LANGUAGES {
            if language.adapter.is_some() {
                assert!(
                    language.unused.is_empty(),
                    "`unused` judges the {} files, whose imports an adapter reads",
                    language.name
                );
            }
        }
    }

    /// What each part of easement makes of a file by its name, from the one
    /// registry: the indexer and the adapter that read it, whether `$std` and
    /// `unused` know it, and its manifest.
    #[test]
    fn every_part_tells_a_language_by_its_file_alike() {
        let files = [
            "a.go", "a.py", "a.pyi", "a.ts", "a.d.ts", "a.d.mts", "a.d.cts", "a.tsx", "a.mts",
            "a.cts", "a.js", "a.jsx", "a.mjs", "a.cjs", "A.java", "A.kt", "a.kts", "A.cs", "a.php",
            "a.rb", "a.rs", "a.swift", "Makefile", "v1.2/x", "a.json",
        ];
        let mut table = String::new();
        for file in files {
            let covered: Vec<&str> = crate::indexer::INDEXERS
                .iter()
                .filter(|l| l.covers(file))
                .map(|l| l.name())
                .collect();
            let adapters: Vec<&str> = crate::lang::adapters(
                std::path::Path::new("/nonexistent"),
                &["A.kt", "a.swift"].map(std::path::PathBuf::from),
            )
            .unwrap()
            .iter()
            .filter(|a| a.handles(std::path::Path::new(file)))
            .map(|a| a.name())
            .collect();
            table.push_str(&format!(
                "{file}: indexer {covered:?}, adapter {adapters:?}, std {}, unused {:?}, manifest {:?}\n",
                crate::stdlib::knows(file),
                crate::unused::family(file),
                of(file).and_then(|l| l.manifest),
            ));
        }
        insta::assert_snapshot!("languages_by_file", table);
        insta::assert_snapshot!("test_patterns", test_patterns().join("\n"));
    }

    /// One rule names the module of a file in every language: its language's
    /// declaration suffix or extension goes, nothing else does.
    #[test]
    fn a_file_is_its_module_without_the_suffix_its_language_drops() {
        for (file, module) in [
            ("src/types/types.d.ts", "src/types/types"),
            ("src/a.d.mts", "src/a"),
            ("src/a.d.cts", "src/a"),
            ("src/a.ts", "src/a"),
            ("src/a.cjs", "src/a"),
            ("src/a.jsx", "src/a"),
            // A `d` that ends no declaration suffix is part of the name.
            ("src/a.d.go", "src/a.d"),
            ("internal/db/db.go", "internal/db/db"),
            ("app/db/client.py", "app/db/client"),
            ("app/db/client.pyi", "app/db/client"),
            ("src/lang/mod.rs", "src/lang/mod"),
            ("lib/shop/db.rb", "lib/shop/db"),
            // A file of no language loses a last extension of letters alone.
            ("src/pages/contacts.vue", "src/pages/contacts"),
            ("v1.2", "v1.2"),
            ("Makefile", "Makefile"),
            (".env", ".env"),
            ("src/.env", "src/.env"),
            ("src/.ts", "src/.ts"),
        ] {
            assert_eq!(module_path(file), module, "{file}");
        }
        assert!(is_declaration("src/a.d.ts") && is_declaration("a.d.cts"));
        assert!(
            !is_declaration("src/a.ts") && !is_declaration(".d.ts") && !is_declaration("a.pyi")
        );
    }

    #[test]
    fn a_module_spelled_with_a_file_suffix_names_the_module_without_it() {
        assert!(spells_with_suffix("src/types.d", "src/types"));
        assert!(spells_with_suffix("src/types.d.ts", "src/types"));
        assert!(spells_with_suffix("pkg/foo.cjs", "pkg/foo"));
        assert!(!spells_with_suffix("pkg/foo", "pkg/foo"));
        assert!(!spells_with_suffix("pkg/foo.x", "pkg/foo"));
        assert!(!spells_with_suffix("pkg/foobar", "pkg/foo"));
    }

    #[test]
    fn a_go_package_is_built_together_without_its_tests() {
        let files = [
            "util/math.go",
            "util/retry.go",
            "util/retry_test.go",
            "util/page.ts",
            "util/inner/deep.go",
            "web/page.ts",
        ];
        assert_eq!(
            built_together(&files),
            [
                vec!["util/math.go", "util/retry.go"],
                vec!["util/inner/deep.go"]
            ]
        );
    }
}
