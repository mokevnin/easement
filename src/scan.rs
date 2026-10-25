use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::config::Config;
use crate::lang::{self, Target};

/// A resolved dependency of one project file.
#[derive(Debug, Clone)]
pub struct Dep {
    pub file: PathBuf,
    pub line: usize,
    pub specifier: String,
    pub target: Target,
}

pub struct Scan {
    pub files: usize,
    pub deps: Vec<Dep>,
}

/// Project walker: honours .gitignore (even outside a git checkout) and
/// always skips `node_modules`.
pub fn walker(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .require_git(false)
        .filter_entry(|e| e.file_name() != "node_modules");
    builder
}

pub fn scan(root: &Path, config: &Config) -> Result<Scan> {
    let exclude = exclude_set(&config.exclude)?;
    let adapters = lang::adapters(root, config)?;
    let mut files = 0;
    let mut deps = Vec::new();
    for entry in walker(root).build() {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(rel) = lang::relative(root, path) else {
            continue;
        };
        if exclude.is_match(&rel) {
            continue;
        }
        let Some(adapter) = adapters.iter().find(|a| a.handles(path)) else {
            continue;
        };
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue, // binary or unreadable
        };
        files += 1;
        for import in adapter
            .imports(path, &source)
            .with_context(|| format!("{} adapter failed on {}", adapter.name(), rel.display()))?
        {
            let target = adapter.resolve(path, &import.specifier);
            deps.push(Dep {
                file: rel.clone(),
                line: import.line,
                specifier: import.specifier,
                target,
            });
        }
    }
    Ok(Scan { files, deps })
}

fn exclude_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let p = p.trim_end_matches('/');
        builder.add(Glob::new(p).with_context(|| format!("bad exclude pattern `{p}`"))?);
        builder.add(Glob::new(&format!("{p}/**"))?);
    }
    Ok(builder.build()?)
}

/// `/`-separated string form of a relative path, used for matching and output.
pub fn slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
