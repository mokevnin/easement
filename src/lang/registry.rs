//! What easement knows of each language, told by the extensions of its files:
//! data, and hooks for what a rule needs done differently, whose code sits in
//! `lang/<language>.rs`. Adding a language is one entry here, plus an indexer in
//! `indexer::INDEXERS` or an adapter in this module. The rules ask this table,
//! never an extension or a language by name; `easement.yml` holds them to it.

use std::collections::BTreeSet;

use super::parts::{self, Found};
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
pub type UnindexedUses = fn(&str, &str, &BTreeSet<&str>) -> Vec<String>;

/// A setting of an indexer, as the policy writes it: `tags: [race]`.
#[derive(Debug)]
pub struct Setting {
    pub key: &'static str,
    /// What a value has to be, said to whoever wrote another.
    pub value: &'static str,
    pub valid: fn(&str) -> bool,
    /// The variable the indexer reads the values from, and its value.
    pub env: fn(&[String]) -> (&'static str, String),
}

#[derive(Debug)]
pub struct Language {
    pub name: &'static str,
    /// Extensions of its source files.
    pub extensions: &'static [&'static str],
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
    /// Directories its toolchain fills with third-party code (`node_modules`),
    /// which easement never walks.
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
    tests: &["**/*.test.*", "**/*.spec.*", "**/__tests__/**"],
    not_tests: &[],
    whole_directory: false,
    std: None,
    unused: TYPESCRIPT_EXTENSIONS,
    conventional: &["**/*.d.ts"],
    manifest: Some(Manifest::Npm),
    symbols: true,
    parts: None,
    runs_on_import: None,
    uses_unimported: &[],
    program: None,
    unindexed_uses: Some(super::typescript::unindexed_uses),
    third_party: &["node_modules"],
    part_names: &[],
    module_separators: &["/"],
    settings: &[],
};

pub static PYTHON: Language = Language {
    name: "python",
    extensions: &["py", "pyi"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

pub static JAVA: Language = Language {
    name: "java",
    extensions: &["java"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

/// Read by imports alone (an adapter), so `unused` cannot judge it.
pub static KOTLIN: Language = Language {
    name: "kotlin",
    extensions: &["kt", "kts"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

pub static CSHARP: Language = Language {
    name: "csharp",
    extensions: &["cs"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

pub static PHP: Language = Language {
    name: "php",
    extensions: &["php"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["\\"],
    settings: &[],
};

/// Read by imports alone (an adapter), so `unused` cannot judge it.
pub static SWIFT: Language = Language {
    name: "swift",
    extensions: &["swift"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["."],
    settings: &[],
};

/// Ruby loads by convention (autoloading), so `unused` cannot judge it.
pub static RUBY: Language = Language {
    name: "ruby",
    extensions: &["rb"],
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
    third_party: &[],
    part_names: &[],
    module_separators: &["::"],
    settings: &[],
};

pub static RUST: Language = Language {
    name: "rust",
    extensions: &["rs"],
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
) -> Vec<(&'static str, String)> {
    let Some(given) = settings.get(language.name) else {
        return Vec::new();
    };
    language
        .settings
        .iter()
        .filter_map(|setting| {
            let values = given.get(setting.key).filter(|values| !values.is_empty())?;
            Some((setting.env)(values))
        })
        .collect()
}

/// Where the module names a file imports are cut into parents: as its language
/// cuts them, a manifest as the language it serves, and at every language's
/// separators for a file of no language.
pub fn module_separators(file: &str) -> &'static [&'static str] {
    of(file)
        .or_else(|| {
            let manifest = manifest_of(file)?;
            LANGUAGES
                .into_iter()
                .find(|language| language.manifest == Some(manifest))
        })
        .map_or(&["/", ".", "::", "\\"], |language| {
            language.module_separators
        })
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

    /// What each part of easement makes of a file by its name, from the one
    /// registry: the indexer and the adapter that read it, whether `$std` and
    /// `unused` know it, and its manifest.
    #[test]
    fn every_part_tells_a_language_by_its_file_alike() {
        let files = [
            "a.go", "a.py", "a.pyi", "a.ts", "a.d.ts", "a.tsx", "a.mts", "a.cts", "a.js", "a.jsx",
            "a.mjs", "a.cjs", "A.java", "A.kt", "a.kts", "A.cs", "a.php", "a.rb", "a.rs",
            "a.swift", "Makefile", "v1.2/x", "a.json",
        ];
        let mut table = String::new();
        for file in files {
            let covered: Vec<&str> = crate::indexer::INDEXERS
                .iter()
                .filter(|l| l.covers(file))
                .map(|l| l.name())
                .collect();
            let adapters: Vec<&str> = crate::lang::adapter_languages()
                .iter()
                .filter(|l| crate::lang::registry::is(std::path::Path::new(file), l))
                .map(|l| l.name)
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
