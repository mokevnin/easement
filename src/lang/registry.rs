//! What easement knows of each language, told by the extensions of its files:
//! data, and hooks for what a rule needs done differently, whose code sits in
//! `lang/<language>.rs`. Adding a language is one entry here, its indexer or its
//! adapter among it, which `indexer.rs` runs or `lang/<language>.rs` builds, and
//! how the scheme of its index reads, which `scip/symbol.rs` follows. Each kind of
//! manifest is an entry here too ([`Manifest`]), whose hooks' code sits in
//! `packages/<kind>.rs`. The rules ask this table, never an extension, a language or
//! a kind of manifest by name; `easement.yml` holds them to it.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use super::Cause;
use super::language::{
    Comments, Explained, Hint, Indexer, Inherited, Language, Parts, Reader, Relations, Setting,
    Std, Symbols, Unrelated, WrittenImports, every_own_file, names_a_file, nothing_needed,
};
use crate::packages::manifest::Manifest;
use crate::packages::{cargo, go as gomod, npm};
use crate::relpath;

/// `package.json`, which npm, pnpm, Yarn and Bun read.
pub static PACKAGE_JSON: Manifest = Manifest {
    file_name: "package.json",
    part_names: npm::PART_NAMES,
    declared: npm::declared,
    declares: npm::importable,
    package: Some(npm::package),
    named_by: Some(npm::named_by),
    covers: None,
    named_in_source: Some(npm::named_in_source),
    namer: Some(npm::name),
    spelled_in_import: None,
    entries: Some(npm::entries),
    frameworks: Some(npm::frameworks),
};

/// `go.mod`.
pub static GO_MOD: Manifest = Manifest {
    file_name: "go.mod",
    part_names: &[],
    declared: gomod::declared,
    declares: gomod::importable,
    package: None,
    named_by: None,
    covers: Some(gomod::covers),
    named_in_source: None,
    namer: Some(gomod::name),
    spelled_in_import: None,
    entries: None,
    frameworks: None,
};

/// `Cargo.toml`. A dependency names a path or the workspace, never a package by
/// its name.
pub static CARGO_TOML: Manifest = Manifest {
    file_name: "Cargo.toml",
    part_names: cargo::PART_NAMES,
    declared: cargo::declared,
    declares: cargo::importable,
    package: Some(cargo::package),
    named_by: None,
    covers: None,
    named_in_source: Some(cargo::named_in_source),
    namer: None,
    spelled_in_import: Some(cargo::spelled_in_import),
    entries: None,
    frameworks: None,
};

/// Every kind of manifest.
pub static MANIFESTS: [&Manifest; 3] = [&PACKAGE_JSON, &GO_MOD, &CARGO_TOML];

/// `//` and `/* */`, as the languages of C's family write comments.
const C_COMMENTS: Comments = Comments {
    line: &["//"],
    block: &[("/*", "*/")],
};

/// `#` to the end of the line.
const HASH_COMMENTS: Comments = Comments {
    line: &["#"],
    block: &[],
};

const TYPESCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

pub static GO: Language = Language {
    name: "go",
    extensions: &["go"],
    comments: C_COMMENTS,
    declarations: &[],
    tests: &["**/*_test.go"],
    not_tests: &[],
    whole_directory: true,
    std: Some(Std::Rule(super::go::std)),
    unused: &["go"],
    conventional: &[],
    manifest: Some(&GO_MOD),
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
    comments: C_COMMENTS,
    declarations: &["d.ts", "d.mts", "d.cts"],
    tests: &["**/*.test.*", "**/*.spec.*", "**/__tests__/**"],
    not_tests: &[],
    whole_directory: false,
    std: None,
    unused: TYPESCRIPT_EXTENSIONS,
    conventional: &[],
    manifest: Some(&PACKAGE_JSON),
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
    module_separators: &["/"],
    member_separator: ".",
    settings: &[],
};

pub static PYTHON: Language = Language {
    name: "python",
    extensions: &["py", "pyi"],
    comments: HASH_COMMENTS,
    declarations: &[],
    tests: &["**/test_*.py", "**/*_test.py"],
    not_tests: &[],
    whole_directory: false,
    std: Some(Std::Roots(super::python::STD)),
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
    module_separators: &["."],
    member_separator: ".",
    settings: &[],
};

pub static JAVA: Language = Language {
    name: "java",
    extensions: &["java"],
    comments: C_COMMENTS,
    declarations: &[],
    tests: &["**/*Test.java", "**/*Tests.java"],
    not_tests: &[],
    whole_directory: false,
    std: Some(Std::Rule(super::java::std)),
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
    module_separators: &["."],
    member_separator: ".",
    settings: &[],
};

/// Read by imports alone (an adapter), so `unused` cannot judge it.
pub static KOTLIN: Language = Language {
    name: "kotlin",
    extensions: &["kt", "kts"],
    comments: C_COMMENTS,
    declarations: &[],
    tests: &["**/*Test.kt", "**/*Tests.kt"],
    not_tests: &[],
    whole_directory: false,
    std: Some(Std::Rule(super::kotlin::std)),
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
    module_separators: &["."],
    member_separator: ".",
    settings: &[],
};

pub static CSHARP: Language = Language {
    name: "csharp",
    extensions: &["cs"],
    comments: C_COMMENTS,
    declarations: &[],
    tests: &["**/*Test.cs", "**/*Tests.cs"],
    not_tests: &[],
    whole_directory: false,
    std: Some(Std::Roots(&["System"])),
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
    module_separators: &["."],
    member_separator: ".",
    settings: &[],
};

pub static PHP: Language = Language {
    name: "php",
    extensions: &["php"],
    // A PHP file holds HTML around its code.
    comments: Comments {
        line: &["//", "#"],
        block: &[("/*", "*/"), ("<!--", "-->")],
    },
    declarations: &[],
    tests: &["**/*Test.php"],
    not_tests: &[],
    whole_directory: false,
    std: Some(Std::Rule(super::php::std)),
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
    module_separators: &["\\"],
    member_separator: ".",
    settings: &[],
};

/// Read by imports alone (an adapter), so `unused` cannot judge it.
pub static SWIFT: Language = Language {
    name: "swift",
    extensions: &["swift"],
    comments: C_COMMENTS,
    declarations: &[],
    tests: &["**/*Tests.swift"],
    not_tests: &[],
    whole_directory: false,
    std: Some(Std::Roots(super::swift::STD)),
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
    module_separators: &["."],
    member_separator: ".",
    settings: &[],
};

/// Ruby loads by convention (autoloading), so `unused` cannot judge it.
pub static RUBY: Language = Language {
    name: "ruby",
    extensions: &["rb"],
    comments: HASH_COMMENTS,
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
    module_separators: &["::"],
    member_separator: "::",
    settings: &[],
};

pub static RUST: Language = Language {
    name: "rust",
    extensions: &["rs"],
    comments: C_COMMENTS,
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
    std: Some(Std::Roots(&["std", "proc_macro"])),
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
    manifest: Some(&CARGO_TOML),
    symbols: true,
    definition_ranges: true,
    relations: Relations::None {
        because: "the rust index declares no implementations",
    },
    parts: Some(Parts {
        find: super::rust::parts,
        names: &[super::rust::TEST],
        test: super::rust::TEST,
        unreadable: super::rust::UNREADABLE,
        missing_file: super::rust::MODULE_IN_ANOTHER_FILE,
    }),
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

/// Whether `module`, imported by `file`, belongs to the standard library of the
/// file's language. A language whose entry says nothing of it never matches.
pub fn is_std(file: &str, module: &str) -> bool {
    of(file).is_some_and(|language| language.is_std(module))
}

/// Whether `$std` can tell the standard library of `file`'s language. What a
/// manifest declares is never the standard library, so `$std` knows it too.
pub fn knows_std(file: &str) -> bool {
    manifest_of(file).is_some() || of(file).is_some_and(|language| language.std.is_some())
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

/// How the language of `file` writes a comment; for a file of no language,
/// every way some language writes one.
pub fn comments_of(file: &str) -> &'static Comments {
    static EVERY: LazyLock<Comments> = LazyLock::new(|| {
        let line: BTreeSet<&'static str> = LANGUAGES
            .iter()
            .flat_map(|language| language.comments.line.iter().copied())
            .collect();
        let block: BTreeSet<(&'static str, &'static str)> = LANGUAGES
            .iter()
            .flat_map(|language| language.comments.block.iter().copied())
            .collect();
        // Once, for the life of the process, as every other entry is static.
        Comments {
            line: Vec::from_iter(line).leak(),
            block: Vec::from_iter(block).leak(),
        }
    });
    of(file).map_or(&EVERY, |language| &language.comments)
}

/// Whether `file` lies in a directory that the toolchain of a language checked
/// against `manifest` fills with code not the project's own (Go's `vendor/`): a
/// manifest there is no package of the project.
pub fn is_foreign_manifest(manifest: &Manifest, file: &str) -> bool {
    LANGUAGES
        .iter()
        .filter(|language| language.manifest == Some(manifest))
        .any(|language| language.is_foreign(file))
}

/// The kind of manifest a file is, by its name.
pub fn manifest_of(file: &str) -> Option<&'static Manifest> {
    let name = crate::relpath::file_name(file);
    MANIFESTS
        .into_iter()
        .find(|manifest| manifest.file_name == name)
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
    let manifests: Vec<&str> = MANIFESTS
        .iter()
        .flat_map(|m| m.part_names)
        .copied()
        .collect();
    let mut sources = vec![format!(
        "the manifests `packages` reads ({})",
        quoted(&manifests)
    )];
    sources.extend(
        LANGUAGES
            .iter()
            .filter_map(|language| Some((language.name, language.parts.as_ref()?)))
            .map(|(name, parts)| format!("{name} source ({})", quoted(parts.names))),
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

    /// A `part:` that matches nothing is told where parts come from: the tables of
    /// each kind of manifest, and the parts each language finds in its source.
    #[test]
    fn the_sources_of_parts_are_every_manifest_and_every_language_with_parts() {
        assert_eq!(
            part_sources(),
            "the manifests `packages` reads (`devDependencies`, `feature/<name>`, `dev-dependencies`, `build-dependencies`) and rust source (`cfg(test)`)"
        );
    }

    /// Kinds of manifest are told apart, and ordered, by the name of their file.
    #[test]
    fn a_kind_of_manifest_is_its_file_name() {
        assert!(CARGO_TOML < GO_MOD && GO_MOD < PACKAGE_JSON);
        assert_eq!(MANIFESTS.iter().max(), Some(&&PACKAGE_JSON));
        assert_ne!(GO_MOD, CARGO_TOML);
        assert_eq!(manifest_of("crates/a/Cargo.toml"), Some(&CARGO_TOML));
        assert_eq!(manifest_of("Cargo.lock"), None);
        assert_eq!(format!("{:?}", GO_MOD), "Manifest(\"go.mod\")");
    }

    /// The part a file only test code declares is put in as a whole is one of
    /// the parts its language names, so `part:` can name it.
    #[test]
    fn the_test_part_of_a_language_is_one_of_its_parts() {
        for language in LANGUAGES {
            if let Some(parts) = &language.parts {
                assert!(parts.names.contains(&parts.test), "{}", language.name);
            }
        }
        assert!(LANGUAGES.iter().any(|language| language.parts.is_some()));
    }

    /// Which modules each language takes for its standard library.
    #[test]
    fn each_language_tells_its_standard_library() {
        let mut table = String::new();
        for (file, module) in [
            ("a.go", "net/http"),
            ("a.go", "github.com/x/y"),
            ("a.go", "C"),
            ("a.py", "os.path"),
            ("a.pyi", "os"),
            ("a.py", "requests"),
            ("A.java", "java.util.List"),
            ("A.kt", "kotlin.collections.List"),
            ("a.kts", "java.io.File"),
            ("a.rs", "std::fmt"),
            ("a.rs", "serde::de"),
            ("A.cs", "System.IO"),
            ("a.php", "DateTime"),
            ("a.php", "Symfony\\Component"),
            ("a.swift", "Foundation"),
            ("a.ts", "fs"),
            ("a.rb", "json"),
        ] {
            table.push_str(&format!("{file} {module}: {}\n", is_std(file, module)));
        }
        insta::assert_snapshot!("std_by_language", table);
        assert!(knows_std("a.go") && knows_std("package.json") && !knows_std("a.ts"));
    }

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
            let adapters: Vec<&str> = crate::lang::adapters::build(
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
                knows_std(file),
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
