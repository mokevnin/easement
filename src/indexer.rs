//! Runs the SCIP indexers of the project's languages and reads what they write.
//!
//! easement does not resolve imports itself: an indexer that knows the language's
//! build system (`go.mod`, `tsconfig.json`) does. A missing indexer, a failed run
//! or missing dependencies is an error, never a quiet gap in the check.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::scip::{self, Document, Documents};

/// A language whose dependencies come from a SCIP index.
pub struct Language {
    pub name: &'static str,
    binary: &'static str,
    /// Files that mark a directory the indexer runs in.
    markers: &'static [&'static str],
    /// Run only in the outermost marked directories: a Cargo workspace holds its members.
    outermost: bool,
    /// Run at the project root when no marker is found (Python projects often have none).
    fallback_root: bool,
    /// Extensions of the source files the indexer covers.
    extensions: &'static [&'static str],
    /// Arguments that write the index to the given file.
    args: fn(&Path) -> Vec<String>,
    install: &'static str,
    /// Directories whose files belong to dependencies, not to the project.
    skip_dirs: &'static [&'static str],
    /// For an indexer that has no output option: the file it writes in its working directory.
    writes_in_cwd: Option<&'static str>,
    /// What has to be in place before the indexer runs: (project root, directory, shown name).
    needs: fn(&Path, &Path, &str) -> Result<()>,
}

fn nothing_needed(_: &Path, _: &Path, _: &str) -> Result<()> {
    Ok(())
}

pub const LANGUAGES: &[Language] = &[
    Language {
        name: "go",
        binary: "scip-go",
        markers: &["go.mod"],
        outermost: false,
        fallback_root: false,
        extensions: &["go"],
        args: |out| vec!["--output".into(), out.display().to_string()],
        install: "mise use -g go:github.com/scip-code/scip-go/cmd/scip-go",
        skip_dirs: &[],
        writes_in_cwd: None,
        needs: nothing_needed,
    },
    Language {
        name: "typescript",
        binary: "scip-typescript",
        markers: &["tsconfig.json"],
        outermost: false,
        fallback_root: false,
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        args: |out| {
            vec![
                "index".into(),
                "--no-progress-bar".into(),
                "--output".into(),
                out.display().to_string(),
            ]
        },
        install: "mise use -g npm:@sourcegraph/scip-typescript",
        skip_dirs: &[],
        writes_in_cwd: None,
        needs: require_installed,
    },
    Language {
        name: "python",
        binary: "scip-python",
        markers: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        outermost: false,
        fallback_root: true,
        extensions: &["py", "pyi"],
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
        skip_dirs: &[],
        writes_in_cwd: None,
        needs: nothing_needed,
    },
    Language {
        name: "php",
        binary: "scip-php",
        markers: &["composer.json"],
        outermost: false,
        fallback_root: false,
        extensions: &["php"],
        args: |_| Vec::new(),
        install: "composer global require davidrjenni/scip-php",
        skip_dirs: &["vendor"],
        writes_in_cwd: Some("index.scip"),
        needs: require_composer,
    },
    Language {
        name: "java",
        binary: "scip-java",
        markers: &["pom.xml", "build.gradle", "build.gradle.kts"],
        outermost: true,
        fallback_root: false,
        extensions: &["java"],
        args: |out| vec!["index".into(), "--output".into(), out.display().to_string()],
        install: "cs install scip-java (Coursier), see https://sourcegraph.github.io/scip-java",
        skip_dirs: &["target", "build"],
        writes_in_cwd: None,
        needs: nothing_needed,
    },
    Language {
        name: "csharp",
        binary: "scip-dotnet",
        markers: &["*.csproj", "*.sln", "*.slnx"],
        outermost: true,
        fallback_root: false,
        extensions: &["cs"],
        args: |out| vec!["index".into(), "--output".into(), out.display().to_string()],
        install: "dotnet tool install --global scip-dotnet",
        skip_dirs: &["bin", "obj"],
        writes_in_cwd: None,
        needs: nothing_needed,
    },
    Language {
        name: "rust",
        binary: "rust-analyzer",
        markers: &["Cargo.toml"],
        outermost: true,
        fallback_root: false,
        extensions: &["rs"],
        args: |out| {
            vec![
                "scip".into(),
                ".".into(),
                "--output".into(),
                out.display().to_string(),
            ]
        },
        install: "rustup component add rust-analyzer",
        skip_dirs: &[],
        writes_in_cwd: None,
        needs: nothing_needed,
    },
];

impl Language {
    pub fn covers(&self, file: &str) -> bool {
        file.rsplit_once('.')
            .is_some_and(|(_, ext)| self.extensions.contains(&ext))
            && !file.ends_with(".d.ts")
            && !self.in_skip_dir(file)
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

    /// Whether a file is in a directory of build output or dependencies.
    fn in_skip_dir(&self, file: &str) -> bool {
        file.split('/').any(|part| self.skip_dirs.contains(&part))
    }

    fn roots(&self, files: &[String]) -> Vec<String> {
        let mut dirs: Vec<String> = files
            .iter()
            .filter_map(|f| {
                let (dir, name) = f.rsplit_once('/').unwrap_or(("", f));
                self.has_marker(name).then(|| dir.to_string())
            })
            .collect();
        dirs.sort();
        dirs.dedup();
        if self.outermost {
            let all = dirs.clone();
            dirs.retain(|d| {
                !all.iter()
                    .any(|p| p != d && (p.is_empty() || d.starts_with(&format!("{p}/"))))
            });
        }
        if dirs.is_empty() && self.fallback_root {
            dirs.push(String::new());
        }
        dirs
    }
}

/// What the indexers found, and the source files they did not cover.
pub struct Indexed {
    pub documents: Documents,
    /// Indexers that ran, and indexes read from the cache instead.
    pub ran: usize,
    pub cached: usize,
    /// Source files of an indexed language that no index mentions.
    pub uncovered: Vec<String>,
}

/// Indexes every language that has source files among `files` (project files,
/// `/`-separated, relative to `root`).
pub fn run(root: &Path, files: &[String], use_cache: bool) -> Result<Indexed> {
    let (mut ran, mut cached) = (0, 0);
    let mut parts = Vec::new();
    let mut wanted: Vec<&String> = Vec::new();
    for language in LANGUAGES {
        wanted.extend(files.iter().filter(|f| language.covers(f)));
        if !files.iter().any(|f| language.covers(f)) {
            continue;
        }
        // Source files with no `go.mod` or `tsconfig.json` above them are not
        // indexed; they are reported as uncovered, or can be `exclude`d.
        let roots = language.roots(files);
        if roots.is_empty() {
            continue;
        }
        for dir in roots {
            let (documents, from_cache) = index_dir(language, root, &dir, files, use_cache)?;
            parts.push(documents);
            if from_cache {
                cached += 1;
            } else {
                ran += 1;
            }
        }
    }
    let documents = if parts.is_empty() {
        Documents::default()
    } else {
        Documents::merged(parts)?
    };
    let indexed: BTreeSet<&str> = documents.0.iter().map(|d| d.path.as_str()).collect();
    let uncovered = wanted
        .into_iter()
        .filter(|f| !indexed.contains(f.as_str()))
        .cloned()
        .collect();
    Ok(Indexed {
        documents,
        ran,
        cached,
        uncovered,
    })
}

/// Where indexes are kept between runs: `EASEMENT_CACHE_DIR`, else the user's cache directory.
fn cache_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
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

/// Everything that decides what an indexer would write: the indexer itself and
/// the name, size and modification time of every project file under `dir`
/// except its build output. Dependencies outside the project are not covered;
/// `--no-cache` is for when they change without a lock file.
fn fingerprint(
    language: &Language,
    binary: &Path,
    root: &Path,
    dir: &str,
    files: &[String],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    language.name.hash(&mut hasher);
    (language.args)(Path::new("")).hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(binary) {
        (binary, meta.len(), meta.modified().ok()).hash(&mut hasher);
    }
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut seen: Vec<(&str, u64, Option<std::time::SystemTime>)> = files
        .iter()
        .filter(|f| f.starts_with(&prefix) && !language.in_skip_dir(f))
        .filter_map(|f| {
            let meta = std::fs::metadata(root.join(f)).ok()?;
            Some((f.as_str(), meta.len(), meta.modified().ok()))
        })
        .collect();
    seen.sort();
    seen.hash(&mut hasher);
    hasher.finish()
}

/// The file the index of `dir` is kept in; the name carries the fingerprint, so a
/// changed project misses it, and `prefix` finds the stale ones to remove.
fn cache_file(root: &Path, language: &Language, dir: &str, key: u64) -> Option<(PathBuf, String)> {
    let mut hasher = DefaultHasher::new();
    (root, language.name, dir).hash(&mut hasher);
    let prefix = format!("{:016x}-", hasher.finish());
    Some((
        cache_dir()?.join(format!("{prefix}{key:016x}.scip")),
        prefix,
    ))
}

fn index_dir(
    language: &Language,
    root: &Path,
    dir: &str,
    files: &[String],
    use_cache: bool,
) -> Result<(Vec<Document>, bool)> {
    let cwd = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let shown = if dir.is_empty() { "." } else { dir };
    (language.needs)(root, &cwd, shown)?;
    // `which` finds `scip-typescript.cmd` on Windows, which `Command` alone does not.
    let binary = which::which(language.binary).map_err(|_| {
        anyhow::anyhow!(
            "`{}` is not installed, and easement needs it for {} files; install it with `{}`",
            language.binary,
            language.name,
            language.install
        )
    })?;
    let key = fingerprint(language, &binary, root, dir, files);
    let cached = use_cache
        .then(|| cache_file(root, language, dir, key))
        .flatten();
    if let Some((path, _)) = &cached
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(documents) = scip::parse(&bytes, Path::new(dir))
        && !documents.is_empty()
    {
        return Ok((documents, true));
    }
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
    let output = Command::new(binary)
        .args((language.args)(&file))
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
        .with_context(|| format!("{} wrote no index in {shown}", language.binary));
    if language.writes_in_cwd.is_some() {
        let _ = std::fs::remove_file(&file);
    }
    let bytes = bytes?;
    let documents = scip::parse(&bytes, Path::new(dir))
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
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::write(&path, &bytes);
    }
    Ok((documents, false))
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
    bail!("dependencies are not installed in {shown}: run `npm install` (or pnpm, yarn) first")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_cover_their_sources_and_roots() {
        let ts = &LANGUAGES[1];
        assert!(ts.covers("src/a.tsx") && ts.covers("a.mjs"));
        assert!(!ts.covers("src/a.d.ts") && !ts.covers("main.go"));
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
        assert_eq!(LANGUAGES[0].roots(&files), ["", "svc"]);
        assert_eq!(ts.roots(&files), ["web"]);
        // A workspace root holds its members; Python falls back to the root.
        assert_eq!(LANGUAGES[6].roots(&files), [""]);
        assert_eq!(LANGUAGES[2].roots(&files), [""]);
        let nested: Vec<String> = ["a/Cargo.toml", "b/Cargo.toml", "b/c/Cargo.toml"]
            .map(String::from)
            .to_vec();
        assert_eq!(LANGUAGES[6].roots(&nested), ["a", "b"]);
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
        std::fs::create_dir(root.join("node_modules")).unwrap();
        require_installed(root, root, ".").unwrap();
    }
}
