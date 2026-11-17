//! Runs the SCIP indexers of the project's languages and reads what they write.
//!
//! easement does not resolve imports itself: an indexer that knows the language's
//! build system (`go.mod`, `tsconfig.json`) does. A missing indexer, a failed run
//! or missing dependencies is an error, never a quiet gap in the check.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::check::{self, Platform};
use crate::lang::registry::{self, Language};
use crate::relpath;
use crate::scip::{self, Document, Documents};

/// The SCIP indexer of a language: how to run it and what its index depends on.
pub struct Indexer {
    /// The language whose files it reads.
    pub language: &'static Language,
    binary: &'static str,
    /// Files that mark a directory the indexer runs in.
    markers: &'static [&'static str],
    /// Run only in the outermost marked directories: a Cargo workspace holds its members.
    outermost: bool,
    /// Run at the project root when no marker is found (Python projects often have none).
    fallback_root: bool,
    /// Arguments that write the index to the given file.
    args: fn(&Path) -> Vec<String>,
    install: &'static str,
    /// The mise tool that installs the indexer, for the `[tools]` of a `mise.toml`.
    mise: Option<&'static str>,
    /// Why it cannot run on Windows, with the upstream cause; `None` when it can.
    /// There easement does not start it: its files are uncovered with this reason.
    /// Removed by hand once the pinned indexer runs there (the Windows CI run shows it).
    not_on_windows: Option<&'static str>,
    /// For an indexer that has no output option: the file it writes in its working directory.
    writes_in_cwd: Option<&'static str>,
    /// The argument that makes it print its version, for `doctor`; `None` for an
    /// indexer that takes any argument for a request to index (scip-php).
    version_flag: Option<&'static str>,
    /// What has to be in place before the indexer runs: (project root, directory, shown name).
    needs: fn(&Path, &Path, &str) -> Result<()>,
    /// Which project files under the index directory the cache key reads (the `/`-separated
    /// path): what the indexer could see. Another language's sources, docs and assets are left
    /// out, so a change to them reuses the index. When in doubt, a file is in.
    inputs: fn(&Language, &str) -> bool,
    /// Which inherited environment variables change what the indexer writes
    /// (`GOFLAGS`, `PYTHONPATH`); their values are in the cache key.
    env: fn(&str) -> bool,
}

/// Go's own variables (`GOFLAGS`, `GOOS`, `GO111MODULE`) have no `_` after `GO`,
/// unlike `GOOGLE_APPLICATION_CREDENTIALS`, which a CI job may point at another
/// file every run; cgo's are `CGO_*`.
fn go_env(name: &str) -> bool {
    name.strip_prefix("GO").is_some_and(|rest| {
        rest.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }) || name.starts_with("CGO_")
}

fn starts_with_any(name: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| name.starts_with(prefix))
}

/// pyright asks the `python3` on the path for its search paths, and scip-python
/// runs `pip` for the installed packages: `PYTHONPATH` changes what an import
/// resolves to. `PYTHONHASHSEED` is left out: tox sets a new one every run. So
/// is `VIRTUAL_ENV`, which alone does not change the index and names the
/// checkout when the environment lives in it, as uv and Poetry put it.
fn python_env(name: &str) -> bool {
    (name.starts_with("PYTHON") && name != "PYTHONHASHSEED") || name.starts_with("PIP_")
}

/// scip-java runs Maven or Gradle with the JDK of `JAVA_HOME`: `MAVEN_ARGS`
/// can skip the build, `ORG_GRADLE_PROJECT_*` sets project properties. Not
/// `JAVA_*` as a prefix: CI runners name every JDK they carry
/// (`JAVA_HOME_17_X64`), used or not.
fn java_env(name: &str) -> bool {
    matches!(
        name,
        "JAVA_HOME"
            | "JAVA_OPTS"
            | "JAVA_TOOL_OPTIONS"
            | "JDK_JAVA_OPTIONS"
            | "_JAVA_OPTIONS"
            | "GRADLE_OPTS"
            | "GRADLE_USER_HOME"
    ) || starts_with_any(name, &["MAVEN_", "ORG_GRADLE_PROJECT_"])
}

/// MSBuild reads every environment variable as a property, by a name of any
/// case (`DefineConstants=X` turns on `#if X`). Keying all of them would miss on
/// every CI run, so the key holds the SDK's own and the properties that decide
/// what is compiled; a project that reads another variable needs `--no-cache`
/// after changing it.
fn csharp_env(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    starts_with_any(&name, &["DOTNET_", "MSBUILD", "NUGET_"])
        || matches!(
            name.as_str(),
            "DEFINECONSTANTS"
                | "CONFIGURATION"
                | "PLATFORM"
                | "TARGETFRAMEWORK"
                | "TARGETFRAMEWORKS"
                | "RUNTIMEIDENTIFIER"
                | "RUNTIMEIDENTIFIERS"
                | "LANGVERSION"
        )
}

/// rust-analyzer runs Cargo and rustc of the toolchain rustup picks: flags,
/// `--cfg`, the toolchain and the standard library's sources change the index.
/// Not the variables Cargo sets for what it runs (`CARGO_PKG_NAME`,
/// `CARGO_MANIFEST_DIR`), which name the checkout and so would keep a
/// worktree from reusing the index.
fn rust_env(name: &str) -> bool {
    const SET_BY_CARGO: &[&str] = &[
        "CARGO_MANIFEST_",
        "CARGO_PKG_",
        "CARGO_CRATE_NAME",
        "CARGO_BIN_",
        "CARGO_PRIMARY_PACKAGE",
        "CARGO_TARGET_TMPDIR",
        "CARGO_MAKEFLAGS",
        "CARGO_CFG_",
        "CARGO_FEATURE_",
    ];
    starts_with_any(name, &["RUSTFLAGS", "RUSTC", "RUSTDOC"])
        || matches!(
            name,
            "CARGO" | "RUSTUP_TOOLCHAIN" | "RUSTUP_HOME" | "RUST_SRC_PATH"
        )
        || (name.starts_with("CARGO_") && !starts_with_any(name, SET_BY_CARGO))
}

fn nothing_needed(_: &Path, _: &Path, _: &str) -> Result<()> {
    Ok(())
}

/// Every file the project owns: for an indexer whose build reads files of its own choosing
/// (Maven, Gradle, Cargo, MSBuild, Bundler, Composer, Python's environment). The directories
/// its toolchain fills with code not the project's own are left out: build output (`target`)
/// is made from the files keyed, and an install (`vendor`) from a lock file.
fn every_own_file(language: &Language, path: &str) -> bool {
    !language.is_foreign(path)
}

/// scip-go reads the Go sources and what `go` resolves them with: the module and workspace
/// files and a vendored dependency list. A vendored dependency is no code of the project, but
/// scip-go type-checks against it, and one a `replace` takes from outside the project changes
/// `vendor/` alone: its files are in.
fn go_inputs(_: &Language, path: &str) -> bool {
    let name = relpath::file_name(path);
    name.ends_with(".go")
        || matches!(
            name,
            "go.mod" | "go.sum" | "go.work" | "go.work.sum" | "modules.txt"
        )
}

/// scip-typescript reads what the TypeScript compiler does: the sources and declarations, every
/// JSON file (a `tsconfig.json` and what it extends, a `package.json`, a JSON module an import
/// names) and the lock files that decide which dependency is installed.
fn typescript_inputs(_: &Language, path: &str) -> bool {
    let name = relpath::file_name(path);
    let source = registry::TYPESCRIPT
        .extensions
        .contains(&relpath::extension(name));
    source
        || name.ends_with(".json")
        || name.ends_with(".json5")
        || matches!(
            name,
            "pnpm-lock.yaml" | "pnpm-workspace.yaml" | "yarn.lock" | "bun.lock" | "bun.lockb"
        )
}

/// Whether an indexer reads `file`, a declaration file (`a.d.ts`) included.
pub fn indexed(file: &str) -> bool {
    INDEXERS.iter().any(|indexer| indexer.reads(file))
}

pub static INDEXERS: &[Indexer] = &[
    Indexer {
        language: &registry::GO,
        binary: "scip-go",
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
        inputs: go_inputs,
        env: go_env,
    },
    Indexer {
        language: &registry::TYPESCRIPT,
        binary: "scip-typescript",
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
        needs: require_installed,
        inputs: typescript_inputs,
        env: |_| false,
    },
    Indexer {
        language: &registry::PYTHON,
        binary: "scip-python",
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
        env: python_env,
    },
    Indexer {
        language: &registry::PHP,
        binary: "scip-php",
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
        needs: require_composer,
        inputs: every_own_file,
        env: |_| false,
    },
    Indexer {
        language: &registry::JAVA,
        binary: "scip-java",
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
        env: java_env,
    },
    Indexer {
        language: &registry::CSHARP,
        binary: "scip-dotnet",
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
        env: csharp_env,
    },
    Indexer {
        language: &registry::RUBY,
        binary: "scip-ruby",
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
    },
    Indexer {
        language: &registry::RUST,
        binary: "rust-analyzer",
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
        env: rust_env,
    },
];

impl Indexer {
    pub fn name(&self) -> &'static str {
        self.language.name
    }

    /// Why it cannot run on `platform`, `None` when it can.
    fn cannot_run_on(&self, platform: Platform) -> Option<&'static str> {
        match platform {
            Platform::Windows => self.not_on_windows,
            Platform::Unix => None,
        }
    }

    /// Why it cannot run on `platform`, as data; `None` when it can.
    fn cannot_run(&self, platform: Platform) -> Option<check::CannotRun> {
        self.cannot_run_on(platform).map(|reason| check::CannotRun {
            language: self.name(),
            indexer: self.binary,
            platform,
            reason,
        })
    }

    /// Whether it reads `file`: a file of its language that is the project's
    /// own, outside the directories its toolchain fills with other code.
    fn reads(&self, file: &str) -> bool {
        self.language.extensions.contains(&relpath::extension(file))
            && !self.language.is_foreign(file)
    }

    /// Whether its index must hold `file`, which is uncovered without it: every
    /// file it reads but a declaration file (`a.d.ts`), which declares the types
    /// of code written elsewhere and may lie outside what it indexes.
    pub fn covers(&self, file: &str) -> bool {
        self.reads(file) && !self.language.declares(file)
    }

    /// A marker is a file name, or `*.ext` for any file with that extension.
    fn has_marker(&self, name: &str) -> bool {
        self.markers
            .iter()
            .any(|marker| match marker.strip_prefix('*') {
                Some(suffix) => name.ends_with(suffix),
                None => *marker == name,
            })
    }

    fn roots(&self, files: &[String]) -> Vec<String> {
        let mut dirs: Vec<String> = files
            .iter()
            .filter(|f| self.has_marker(relpath::file_name(f)) && !self.language.is_foreign(f))
            .map(|f| relpath::parent(f).to_string())
            .collect();
        dirs.sort();
        dirs.dedup();
        if self.outermost {
            let all = dirs.clone();
            dirs.retain(|d| !all.iter().any(|p| p != d && relpath::is_under(d, p)));
        }
        if dirs.is_empty() && self.fallback_root {
            dirs.push(String::new());
        }
        dirs
    }
}

/// `--cache-only` met an index that is not cached: the indexer would have to run.
#[derive(Debug)]
pub struct NotCached {
    pub language: &'static str,
    pub dir: String,
}

impl std::fmt::Display for NotCached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = if self.dir.is_empty() { "." } else { &self.dir };
        write!(
            f,
            "the {} index of {shown} is not cached, and --cache-only runs no indexer",
            self.language
        )
    }
}

impl std::error::Error for NotCached {}

/// How indexes are obtained: run the indexer, or read a cached index first, or only read one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CacheUse {
    Off,
    #[default]
    On,
    Only,
}

impl CacheUse {
    /// How `--no-cache` and `--cache-only` ask for indexes.
    pub fn of_flags(no_cache: bool, cache_only: bool) -> Self {
        if no_cache {
            CacheUse::Off
        } else if cache_only {
            CacheUse::Only
        } else {
            CacheUse::On
        }
    }
}

/// Why the indexer of `language` (a registry name, `python`) cannot run on
/// `platform`; `None` when it can, or when the language has no indexer.
pub fn cannot_index(platform: Platform, language: &str) -> Option<&'static str> {
    INDEXERS
        .iter()
        .find(|indexer| indexer.name() == language)
        .and_then(|indexer| indexer.cannot_run_on(platform))
}

/// The environment the indexers run in, which their cache key reads too: the
/// variables of the process easement runs in, unless the caller sets others,
/// and the platform it runs on, which decides which indexers can run.
#[derive(Clone, Debug)]
pub struct Environment {
    vars: BTreeMap<OsString, OsString>,
    platform: Platform,
}

impl Environment {
    /// The variables and the platform of this process.
    pub fn of_process() -> Self {
        Environment {
            vars: std::env::vars_os().collect(),
            platform: Platform::current(),
        }
    }

    /// Only `vars`, on the platform of this process.
    #[cfg(test)]
    fn of_vars(vars: BTreeMap<OsString, OsString>) -> Self {
        Environment {
            vars,
            platform: Platform::current(),
        }
    }

    /// This environment with `key` set to `value`.
    pub fn with(mut self, key: &str, value: impl Into<OsString>) -> Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    /// This environment as on `platform`: the indexers that cannot run there are not started.
    pub fn on(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// The value of `key`, `None` when it is unset or empty. Windows spells a
    /// name in any case (`Path`).
    pub fn get(&self, key: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .find(|(name, _)| {
                if cfg!(windows) {
                    name.eq_ignore_ascii_case(key)
                } else {
                    name.as_os_str() == key
                }
            })
            .map(|(_, value)| value.as_os_str())
            .filter(|value| !value.is_empty())
    }

    /// The value of `key` as text, `None` when it is unset or empty.
    fn text(&self, key: &str) -> Option<String> {
        self.get(key).map(|v| v.to_string_lossy().into_owned())
    }

    /// The program `name` on the `PATH` of this environment. `which` finds
    /// `scip-typescript.cmd` on Windows, which `Command` alone does not.
    fn which(&self, name: &str, cwd: &Path) -> Option<PathBuf> {
        which::which_in(name, self.get("PATH"), cwd).ok()
    }

    /// A command of `program` that runs with this environment and no other, but
    /// for the variables that tie git to one repository: the program works on the
    /// project, whatever repository a git that runs easement names.
    fn command(&self, program: &Path) -> Command {
        let repository = crate::diff::repository_env();
        let mut command = Command::new(program);
        command.env_clear().envs(
            self.vars
                .iter()
                .filter(|(name, _)| !repository.iter().any(|r| name.as_os_str() == r.as_str())),
        );
        command
    }

    fn vars(&self) -> impl Iterator<Item = (OsString, OsString)> + '_ {
        self.vars.iter().map(|(k, v)| (k.clone(), v.clone()))
    }
}

/// How the indexes are obtained: from the cache in `cache_dir` as `cache`
/// says, by indexers that run in `environment`, `jobs` languages at once.
#[derive(Clone, Debug)]
pub struct Indexing {
    pub cache: CacheUse,
    /// Where the index cache is kept; without one, no index is cached.
    pub cache_dir: Option<PathBuf>,
    pub environment: Environment,
    pub jobs: Jobs,
}

/// What the indexers found, and the source files they did not cover.
pub struct Indexed {
    pub documents: Documents,
    /// Indexers that ran, and indexes read from the cache instead.
    pub ran: usize,
    pub cached: usize,
    /// Source files of an indexed language that no index mentions.
    pub uncovered: Vec<String>,
    /// The languages whose indexer cannot run on this platform, so was not
    /// started: their files are among `uncovered`.
    pub unavailable: Vec<check::Unavailable>,
}

/// The indexers that cannot run on `platform`, each with the files of `files` it would read.
fn unavailable(files: &[String], platform: Platform) -> Vec<check::Unavailable> {
    INDEXERS
        .iter()
        .filter_map(|indexer| {
            let cannot_run = indexer.cannot_run(platform)?;
            let files: Vec<String> = files
                .iter()
                .filter(|f| indexer.covers(f))
                .cloned()
                .collect();
            (!files.is_empty()).then_some(check::Unavailable { cannot_run, files })
        })
        .collect()
}

/// The indexers of the languages with source files among `files` that can run on
/// `platform`, each with the directories it indexes. Source files with no `go.mod`
/// or `tsconfig.json` above them are not indexed; they are reported as uncovered,
/// or can be `exclude`d.
fn groups(files: &[String], platform: Platform) -> Vec<(&'static Indexer, Vec<String>)> {
    INDEXERS
        .iter()
        .filter(|language| language.cannot_run_on(platform).is_none())
        .filter(|language| files.iter().any(|f| language.covers(f)))
        .map(|language| (language, language.roots(files)))
        .filter(|(_, roots)| !roots.is_empty())
        .collect()
}

/// Indexes every language that has source files among `files` (project files,
/// `/`-separated, relative to `root`), each with the `settings` the policy writes
/// for it. `excluded` are files that `exclude` hides: they are not indexed for
/// themselves, but the index resolves imports of them, so they are part of
/// the cache key.
pub fn run(
    root: &Path,
    files: &[String],
    excluded: &[String],
    indexing: &Indexing,
    settings: &registry::IndexerSettings,
) -> Result<Indexed> {
    let (mut ran, mut cached) = (0, 0);
    let keyed = keyed(files, excluded);
    let wanted = INDEXERS
        .iter()
        .flat_map(|language| files.iter().filter(|f| language.covers(f)));
    let platform = indexing.environment.platform;
    let groups = groups(files, platform);
    let results = in_parallel(&groups, indexing.jobs.get(), |(language, roots)| {
        roots
            .iter()
            .map(|dir| index_dir(language, root, dir, &keyed, indexing, settings))
            .collect::<Result<Vec<_>>>()
    })?;
    let mut parts = Vec::new();
    for (documents, from_cache) in results.into_iter().flatten() {
        parts.push(documents);
        if from_cache {
            cached += 1;
        } else {
            ran += 1;
        }
    }
    let documents = if parts.is_empty() {
        Documents::default()
    } else {
        Documents::merged(parts)?
    };
    let indexed: BTreeSet<&str> = documents.0.iter().map(|d| d.path.as_str()).collect();
    let uncovered = wanted
        .filter(|f| !indexed.contains(f.as_str()))
        .cloned()
        .collect();
    Ok(Indexed {
        documents,
        ran,
        cached,
        uncovered,
        unavailable: unavailable(files, platform),
    })
}

/// How many languages are indexed at once (`--jobs`): one by default, since
/// scip-java and rust-analyzer can each take gigabytes. The directories of one
/// language are always indexed one after another: they share its build state
/// (Maven's repository, the lock of cargo's `target`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jobs(std::num::NonZero<usize>);

impl Jobs {
    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for Jobs {
    fn default() -> Self {
        Jobs(std::num::NonZero::<usize>::MIN)
    }
}

impl std::str::FromStr for Jobs {
    type Err = String;

    /// A number of languages, or `cpus` for as many as the machine has processors.
    fn from_str(text: &str) -> Result<Self, String> {
        if text == "cpus" {
            return Ok(Jobs(
                std::thread::available_parallelism().unwrap_or(std::num::NonZero::<usize>::MIN),
            ));
        }
        text.parse()
            .map(Jobs)
            .map_err(|_| format!("`{text}` is neither a number of 1 or more nor `cpus`"))
    }
}

/// Runs `job` on every group, `at_most` of them at once, and returns the results in
/// the order of `groups`, or the failure that a run in order would meet first. After
/// a failure, the groups behind it do not start; the ones before it still finish,
/// since one of them may fail too and come first.
fn in_parallel<T: Sync, R: Send, E: Send>(
    groups: &[T],
    at_most: usize,
    job: impl Fn(&T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let failed_at = AtomicUsize::new(usize::MAX);
    let mut done: Vec<(usize, Result<R, E>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..at_most.min(groups.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(group) = groups.get(i) else { break };
                        if i > failed_at.load(Ordering::SeqCst) {
                            continue;
                        }
                        let result = job(group);
                        if result.is_err() {
                            failed_at.fetch_min(i, Ordering::SeqCst);
                        }
                        done.push((i, result));
                    }
                    done
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    // Every group before the first failure ran: a group is skipped only behind
    // one, so in order the results meet that failure before any gap.
    done.sort_unstable_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, result)| result).collect()
}

/// Where indexes are kept between runs by default: `EASEMENT_CACHE_DIR` of
/// `environment`, else the user's cache directory; `None` when there is neither.
pub fn default_cache_dir(environment: &Environment) -> Option<PathBuf> {
    let var = |name: &str| environment.get(name).map(PathBuf::from);
    let base = var("EASEMENT_CACHE_DIR");
    if base.is_some() {
        return base;
    }
    let user = var("XDG_CACHE_HOME").or_else(|| {
        if cfg!(windows) {
            var("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            var("HOME").map(|home| home.join("Library/Caches"))
        } else {
            var("HOME").map(|home| home.join(".cache"))
        }
    })?;
    Some(user.join("easement"))
}

/// Everything that decides what an indexer would write: the indexer itself, its
/// environment ([`key_env`]) and
/// the name and content of every project file that the indexer reads
/// (`Indexer::inputs`), in every root of the project and not only under `dir`, since
/// a root may build with another (`go.work`, a `replace`, a project reference), plus
/// the files that sit directly in the directories above `dir`
/// (`tsconfig.base.json`, `go.work`, a root `package.json`), which the project
/// files extend. Nothing about the checkout (its path, the modification times)
/// is in the key, so a worktree of the same code reuses the index. Dependencies
/// outside the project are not covered; `--no-cache` is for when they change
/// without a lock file.
fn fingerprint(
    language: &Indexer,
    binary: &Path,
    root: &Path,
    dir: &str,
    files: &[String],
    env: &[(String, String)],
    environment: &Environment,
) -> Key {
    let mut hasher = StableHasher::default();
    language.name().hash(&mut hasher);
    env.hash(&mut hasher);
    (language.args)(Path::new("")).hash(&mut hasher);
    let cwd = root.join(dir);
    what_runs(
        binary,
        language.binary,
        &cwd,
        &multiplexers(environment, &cwd),
    )
    .hash(&mut hasher);
    let indexer = hasher.finish();
    let mut hasher = StableHasher::default();
    let above = |f: &str| {
        let parent = relpath::parent(f);
        parent.len() < dir.len() && relpath::is_under(dir, parent)
    };
    let mut seen: Vec<(&str, Option<u64>)> = files
        .iter()
        .filter(|f| (language.inputs)(language.language, f) || above(f))
        .map(|f| {
            // An unreadable file is a different state from every readable one.
            let content = std::fs::read(root.join(f))
                .ok()
                .map(|bytes| content_hash(&bytes));
            (f.as_str(), content)
        })
        .collect();
    seen.sort();
    seen.hash(&mut hasher);
    Key {
        indexer,
        files: hasher.finish(),
    }
}

/// The cache key of an index, in two parts, both in the name of its entry, so that
/// a miss can say which changed: the indexer (its build, arguments and environment)
/// or the files it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    indexer: u64,
    files: u64,
}

/// The environment an indexer runs with, as far as it changes the index: the
/// inherited variables that `Language::env` names, with what easement sets for
/// the run over them.
fn key_env(
    language: &Indexer,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    set: &[(&str, String)],
) -> Vec<(String, String)> {
    let mut env: BTreeMap<String, String> = inherited
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .filter(|(k, _)| (language.env)(k))
        .collect();
    env.extend(set.iter().map(|(k, v)| (k.to_string(), v.clone())));
    env.into_iter().collect()
}

/// What tells one build of an indexer from another: where it really is (symbolic links
/// resolved, so a version manager's install directory names the version) and a hash of its
/// content. Not when it was installed: a CI runner that restores the toolchain from a cache gets
/// the same files with other modification times, and must still find the indexes. The indexer is
/// not run for a version: some (scip-php) take any argument as a request to index. A binary
/// that cannot be read is told apart by its modification time, which only costs reuse.
fn indexer_identity(
    binary: &Path,
) -> (Option<PathBuf>, Option<u64>, Option<std::time::SystemTime>) {
    type Seen =
        std::collections::HashMap<(PathBuf, Option<std::time::SystemTime>, u64), Option<u64>>;
    static CONTENT: std::sync::OnceLock<std::sync::Mutex<Seen>> = std::sync::OnceLock::new();
    let real = std::fs::canonicalize(binary).ok();
    let path = real.clone().unwrap_or_else(|| binary.to_path_buf());
    let meta = std::fs::metadata(&path).ok();
    let modified = meta.as_ref().and_then(|m| m.modified().ok());
    // Hashed once per process while the file stays as it is; an indexer upgraded in
    // place during `--watch` has another time or size, and is hashed again.
    let seen = (path.clone(), modified, meta.map_or(0, |m| m.len()));
    let hash = |path: &Path| std::fs::read(path).ok().map(|bytes| content_hash(&bytes));
    let content = CONTENT
        .get_or_init(Default::default)
        .lock()
        .map(|mut known| *known.entry(seen).or_insert_with(|| hash(&path)))
        .unwrap_or_else(|_| hash(&path));
    (
        real,
        content,
        if content.is_some() { None } else { modified },
    )
}

/// What the key holds of an indexer: the binary it is, or, for a multiplexer's
/// proxy, the binary the multiplexer runs.
#[derive(Debug, PartialEq, Hash)]
enum Runs {
    Binary((Option<PathBuf>, Option<u64>, Option<std::time::SystemTime>)),
    /// A proxy whose multiplexer could not tell what it runs here: never the
    /// proxy's own identity, which every toolchain shares.
    Unresolved(Option<PathBuf>),
}

/// A program that runs another one by the name it is called with. rustup's
/// proxies (`~/.cargo/bin/rust-analyzer`) and mise's shims are one file for
/// every toolchain, so what tells two indexers apart is the binary each runs
/// from the index directory, where `rust-toolchain.toml` or `mise.toml`
/// choose it.
struct Multiplexer {
    path: PathBuf,
    resolve: Box<Resolve>,
}

/// The binary a multiplexer runs for `name` in a directory, `None` when it cannot tell.
type Resolve = dyn Fn(&str, &Path) -> Option<PathBuf>;

/// The multiplexers on the path: rustup and mise, both of which answer `which <name>`.
fn multiplexers(environment: &Environment, cwd: &Path) -> Vec<Multiplexer> {
    ["rustup", "mise"]
        .into_iter()
        .filter_map(|tool| {
            let path = environment.which(tool, cwd)?;
            let program = path.clone();
            let environment = environment.clone();
            let resolve = move |name: &str, cwd: &Path| {
                let output = environment
                    .command(&program)
                    .args(["which", name])
                    .current_dir(cwd)
                    // A toolchain that is not installed is an answer, not a download.
                    .env("RUSTUP_AUTO_INSTALL", "0")
                    .output()
                    .ok()?;
                let found = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
                (output.status.success() && found.is_file()).then_some(found)
            };
            Some(Multiplexer {
                path,
                resolve: Box::new(resolve),
            })
        })
        .collect()
}

/// What runs when `binary` (installed as `name`) runs in `cwd`. A file with the
/// content of a multiplexer is its proxy, whether a symbolic link, a hard link
/// or a copy.
fn what_runs(binary: &Path, name: &str, cwd: &Path, multiplexers: &[Multiplexer]) -> Runs {
    let own = indexer_identity(binary);
    let size = |path: &Path| std::fs::metadata(path).map(|m| m.len()).ok();
    let proxy = multiplexers.iter().find(|m| {
        own.1.is_some() && size(&m.path) == size(binary) && indexer_identity(&m.path).1 == own.1
    });
    match proxy {
        None => Runs::Binary(own),
        Some(multiplexer) => match (multiplexer.resolve)(name, cwd) {
            Some(real) => Runs::Binary(indexer_identity(&real)),
            None => Runs::Unresolved(own.0),
        },
    }
}

/// A hasher for the keys of the index cache, which outlives the build of easement that
/// wrote it: `DefaultHasher` may change its algorithm in any Rust release, BLAKE3 does not.
/// The bytes still come from std's `Hash`, which Rust does not promise to keep either; a
/// change there only makes the cache miss once.
#[derive(Default)]
struct StableHasher(blake3::Hasher);

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(&self) -> u64 {
        first_word(self.0.finalize())
    }
}

/// The BLAKE3 hash of a file's content, cut to 64 bits.
fn content_hash(bytes: &[u8]) -> u64 {
    first_word(blake3::hash(bytes))
}

fn first_word(hash: blake3::Hash) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_le_bytes(word)
}

/// Where the index of one directory is kept in the cache for the project as it is now.
struct Slot {
    /// The name of the entry; it carries the key, so a changed project misses it.
    name: String,
    /// What the names of the entries of the same directory and indexer begin with:
    /// they are pruned to the most recently used few.
    prefix: String,
    /// What the names of those entries that the same indexer wrote begin with.
    indexer_prefix: String,
}

/// The slot of the index of `dir` in the cache, keyed on everything the indexer at
/// `binary` reads ([`fingerprint`]): what `check` reads and writes, and what
/// `doctor --cache` looks for, so the two cannot drift.
fn slot(
    language: &Indexer,
    binary: &Path,
    root: &Path,
    dir: &str,
    files: &[String],
    env: &[(&str, String)],
    environment: &Environment,
) -> Slot {
    let key = fingerprint(
        language,
        binary,
        root,
        dir,
        files,
        &key_env(language, environment.vars(), env),
        environment,
    );
    let mut hasher = StableHasher::default();
    (language.name(), dir).hash(&mut hasher);
    let prefix = format!("{:016x}-", hasher.finish());
    let indexer_prefix = format!("{prefix}{:016x}-", key.indexer);
    Slot {
        name: format!("{indexer_prefix}{:016x}.scip", key.files),
        prefix,
        indexer_prefix,
    }
}

/// Whether the index of one directory is in the cache for the project as it is now.
#[derive(Debug, serde::Serialize)]
pub struct Probe {
    pub language: &'static str,
    /// The directory indexed, relative to the root; `.` for the root.
    pub dir: String,
    pub cached: bool,
    /// Why the indexer would run, when it would.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Miss>,
}

/// Why an index is not in the cache, so that a run would index again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Miss {
    /// No cache directory: neither `EASEMENT_CACHE_DIR` nor a user cache directory.
    NoCacheDir,
    /// The indexer is not on the `PATH`, so its key, its build, cannot be known.
    NotInstalled,
    /// The cache holds no index of this directory by this indexer.
    NoEntry,
    /// The cache holds one by the same indexer, of other files.
    FilesChanged,
    /// The cache holds one by another build of the indexer, or with other
    /// arguments or environment variables.
    IndexerChanged,
}

impl Probe {
    fn new(language: &Indexer, dir: String, reason: Option<Miss>) -> Self {
        Probe {
            language: language.name(),
            dir: if dir.is_empty() { ".".to_string() } else { dir },
            // Said apart in the JSON, for a reader that only asks whether it is.
            cached: reason.is_none(),
            reason,
        }
    }
}

impl Miss {
    pub fn text(self) -> &'static str {
        match self {
            Miss::NoCacheDir => "no cache directory",
            Miss::NotInstalled => "the indexer is not installed",
            Miss::NoEntry => "no cached index of this directory",
            Miss::FilesChanged => "the files it reads changed",
            Miss::IndexerChanged => "the indexer, its arguments or its environment changed",
        }
    }
}

/// For every directory `run` would index: is its index in the cache under the key
/// `run` would compute? It reads no index and runs no indexer; an entry that is
/// there but cannot be read counts as cached, and `run` indexes it again.
pub fn probe(
    root: &Path,
    files: &[String],
    excluded: &[String],
    indexing: &Indexing,
    settings: &registry::IndexerSettings,
) -> Vec<Probe> {
    let environment = &indexing.environment;
    let keyed = keyed(files, excluded);
    let names: Vec<String> = indexing
        .cache_dir
        .as_deref()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".scip"))
        .collect();
    let mut probes = Vec::new();
    for (language, roots) in groups(files, environment.platform) {
        let env = settings_env(language, settings, environment);
        for dir in roots {
            let reason = match (
                indexing.cache_dir.as_deref(),
                environment.which(language.binary, &cwd(root, &dir)),
            ) {
                (None, _) => Some(Miss::NoCacheDir),
                (_, None) => Some(Miss::NotInstalled),
                (Some(cache_dir), Some(binary)) => {
                    let slot = slot(language, &binary, root, &dir, &keyed, &env, environment);
                    let any = |prefix: &str| names.iter().any(|n| n.starts_with(prefix));
                    if cache_dir.join(&slot.name).is_file() {
                        None
                    } else if any(&slot.indexer_prefix) {
                        Some(Miss::FilesChanged)
                    } else if any(&slot.prefix) {
                        Some(Miss::IndexerChanged)
                    } else {
                        Some(Miss::NoEntry)
                    }
                }
            };
            probes.push(Probe::new(language, dir, reason));
        }
    }
    probes
}

/// Whether the indexer of one language can run in a project.
#[derive(Debug, serde::Serialize)]
pub struct Diagnosis {
    pub language: &'static str,
    pub binary: &'static str,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// How to install it; `None` where it cannot run, which no install changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mise: Option<&'static str>,
    /// What is missing around the indexer: dependencies that are not installed.
    pub problems: Vec<String>,
    /// Why it cannot run on this platform, where it cannot: easement does not
    /// start it, and the files of its language are uncovered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cannot_run: Option<check::CannotRun>,
}

impl Diagnosis {
    /// Whether the indexer can run, or cannot run on this platform at all: then
    /// nothing is missing, and a check reports its files as uncovered.
    pub fn ok(&self) -> bool {
        self.cannot_run.is_some() || (self.found && self.problems.is_empty())
    }
}

/// For every language with source files among `files`: is its indexer on the
/// `PATH`, which version, and is what it needs in place.
pub fn diagnose(root: &Path, files: &[String], environment: &Environment) -> Vec<Diagnosis> {
    INDEXERS
        .iter()
        .filter(|language| files.iter().any(|f| language.covers(f)))
        .map(|language| {
            // Where it cannot run, it is not started, not even for its version,
            // and what it would need is beside the point.
            if let Some(cannot_run) = language.cannot_run(environment.platform) {
                return Diagnosis {
                    language: language.name(),
                    binary: language.binary,
                    found: false,
                    path: None,
                    version: None,
                    install: None,
                    mise: None,
                    problems: Vec::new(),
                    cannot_run: Some(cannot_run),
                };
            }
            let binary = environment.which(language.binary, root);
            let version = binary
                .as_deref()
                .zip(language.version_flag)
                .and_then(|(binary, flag)| version_of(binary, flag, environment));
            let problems = language
                .roots(files)
                .iter()
                .filter_map(|dir| {
                    let shown = if dir.is_empty() { "." } else { dir };
                    (language.needs)(root, &root.join(dir), shown)
                        .err()
                        .map(|e| e.to_string())
                })
                .collect();
            Diagnosis {
                language: language.name(),
                binary: language.binary,
                found: binary.is_some(),
                path: binary.map(|p| p.display().to_string()),
                version,
                install: Some(language.install),
                mise: language.mise,
                problems,
                cannot_run: None,
            }
        })
        .collect()
}

/// The first line a tool prints for `flag`.
fn version_of(binary: &Path, flag: &str, environment: &Environment) -> Option<String> {
    let output = environment.command(binary).arg(flag).output().ok()?;
    [output.stdout, output.stderr].iter().find_map(|bytes| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    })
}

/// The files whose content keys an index: the project files, and the excluded
/// ones, which the index resolves imports of.
fn keyed(files: &[String], excluded: &[String]) -> Vec<String> {
    files.iter().chain(excluded).cloned().collect()
}

/// The variables easement sets for a run of the indexer, from the policy's `settings`.
fn settings_env(
    language: &Indexer,
    settings: &registry::IndexerSettings,
    environment: &Environment,
) -> Vec<(&'static str, String)> {
    registry::settings_env(language.language, settings, &|key| environment.text(key))
}

/// The directory the indexer of `dir` runs in.
fn cwd(root: &Path, dir: &str) -> PathBuf {
    if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    }
}

fn index_dir(
    language: &Indexer,
    root: &Path,
    dir: &str,
    files: &[String],
    indexing: &Indexing,
    settings: &registry::IndexerSettings,
) -> Result<(Vec<Document>, bool)> {
    let environment = &indexing.environment;
    let env = settings_env(language, settings, environment);
    let cwd = cwd(root, dir);
    let shown = if dir.is_empty() { "." } else { dir };
    let binary = environment.which(language.binary, &cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "`{}` is not installed, and easement needs it for {} files; install it with `{}`",
            language.binary,
            language.name(),
            language.install
        )
    })?;
    // The key reads every input of the indexer: only a run that uses the cache needs it.
    let cached = indexing
        .cache_dir
        .as_deref()
        .filter(|_| indexing.cache != CacheUse::Off)
        .map(|cache_dir| {
            let slot = slot(language, &binary, root, dir, files, &env, environment);
            (cache_dir.join(slot.name), slot.prefix)
        });
    if let Some((path, _)) = &cached
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(documents) = scip::parse(&bytes, root, Path::new(dir))
        && !documents.is_empty()
    {
        // Recency decides what pruning keeps: best effort, a failure only ages the entry.
        if let Ok(file) = std::fs::File::options().write(true).open(path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
        return Ok((documents, true));
    }
    if indexing.cache == CacheUse::Only {
        return Err(NotCached {
            language: language.name(),
            dir: dir.to_string(),
        }
        .into());
    }
    // Checked only when the indexer runs: a cached index was written with them in place, and
    // what they are is in its key (a lock file).
    (language.needs)(root, &cwd, shown)?;
    let out = tempfile::tempdir().context("cannot create a temporary directory")?;
    let file = match language.writes_in_cwd {
        Some(name) => {
            let written = cwd.join(name);
            if written.exists() {
                bail!(
                    "{} would overwrite {}, which is in the way; move it",
                    language.binary,
                    written.display()
                );
            }
            written
        }
        None => out.path().join("index.scip"),
    };
    // What the indexer writes in the project goes when this run ends, failed or not:
    // left there, it would be in the way of every run after it.
    let _written = language.writes_in_cwd.map(|_| RemoveOnDrop(file.clone()));
    let output = environment
        .command(&binary)
        .args((language.args)(&file))
        .envs(env.iter().map(|(k, v)| (*k, v)))
        .current_dir(&cwd)
        .output()
        .with_context(|| format!("cannot run {}", language.binary))?;
    if !output.status.success() {
        // Some indexers report on stdout (Maven), others on stderr.
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        // The first lines name the error, the last ones the place; keep both.
        let excerpt = if lines.len() > 14 {
            [&lines[..5], &["    …"][..], &lines[lines.len() - 8..]].concat()
        } else {
            lines
        };
        let hint = if text.contains("LAUNCHER_ARGS[@]: unbound variable") {
            "\nscip-java's wrapper needs bash 4.4 or newer; the bash of macOS is 3.2, put a newer one first on the PATH (`brew install bash`)"
        } else {
            ""
        };
        bail!(
            "{} failed in {shown} ({}); if that directory is not part of the project, add it to `exclude`:\n{}{hint}",
            language.binary,
            output.status,
            excerpt.join("\n")
        );
    }
    let bytes = std::fs::read(&file)
        .with_context(|| format!("{} wrote no index in {shown}", language.binary))?;
    let documents = scip::parse(&bytes, root, Path::new(dir))
        .with_context(|| format!("cannot read the index of {} in {shown}", language.binary))?;
    if documents.is_empty() {
        bail!(
            "{} indexed no files in {shown}; check what its project file includes",
            language.binary
        );
    }
    if let Some((path, prefix)) = cached
        && let Some(parent) = path.parent()
    {
        // Best effort: a cache that cannot be written only costs the next run its speed.
        let _ = std::fs::create_dir_all(parent);
        // Written to a temporary file first, so that a run in another checkout never
        // reads half an index: a cut at a message boundary would parse as fewer files.
        let _ = tempfile::NamedTempFile::new_in(parent).and_then(|mut file| {
            std::io::Write::write_all(&mut file, &bytes)?;
            file.persist(&path).map_err(|e| e.error)
        });
        prune(parent, &prefix);
        trim(parent, &path, CACHE_BUDGET);
    }
    Ok((documents, false))
}

/// A file removed when this goes out of scope, as far as it can be.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Entries of one directory and indexer that are kept: checkouts of the same
/// project share them, so a new entry must not remove those of the others.
const KEPT_PER_DIR: usize = 32;

/// Removes all but the `KEPT_PER_DIR` most recently used entries with `prefix`.
fn prune(parent: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let mut mine: Vec<_> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    mine.sort_by(|a, b| b.cmp(a));
    for (_, path) in mine.into_iter().skip(KEPT_PER_DIR) {
        let _ = std::fs::remove_file(path);
    }
}

/// What the whole cache may take. `KEPT_PER_DIR` counts entries, not bytes, and
/// only for the directory just indexed: an index of a big project weighs hundreds
/// of megabytes, and every project and worktree adds 32 of its own, so without a
/// budget the cache grew past 14 GB.
const CACHE_BUDGET: u64 = 4 << 30;

/// Removes the least recently used indexes, of any directory and indexer, until
/// the cache fits in `budget`. `written`, the entry just made, stays even when it
/// alone is over: removing it would only make the next run index again.
fn trim(parent: &Path, written: &Path, budget: u64) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let mut all: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        // Only finished entries: a temporary file is another run's index in the writing.
        .filter(|path| path.extension().is_some_and(|ext| ext == "scip"))
        .filter_map(|path| {
            let meta = path.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), path))
        })
        .collect();
    all.sort_by(|a, b| b.cmp(a));
    let mut used: u64 = 0;
    for (_, len, path) in all {
        used = used.saturating_add(len);
        if used > budget && path != written {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `scip-php` reads the dependencies' code from `vendor`, through the autoloader.
fn require_composer(_root: &Path, dir: &Path, shown: &str) -> Result<()> {
    if !dir.join("composer.lock").is_file() || !dir.join("vendor/autoload.php").is_file() {
        bail!(
            "dependencies are not installed in {shown}: run `composer install` (scip-php needs composer.lock and vendor/)"
        );
    }
    Ok(())
}

/// Without `node_modules` the TypeScript indexer cannot see third-party
/// modules, so external dependencies would silently disappear from the check.
fn require_installed(root: &Path, dir: &Path, shown: &str) -> Result<()> {
    let manifest = dir.join("package.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Ok(());
    };
    let declares = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .is_some_and(|json| {
            ["dependencies", "devDependencies"].iter().any(|key| {
                json.get(key)
                    .and_then(|v| v.as_object())
                    .is_some_and(|o| !o.is_empty())
            })
        });
    if !declares {
        return Ok(());
    }
    let mut at: Option<PathBuf> = Some(dir.to_path_buf());
    while let Some(current) = at {
        if current.join("node_modules").is_dir() {
            return Ok(());
        }
        at = (current != root)
            .then(|| current.parent().map(Path::to_path_buf))
            .flatten();
    }
    bail!(
        "dependencies are not installed in {shown}: run `npm install` (or pnpm, yarn) first; `easement doctor` lists everything else that is missing"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn jobs_are_a_number_or_the_cpus() {
        assert_eq!(Jobs::default().get(), 1);
        assert_eq!("3".parse::<Jobs>().unwrap().get(), 3);
        let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        assert_eq!("cpus".parse::<Jobs>().unwrap().get(), cpus);
        assert!("0".parse::<Jobs>().is_err());
        assert!("many".parse::<Jobs>().is_err());
    }

    #[test]
    fn groups_run_at_most_so_many_at_once_and_answer_in_their_order() {
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let groups = [30, 0, 0, 0, 0];
        let results = in_parallel(&groups, 2, |&ms| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            // Wait for a second group to run, however late its thread starts; a pool
            // that runs one group at a time gives up here and fails the test.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while most.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            // The first group finishes last.
            std::thread::sleep(Duration::from_millis(ms));
            running.fetch_sub(1, Ordering::SeqCst);
            Ok::<_, ()>(ms)
        });
        assert_eq!(results, Ok(groups.to_vec()));
        assert_eq!(most.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_failure_reported_is_the_first_in_order() {
        // The second group fails first, but a run in order meets the first one's failure.
        let groups = [(30, Err("first")), (0, Err("second")), (0, Ok(()))];
        let results = in_parallel(&groups, 3, |(ms, result)| {
            std::thread::sleep(Duration::from_millis(*ms));
            *result
        });
        assert_eq!(results, Err("first"));
    }

    #[test]
    fn after_a_failure_the_groups_behind_it_do_not_start() {
        let started = AtomicUsize::new(0);
        // Set by the second group just before it fails.
        let failing = (std::sync::Mutex::new(false), std::sync::Condvar::new());
        // The first group still runs to its end: it comes before the failure. It
        // holds its worker until well after the failure, so that the other worker
        // is the one that meets the groups behind it. No sleep stands between the
        // failure and the moment it is recorded (Windows can sleep a whole tick).
        let groups = [Ok(()), Err("second"), Ok(()), Ok(())];
        let indexed: Vec<(usize, Result<(), &str>)> = groups.into_iter().enumerate().collect();
        let results = in_parallel(&indexed, 2, |(i, result)| {
            started.fetch_add(1, Ordering::SeqCst);
            let (flag, signal) = &failing;
            match i {
                0 => {
                    let guard = flag.lock().unwrap();
                    let (_failed, timeout) = signal
                        .wait_timeout_while(guard, Duration::from_secs(10), |failed| !*failed)
                        .unwrap();
                    assert!(!timeout.timed_out(), "the groups did not run at once");
                    std::thread::sleep(Duration::from_millis(50));
                }
                1 => {
                    *flag.lock().unwrap() = true;
                    signal.notify_all();
                }
                _ => {}
            }
            *result
        });
        assert_eq!(results, Err("second"));
        assert_eq!(started.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn each_indexer_keys_the_variables_that_change_its_index() {
        let keys = |language: &str, name: &str| {
            let indexer = INDEXERS.iter().find(|i| i.name() == language).unwrap();
            (indexer.env)(name)
        };
        for (language, name, keyed) in [
            ("go", "GOFLAGS", true),
            ("go", "GOOS", true),
            ("go", "CGO_ENABLED", true),
            ("go", "GOOGLE_APPLICATION_CREDENTIALS", false),
            ("go", "GOROOT_1_22_X64", false),
            ("typescript", "NODE_PATH", false),
            ("python", "PYTHONPATH", true),
            ("python", "PIP_USER", true),
            ("python", "VIRTUAL_ENV", false),
            ("python", "PYTHONHASHSEED", false),
            ("php", "COMPOSER_VENDOR_DIR", false),
            ("java", "JAVA_HOME", true),
            ("java", "MAVEN_ARGS", true),
            ("java", "ORG_GRADLE_PROJECT_flavor", true),
            ("java", "GRADLE_USER_HOME", true),
            ("java", "JAVA_HOME_17_X64", false),
            ("csharp", "DefineConstants", true),
            ("csharp", "MSBuildSDKsPath", true),
            ("csharp", "DOTNET_ROOT", true),
            ("csharp", "GITHUB_RUN_ID", false),
            ("ruby", "BUNDLE_GEMFILE", false),
            ("rust", "RUSTFLAGS", true),
            ("rust", "CARGO_ENCODED_RUSTFLAGS", true),
            ("rust", "CARGO_BUILD_RUSTFLAGS", true),
            ("rust", "RUSTUP_TOOLCHAIN", true),
            ("rust", "RUST_SRC_PATH", true),
            ("rust", "CARGO", true),
            ("rust", "CARGO_PKG_NAME", false),
            ("rust", "CARGO_MANIFEST_DIR", false),
        ] {
            assert_eq!(keys(language, name), keyed, "{language} {name}");
        }
    }

    /// A hook or `git rebase --exec` that runs easement points git at its own
    /// repository; an indexer, which may run git (`go` stamps the build with it),
    /// works on the project and inherits none of that.
    #[test]
    fn an_indexer_runs_without_the_repository_a_parent_git_names() {
        let environment = Environment::of_vars(
            [
                ("GIT_DIR", "/elsewhere/.git"),
                ("GIT_INDEX_FILE", "/elsewhere/.git/index"),
                ("HOME", "/home/a"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        );
        let command = environment.command(Path::new("scip-x"));
        let names: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_some())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["HOME"]);
    }

    #[test]
    fn the_cache_key_reads_the_environment_the_indexer_inherits() {
        let go = &INDEXERS[0];
        let ts = &INDEXERS[1];
        let inherited = || {
            [
                ("GOFLAGS", "-tags=race"),
                ("HOME", "/home/a"),
                ("CGO_ENABLED", "0"),
                // Not Go's: a CI job may point it at another file every run.
                ("GOOGLE_APPLICATION_CREDENTIALS", "/tmp/gha-creds-1.json"),
            ]
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        };
        assert_eq!(
            key_env(go, inherited(), &[]),
            [("CGO_ENABLED", "0"), ("GOFLAGS", "-tags=race")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        // What easement sets for the run wins over what it inherits.
        assert_eq!(
            key_env(go, inherited(), &[("GOFLAGS", "-tags=x".to_string())])[1],
            ("GOFLAGS".to_string(), "-tags=x".to_string())
        );
        assert!(key_env(ts, inherited(), &[]).is_empty());
    }

    #[test]
    fn a_proxy_is_known_by_the_binary_its_multiplexer_runs() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, content).unwrap();
            path
        };
        let mux = write("mux", "the multiplexer");
        // A proxy is the multiplexer's file under the indexer's name (a copy on Windows).
        let proxy = write("scip-x", "the multiplexer");
        let stable = write("scip-x-stable", "build 1");
        let nightly = write("scip-x-nightly", "build 2");
        let runs = |real: Option<PathBuf>| {
            let multiplexers = [Multiplexer {
                path: mux.clone(),
                resolve: Box::new(move |name, _| {
                    assert_eq!(name, "scip-x");
                    real.clone()
                }),
            }];
            what_runs(&proxy, "scip-x", dir.path(), &multiplexers)
        };
        let on_stable = runs(Some(stable.clone()));
        assert_eq!(on_stable, Runs::Binary(indexer_identity(&stable)));
        assert_ne!(on_stable, runs(Some(nightly)));
        // Unresolved is neither a toolchain's binary nor the proxy shared by all.
        let unresolved = runs(None);
        assert!(matches!(unresolved, Runs::Unresolved(_)));
        assert_ne!(unresolved, Runs::Binary(indexer_identity(&proxy)));
        // A binary of its own is itself.
        assert_eq!(
            what_runs(&stable, "scip-x", dir.path(), &[]),
            Runs::Binary(indexer_identity(&stable))
        );
    }

    #[test]
    fn an_indexer_replaced_in_place_has_another_identity() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("scip-x");
        std::fs::write(&binary, b"version 1").unwrap();
        let before = indexer_identity(&binary);
        assert_eq!(before, indexer_identity(&binary));
        std::fs::write(&binary, b"version 2, longer").unwrap();
        assert_ne!(before.1, indexer_identity(&binary).1);
    }

    #[test]
    fn cache_keys_hash_with_blake3_which_no_build_of_easement_changes() {
        // The BLAKE3 test vector: the hash of `abc` begins 64 37 b3 ac 38 46 51 33.
        assert_eq!(content_hash(b"abc"), 0x3351_4638_acb3_3764);
        let mut hasher = StableHasher::default();
        hasher.write(b"abc");
        assert_eq!(hasher.finish(), 0x3351_4638_acb3_3764);
    }

    #[test]
    fn languages_cover_their_sources_and_roots() {
        let ts = &INDEXERS[1];
        assert!(ts.covers("src/a.tsx") && ts.covers("a.mjs"));
        assert!(!ts.covers("src/a.d.ts") && !ts.covers("main.go"));
        assert!(!ts.covers("src/a.d.mts") && !ts.covers("src/a.d.cts"));
        assert!(indexed("src/a.d.ts") && indexed("src/a.d.mts"));
        let files: Vec<String> = [
            "go.mod",
            "svc/go.mod",
            "web/tsconfig.json",
            "a.go",
            "Cargo.toml",
            "crates/x/Cargo.toml",
            "tools/Cargo.toml",
            "app.py",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(INDEXERS[0].roots(&files), ["", "svc"]);
        assert_eq!(ts.roots(&files), ["web"]);
        // A workspace root holds its members; Python falls back to the root.
        assert_eq!(INDEXERS[7].roots(&files), [""]);
        assert_eq!(INDEXERS[2].roots(&files), [""]);
        let nested: Vec<String> = ["a/Cargo.toml", "b/Cargo.toml", "b/c/Cargo.toml"]
            .map(String::from)
            .to_vec();
        assert_eq!(INDEXERS[7].roots(&nested), ["a", "b"]);
    }

    /// The go tool ignores `testdata/` and a committed `vendor/`: a module in one
    /// is no part of the project, and its indexer does not run on it.
    #[test]
    fn a_marker_in_a_directory_of_code_not_the_projects_own_is_no_root() {
        let files: Vec<String> = [
            "go.mod",
            "internal/a/testdata/go.mod",
            "internal/a/testdata/x.go",
            "vendor/example.com/dep/go.mod",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(INDEXERS[0].roots(&files), [""]);
    }

    /// scip-php runs in every directory with a `composer.json`, not only the
    /// outermost: an installed package in a committed `vendor/` is no project of its own.
    #[test]
    fn an_installed_php_package_is_no_root() {
        let php = INDEXERS.iter().find(|i| i.name() == "php").unwrap();
        let files: Vec<String> = [
            "composer.json",
            "src/A.php",
            "vendor/acme/x/composer.json",
            "packages/b/composer.json",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(php.roots(&files), ["", "packages/b"]);
    }

    #[test]
    fn the_cache_key_of_go_and_typescript_reads_only_what_their_indexer_does() {
        for path in [
            "a.go",
            "svc/x_test.go",
            "go.mod",
            "go.sum",
            "go.work",
            "vendor/modules.txt",
        ] {
            assert!(go_inputs(&registry::GO, path), "{path}");
        }
        for path in [
            "web/a.tsx",
            "README.md",
            "locales/en.json",
            "pnpm-lock.yaml",
            "x.goal",
        ] {
            assert!(!go_inputs(&registry::GO, path), "{path}");
        }
        for path in [
            "src/a.tsx",
            "types/x.d.ts",
            "lib/a.mjs",
            "tsconfig.json",
            "tsconfig.app.json",
            "package.json",
            "locales/en.json",
            "pnpm-lock.yaml",
            "yarn.lock",
        ] {
            assert!(typescript_inputs(&registry::TYPESCRIPT, path), "{path}");
        }
        for path in ["main.go", "go.mod", "README.md", "style.css", "logo.svg"] {
            assert!(!typescript_inputs(&registry::TYPESCRIPT, path), "{path}");
        }
    }

    #[test]
    fn an_indexer_is_told_apart_by_its_content_not_by_when_it_was_installed() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("scip-go");
        std::fs::write(&first, "build 1").unwrap();
        let identity = indexer_identity(&first);
        assert!(identity.1.is_some() && identity.2.is_none(), "{identity:?}");
        // The same build restored from a cache: another modification time, the same identity.
        let file = std::fs::File::options().write(true).open(&first).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
        assert_eq!(indexer_identity(&first), identity);
        // Another build is another indexer.
        let second = dir.path().join("other");
        std::fs::write(&second, "build 2").unwrap();
        assert_ne!(indexer_identity(&second).1, identity.1);
    }

    #[test]
    fn missing_node_modules_is_an_error_only_when_dependencies_are_declared() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        require_installed(root, root, ".").unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"x"}"#).unwrap();
        require_installed(root, root, ".").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"lodash":"1"}}"#,
        )
        .unwrap();
        let err = require_installed(root, root, ".").unwrap_err();
        assert!(err.to_string().contains("not installed"));
        assert!(err.to_string().contains("easement doctor"));
        std::fs::create_dir(root.join("node_modules")).unwrap();
        require_installed(root, root, ".").unwrap();
    }

    #[test]
    fn pruning_keeps_the_most_recently_used_entries_of_one_directory_only() {
        let dir = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now();
        let write = |name: String, age: u64| {
            let file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.set_modified(now - std::time::Duration::from_secs(age))
                .unwrap();
        };
        for i in 0..KEPT_PER_DIR + 3 {
            write(format!("aaaa-{i:016x}.scip"), 100 + i as u64);
        }
        write("bbbb-0.scip".into(), 10_000);
        prune(dir.path(), "aaaa-");
        let left = |prefix: &str| {
            std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
                .count()
        };
        assert_eq!(left("aaaa-"), KEPT_PER_DIR);
        assert_eq!(left("bbbb-"), 1);
        // The newest ones stay, the oldest go.
        assert!(dir.path().join(format!("aaaa-{:016x}.scip", 0)).exists());
        assert!(
            !dir.path()
                .join(format!("aaaa-{:016x}.scip", KEPT_PER_DIR + 2))
                .exists()
        );
    }

    #[test]
    fn trimming_drops_the_least_recently_used_entries_of_every_directory_past_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now();
        let write = |name: &str, len: u64, age: u64| {
            let file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.set_len(len).unwrap();
            file.set_modified(now - std::time::Duration::from_secs(age))
                .unwrap();
        };
        write("aaaa-0.scip", 40, 400);
        write("bbbb-0.scip", 40, 300);
        write("aaaa-1.scip", 40, 200);
        write("bbbb-1.scip", 40, 100);
        write("cccc-0.scip", 40, 0);
        // A half-written entry of another run is not an index: it is neither counted nor removed.
        write(".tmpXXXX", 1_000, 1_000);
        let written = dir.path().join("cccc-0.scip");
        let left = || {
            let mut names: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };

        trim(dir.path(), &written, 120);
        assert_eq!(
            left(),
            [".tmpXXXX", "aaaa-1.scip", "bbbb-1.scip", "cccc-0.scip"]
        );

        // The entry just written stays even when it alone is over the budget.
        trim(dir.path(), &written, 10);
        assert_eq!(left(), [".tmpXXXX", "cccc-0.scip"]);
    }

    /// One root of a project may build with another (`go.work`, a `replace`, a
    /// project reference): the key of a root reads what its indexer reads in
    /// every root, so a change next door misses the cache.
    #[test]
    fn the_cache_key_of_a_root_reads_the_other_roots_of_its_language() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let files: Vec<String> = [
            "go.work",
            "a/go.mod",
            "a/x.go",
            "b/go.mod",
            "b/y.go",
            "b/README.md",
        ]
        .map(str::to_string)
        .to_vec();
        for file in &files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "1").unwrap();
        }
        let binary = root.join("scip-go");
        std::fs::write(&binary, "binary").unwrap();
        let go = &INDEXERS[0];
        let environment = Environment::of_vars(BTreeMap::new());
        let key = || fingerprint(go, &binary, root, "a", &files, &[], &environment);
        let before = key();
        std::fs::write(root.join("b/y.go"), "2").unwrap();
        assert_ne!(key(), before);
        // What the indexer does not read stays out of the key.
        let before = key();
        std::fs::write(root.join("b/README.md"), "2").unwrap();
        assert_eq!(key(), before);
    }

    /// A PHP project scip-php can run in, with a stand-in for scip-php that
    /// runs `script` from the bin directory it returns on the `PATH`.
    #[cfg(unix)]
    fn php_project(script: &str) -> (tempfile::TempDir, PathBuf, Environment) {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let bin = tmp.path().join("bin");
        for (file, text) in [
            ("composer.json", "{}"),
            ("composer.lock", "{}"),
            ("vendor/autoload.php", "<?php\n"),
            ("a.php", "<?php\n"),
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("scip-php");
        std::fs::write(&fake, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let environment = Environment::of_vars(BTreeMap::new())
            .with("PATH", bin.as_os_str())
            .on(Platform::Unix);
        (tmp, root, environment)
    }

    fn php() -> &'static Indexer {
        INDEXERS.iter().find(|i| i.binary == "scip-php").unwrap()
    }

    /// scip-php writes its index into the directory it indexes: a run that fails
    /// takes away what it wrote, or every run after it would find it in the way.
    #[test]
    #[cfg(unix)]
    fn a_failed_indexer_leaves_no_index_in_the_project() {
        let (_tmp, root, environment) = php_project("echo partial > index.scip\nexit 1");
        let indexing = Indexing {
            cache: CacheUse::Off,
            cache_dir: None,
            environment,
            jobs: Jobs::default(),
        };
        let files = vec!["a.php".to_string()];
        let failed = index_dir(php(), &root, "", &files, &indexing, &Default::default());
        assert!(failed.is_err());
        assert!(!root.join("index.scip").exists());
    }

    /// scip-php takes any argument for a request to index: `doctor` does not
    /// ask it for its version.
    #[test]
    #[cfg(unix)]
    fn doctor_runs_no_indexer_that_would_index_for_a_version() {
        let (tmp, root, environment) = php_project("echo \"$@\" > \"$0.ran\"");
        let found = diagnose(&root, &["a.php".to_string()], &environment);
        assert!(found[0].found, "{:?}", found[0].path);
        assert_eq!(found[0].version, None);
        assert!(!tmp.path().join("bin/scip-php.ran").exists());
    }
}
