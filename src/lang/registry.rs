//! What easement knows of each language, told by the extensions of its files:
//! data, and hooks for what a rule needs done differently, whose code sits in
//! `lang/<language>.rs`. Adding a language is one entry here, its indexer or its
//! adapter among it, which `indexer.rs` runs or `lang/<language>.rs` builds, and
//! how the scheme of its index reads, which `scip/symbol.rs` follows. The rules ask
//! this table, never an extension or a language by name; `easement.yml` holds them
//! to it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use super::parts::Found;
use super::{Adapter, Cause, Import};
use crate::relpath;
use crate::scip::symbol::{Module, Name, Parsed, Qualifier};
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
    /// The packages its index may leave unnamed, which only an import names.
    pub unnamed: Option<UnnamedPackages>,
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

/// Packages an index may leave unnamed, so that only the string of the import
/// names them (scip-typescript under a `package.json` without `name`).
#[derive(Debug)]
pub struct UnnamedPackages {
    /// Whether the index left the package of `symbol` unnamed.
    pub is: fn(&str) -> bool,
    /// The module an import names, from the string the reference stands on and the
    /// path of the file that writes it; `None` when it names no package installed
    /// where the file is.
    pub spelled: fn(&str, &Path) -> Option<String>,
    /// Why the package of such a name is unknown, for the report.
    pub cause: crate::symbols::Unnamed,
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
    /// Whether its index gives each method and function the lines it spans, which
    /// `impact --diff` maps the changed lines onto. Without them a changed file is
    /// one whole, and an evidence gap says so.
    pub definition_ranges: bool,
    /// What its index gives of the types a type implements or extends, which the
    /// `implementation` rule judges. A language without declared relations is an
    /// evidence gap for that rule.
    pub relations: Relations,
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
    /// in `node_modules` is not the project's either.
    pub third_party: &'static [&'static str],
    /// What its indexer may be run with, under `indexers.<name>` of the policy.
    pub settings: &'static [Setting],
    /// Where its module names are cut into parents (`/` of `lodash/fp`, `::` of
    /// `serde::de`): a pattern of a parent covers what is below it.
    pub module_separators: &'static [&'static str],
    /// What joins a type and a member of it in a name as `forbid_symbol` writes it
    /// (`.` of `db.Client.Query`, `::` of `db::client::Client.query`), as its index
    /// is read in `src/scip/symbol.rs`.
    pub member_separator: &'static str,
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
    definition_ranges: true,
    relations: Relations::Structural {
        because: "a go type implements an interface by having its methods, which declares nothing",
    },
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
    reader: Reader::Indexer(&Indexer {
        binary: "scip-go",
        scheme: "scip-go",
        symbols: Symbols::Modules {
            qualifier: super::go::qualifier,
            external: super::go::external,
        },
        unnamed: None,
        markers: &["go.mod"],
        outermost: false,
        fallback_root: false,
        args: |out| vec!["--output".into(), out.display().to_string()],
        install: "mise use -g go:github.com/scip-code/scip-go/cmd/scip-go",
        mise: Some("go:github.com/scip-code/scip-go/cmd/scip-go"),
        not_on_windows: None,
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: nothing_needed,
        inputs: super::go::inputs,
        env: super::go::env,
        hints: &[],
    }),
    foreign_dirs: &["vendor", "testdata"],
    third_party: &[],
    part_names: &[],
    module_separators: &["/"],
    member_separator: ".",
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
    definition_ranges: true,
    relations: Relations::Declared {
        unrelated: Some(Unrelated {
            find: super::typescript::unrelated,
            because: "scip-typescript gives an interface declaration, a class expression or a class that implements a type alias no relations",
        }),
        anonymous: None,
    },
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
    reader: Reader::Indexer(&Indexer {
        binary: "scip-typescript",
        scheme: "scip-typescript",
        symbols: Symbols::Modules {
            qualifier: super::typescript::qualifier,
            external: super::typescript::external,
        },
        unnamed: Some(super::typescript::UNNAMED),
        markers: &["tsconfig.json"],
        outermost: false,
        fallback_root: false,
        args: |out| {
            vec![
                "index".into(),
                "--no-progress-bar".into(),
                "--output".into(),
                out.display().to_string(),
            ]
        },
        install: "mise use -g npm:@sourcegraph/scip-typescript",
        mise: Some("npm:@sourcegraph/scip-typescript"),
        not_on_windows: None,
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: super::typescript::needs,
        inputs: super::typescript::inputs,
        env: |_| false,
        hints: &[],
    }),
    foreign_dirs: &[],
    third_party: &["node_modules"],
    part_names: &[],
    module_separators: &["/"],
    member_separator: ".",
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
    definition_ranges: true,
    relations: Relations::Declared {
        unrelated: None,
        anonymous: None,
    },
    parts: None,
    runs_on_import: Some("__init__.py"),
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: Some(WrittenImports {
        list: super::python::written_imports,
        explained: super::python::EXPLAINED,
        spelled: Some(super::python::spelled),
    }),
    reader: Reader::Indexer(&Indexer {
        binary: "scip-python",
        scheme: "scip-python",
        symbols: Symbols::Modules {
            qualifier: super::python::qualifier,
            external: super::python::external,
        },
        unnamed: None,
        markers: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        outermost: false,
        fallback_root: true,
        args: |out| {
            vec![
                "index".into(),
                ".".into(),
                "--quiet".into(),
                "--project-name".into(),
                "project".into(),
                "--project-version".into(),
                "0".into(),
                "--output".into(),
                out.display().to_string(),
            ]
        },
        install: "mise use -g npm:@sourcegraph/scip-python",
        mise: Some("npm:@sourcegraph/scip-python"),
        not_on_windows: Some(
            "scip-python crashes on start there (https://github.com/sourcegraph/scip-python/issues/210, fix pending in https://github.com/sourcegraph/scip-python/pull/224)",
        ),
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: nothing_needed,
        inputs: every_own_file,
        env: super::python::env,
        hints: &[],
    }),
    foreign_dirs: &[],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    member_separator: ".",
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
    definition_ranges: true,
    relations: Relations::Declared {
        unrelated: None,
        anonymous: Some(
            "scip-java gives an anonymous class no type, only its methods relations to the methods they implement",
        ),
    },
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Indexer(&Indexer {
        binary: "scip-java",
        scheme: "scip-java",
        // scip-java gives `new X()` only as the constructor of `X`.
        symbols: Symbols::Classes {
            separator: ".",
            constructor: Some("<init>"),
            owned: true,
        },
        unnamed: None,
        markers: &["pom.xml", "build.gradle", "build.gradle.kts"],
        outermost: true,
        fallback_root: false,
        args: |out| vec!["index".into(), "--output".into(), out.display().to_string()],
        install: "mise use -g github:sourcegraph/scip-java",
        mise: Some("github:sourcegraph/scip-java"),
        not_on_windows: Some(
            "the scip-java release ships a POSIX launcher only, which Windows cannot run",
        ),
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: nothing_needed,
        inputs: every_own_file,
        env: super::java::env,
        hints: &[Hint {
            output: "LAUNCHER_ARGS[@]: unbound variable",
            hint: "scip-java's wrapper needs bash 4.4 or newer; the bash of macOS is 3.2, put a newer one first on the PATH (`brew install bash`)",
        }],
    }),
    foreign_dirs: &["target", "build"],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    member_separator: ".",
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
    definition_ranges: false,
    relations: Relations::None {
        because: "kotlin files are read without an index, which declares no implementations",
    },
    parts: None,
    runs_on_import: None,
    uses_unimported: &[(
        &JAVA,
        "Kotlin beside it uses Java classes without an import",
    )],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Adapter(super::kotlin::adapter),
    foreign_dirs: &[],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    member_separator: ".",
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
    definition_ranges: false,
    relations: Relations::Declared {
        unrelated: None,
        anonymous: None,
    },
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Indexer(&Indexer {
        binary: "scip-dotnet",
        scheme: "scip-dotnet",
        // scip-dotnet gives an attribute `[X]` only as the constructor of `XAttribute`,
        // and the project's own types and the framework's the same header.
        symbols: Symbols::Classes {
            separator: ".",
            constructor: Some(".ctor"),
            owned: false,
        },
        unnamed: None,
        markers: &["*.csproj", "*.sln", "*.slnx"],
        outermost: true,
        fallback_root: false,
        args: |out| vec!["index".into(), "--output".into(), out.display().to_string()],
        install: "dotnet tool install --global scip-dotnet",
        mise: None,
        not_on_windows: None,
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: nothing_needed,
        inputs: every_own_file,
        env: super::csharp::env,
        hints: &[],
    }),
    foreign_dirs: &["bin", "obj"],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    member_separator: ".",
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
    definition_ranges: false,
    relations: Relations::None {
        because: "the php index declares no implementations",
    },
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Indexer(&Indexer {
        binary: "scip-php",
        scheme: "scip-php",
        symbols: Symbols::Classes {
            separator: "\\",
            constructor: None,
            owned: true,
        },
        unnamed: None,
        markers: &["composer.json"],
        outermost: false,
        fallback_root: false,
        args: |_| Vec::new(),
        install: "composer global require davidrjenni/scip-php",
        mise: None,
        not_on_windows: Some(
            "scip-php leaves every file out of its index there (https://github.com/davidrjenni/scip-php/issues/814)",
        ),
        writes_in_cwd: Some("index.scip"),
        version_flag: None,
        needs: super::php::needs,
        inputs: every_own_file,
        env: |_| false,
        hints: &[],
    }),
    foreign_dirs: &["vendor"],
    third_party: &[],
    part_names: &[],
    module_separators: &["\\"],
    member_separator: ".",
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
    definition_ranges: false,
    relations: Relations::None {
        because: "swift files are read without an index, which declares no implementations",
    },
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Adapter(super::swift::adapter),
    foreign_dirs: &[],
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    member_separator: ".",
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
    definition_ranges: true,
    relations: Relations::None {
        because: "the ruby index declares no implementations",
    },
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Indexer(&Indexer {
        binary: "scip-ruby",
        scheme: "scip-ruby",
        symbols: Symbols::Own(super::ruby::read),
        unnamed: None,
        markers: &["Gemfile", "*.gemspec"],
        outermost: true,
        fallback_root: false,
        args: |out| {
            vec![
                "--gem-metadata".into(),
                "project@0".into(),
                "--index-file".into(),
                out.display().to_string(),
                ".".into(),
            ]
        },
        install: "mise use -g github:sourcegraph/scip-ruby",
        mise: Some("github:sourcegraph/scip-ruby"),
        not_on_windows: Some("scip-ruby has no Windows release"),
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: nothing_needed,
        inputs: every_own_file,
        // scip-ruby reads no variable that changes the index; Bundler's are read
        // by the gem's wrapper, which easement does not run.
        env: |_| false,
        hints: &[],
    }),
    foreign_dirs: &["vendor"],
    third_party: &[],
    part_names: &[],
    module_separators: &["::"],
    member_separator: "::",
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
    definition_ranges: true,
    relations: Relations::None {
        because: "the rust index declares no implementations",
    },
    parts: Some(super::rust::parts),
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: None,
    written_imports: None,
    reader: Reader::Indexer(&Indexer {
        binary: "rust-analyzer",
        scheme: "rust-analyzer",
        symbols: Symbols::Modules {
            qualifier: super::rust::qualifier,
            external: super::rust::external,
        },
        unnamed: None,
        markers: &["Cargo.toml"],
        outermost: true,
        fallback_root: false,
        args: |out| {
            vec![
                "scip".into(),
                ".".into(),
                "--output".into(),
                out.display().to_string(),
            ]
        },
        install: "rustup component add rust-analyzer",
        mise: None,
        not_on_windows: None,
        writes_in_cwd: None,
        version_flag: Some("--version"),
        needs: nothing_needed,
        inputs: every_own_file,
        env: super::rust::env,
        hints: &[],
    }),
    foreign_dirs: &[],
    third_party: &[],
    part_names: &["cfg(test)"],
    module_separators: &["::"],
    member_separator: "::",
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

/// The language whose indexer writes symbols of `scheme`.
pub fn of_scheme(scheme: &str) -> Option<&'static Language> {
    LANGUAGES
        .into_iter()
        .find(|language| language.indexer().is_some_and(|i| i.scheme == scheme))
}

/// The language of a file, by its extension.
pub fn of(file: &str) -> Option<&'static Language> {
    let ext = relpath::extension(file);
    LANGUAGES
        .into_iter()
        .find(|language| language.extensions.contains(&ext))
}

impl Language {
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

/// Whether a directory of this name is one some language's toolchain fills with
/// code that is not the project's own: build output (`target`, `bin`), installs
/// (`vendor`, `node_modules`), fixtures it ignores (`testdata`).
pub fn is_toolchain_dir(dir: &str) -> bool {
    LANGUAGES
        .iter()
        .any(|language| language.foreign_dirs.contains(&dir) || language.third_party.contains(&dir))
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

/// The type that a member, written in a name used in `file` as `forbid_symbol`
/// writes it, would be used through: `qualifier` cut at the member separator of
/// the file's language into the qualifier of the type and the type (`ent` and
/// `Client` of `ent.Client`), never inside the name of a module. `None` for a
/// file of no language, or a qualifier that names no type.
pub fn member_type<'a>(file: &str, qualifier: &'a str) -> Option<MemberType<'a>> {
    let language = of(file)?;
    let separator = language.member_separator;
    let (outer, ty) = qualifier.rsplit_once(separator)?;
    let module = |separator: &&str| ty.contains(*separator);
    (!outer.is_empty() && !ty.is_empty() && !language.module_separators.iter().any(module))
        .then_some(MemberType {
            qualifier: outer,
            name: ty,
            certain: !language.module_separators.contains(&separator),
        })
}

/// The type a written member is used through, as [`member_type`] cuts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberType<'a> {
    pub qualifier: &'a str,
    pub name: &'a str,
    /// Whether the name is a member for sure: its language cuts modules at
    /// other separators (Go's `/`), so the cut can be no module's. Where it cuts
    /// them at the same one (`.` in Python, `::` in Rust), the qualifier may as
    /// well end in a module, and the name be no member.
    pub certain: bool,
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

/// Whether the module `name` ends with `tail`, cut at a separator of some
/// language: `tags` ends `example.com/app/tags`, `client` ends `app::db::client`
/// and `app.db.client`, `Domain` ends `App\Domain`, and each module ends itself;
/// `ent` does not end `client` nor `content`.
pub fn ends_with_module(name: &str, tail: &str) -> bool {
    name.strip_suffix(tail).is_some_and(|rest| {
        rest.is_empty()
            || every_module_separator()
                .iter()
                .any(|separator| rest.ends_with(separator))
    })
}

/// The parents of the module `name`, outermost first, cut at a separator of
/// any language: what a module is read as when no file says which language
/// writes it (`lodash` of `lodash/fp`, `okhttp3` of `okhttp3.Client`).
pub fn module_parents_anywhere(name: &str) -> impl Iterator<Item = &str> {
    parents(every_module_separator(), name)
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

    #[test]
    fn a_module_ends_with_what_follows_a_separator_of_any_language() {
        for (name, tail) in [
            ("example.com/app/tags", "tags"),
            ("app::db::client", "client"),
            ("app::db::client", "db::client"),
            ("app.db.client", "client"),
            ("App\\Domain", "Domain"),
            ("tags", "tags"),
        ] {
            assert!(ends_with_module(name, tail), "{tail} ends {name}");
        }
        for (name, tail) in [
            ("app::db::content", "ent"),
            ("app.db.client", "ient"),
            ("tags", "example.com/app/tags"),
            ("app::db", "app"),
        ] {
            assert!(!ends_with_module(name, tail), "{tail} does not end {name}");
        }
    }

    /// The cache key of an indexer whose build reads any file reads every file the
    /// project owns, not what its toolchain installs or builds.
    #[test]
    fn every_own_file_leaves_out_the_code_a_toolchain_fills_in() {
        assert!(every_own_file(&GO, "internal/a.go"));
        assert!(every_own_file(&GO, "README.md"));
        assert!(!every_own_file(&GO, "vendor/example.com/x/x.go"));
        assert!(!every_own_file(&JAVA, "target/classes/A.class"));
    }

    /// A language whose files an adapter reads may declare a name in several
    /// files; `unused` judges only the files of the languages an indexer
    /// covers, so it never sees those candidates.
    #[test]
    fn a_language_an_adapter_reads_is_not_judged_by_unused() {
        for language in LANGUAGES {
            if language.adapter().is_some() {
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
            let covered = crate::indexer::covering(file);
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
