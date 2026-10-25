//! Runs the SCIP indexers of the project's languages and reads what they write.
//!
//! easement does not resolve imports itself: an indexer that knows the language's
//! build system (`go.mod`, `tsconfig.json`) does. A missing indexer, a failed run
//! or missing dependencies is an error, never a quiet gap in the check.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::scip::{self, Document, Documents};

/// A language whose dependencies come from a SCIP index.
pub struct Language {
    pub name: &'static str,
    binary: &'static str,
    /// The file that marks a directory the indexer runs in.
    marker: &'static str,
    /// Extensions of the source files the indexer covers.
    extensions: &'static [&'static str],
    /// Arguments that write the index to the given file.
    args: fn(&Path) -> Vec<String>,
    install: &'static str,
}

pub const LANGUAGES: &[Language] = &[
    Language {
        name: "go",
        binary: "scip-go",
        marker: "go.mod",
        extensions: &["go"],
        args: |out| vec!["--output".into(), out.display().to_string()],
        install: "mise use -g go:github.com/scip-code/scip-go/cmd/scip-go",
    },
    Language {
        name: "typescript",
        binary: "scip-typescript",
        marker: "tsconfig.json",
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
    },
];

impl Language {
    pub fn covers(&self, file: &str) -> bool {
        file.rsplit_once('.')
            .is_some_and(|(_, ext)| self.extensions.contains(&ext))
            && !file.ends_with(".d.ts")
    }

    fn roots(&self, files: &[String]) -> Vec<String> {
        files
            .iter()
            .filter_map(|f| {
                let (dir, name) = f.rsplit_once('/').unwrap_or(("", f));
                (name == self.marker).then(|| dir.to_string())
            })
            .collect()
    }
}

/// What the indexers found, and the source files they did not cover.
pub struct Indexed {
    pub documents: Documents,
    /// Source files of an indexed language that no index mentions.
    pub uncovered: Vec<String>,
}

/// Indexes every language that has source files among `files` (project files,
/// `/`-separated, relative to `root`).
pub fn run(root: &Path, files: &[String]) -> Result<Indexed> {
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
            parts.push(index_dir(language, root, &dir)?);
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
        uncovered,
    })
}

fn index_dir(language: &Language, root: &Path, dir: &str) -> Result<Vec<Document>> {
    let cwd = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let shown = if dir.is_empty() { "." } else { dir };
    if language.name == "typescript" {
        require_installed(root, &cwd, shown)?;
    }
    let out = tempfile::tempdir().context("cannot create a temporary directory")?;
    let file = out.path().join("index.scip");
    // `which` finds `scip-typescript.cmd` on Windows, which `Command` alone does not.
    let binary = which::which(language.binary).map_err(|_| {
        anyhow::anyhow!(
            "`{}` is not installed, and easement needs it for {} files; install it with `{}`",
            language.binary,
            language.name,
            language.install
        )
    })?;
    let output = Command::new(binary)
        .args((language.args)(&file))
        .current_dir(&cwd)
        .output()
        .with_context(|| format!("cannot run {}", language.binary))?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = text.lines().rev().take(8).collect();
        bail!(
            "{} failed in {shown} ({}); if that directory is not part of the project, add it to `exclude`:\n{}",
            language.binary,
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    let bytes = std::fs::read(&file)
        .with_context(|| format!("{} wrote no index in {shown}", language.binary))?;
    let documents = scip::parse(&bytes, Path::new(dir))
        .with_context(|| format!("cannot read the index of {} in {shown}", language.binary))?;
    if documents.is_empty() {
        bail!(
            "{} indexed no files in {shown}; check the `include` of its {}",
            language.binary,
            language.marker
        );
    }
    Ok(documents)
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
        let files: Vec<String> = ["go.mod", "svc/go.mod", "web/tsconfig.json", "a.go"]
            .map(String::from)
            .to_vec();
        assert_eq!(LANGUAGES[0].roots(&files), ["", "svc"]);
        assert_eq!(ts.roots(&files), ["web"]);
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
