use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::config::Config;
use crate::lang::{self, Import, Target};
use crate::suppress::{self, Suppression};

/// A resolved dependency of one project file.
#[derive(Debug, Clone)]
pub struct Dep {
    pub file: PathBuf,
    pub line: usize,
    pub specifier: String,
    pub target: Target,
}

/// What was read from a file, valid while its size and modification time stay the same.
struct Parsed {
    modified: Option<SystemTime>,
    len: u64,
    imports: Vec<Import>,
    dynamic: Vec<usize>,
    suppressions: Vec<Suppression>,
}

/// Parsed files kept between scans, so watch mode re-reads only what changed.
/// Resolution is never cached: it depends on the other files.
#[derive(Default)]
pub struct Cache(HashMap<PathBuf, Parsed>);

pub struct Scan {
    /// Every file that is not excluded, source or not, relative to the root with `/` separators.
    pub all_files: Vec<String>,
    /// `file:line` of imports whose target is computed at run time.
    pub dynamic: Vec<String>,
    /// Every checked file, relative to the root with `/` separators.
    pub paths: Vec<String>,
    pub deps: Vec<Dep>,
    pub suppressions: Vec<Suppression>,
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

pub fn scan(root: &Path, config: &Config, cache: &mut Cache) -> Result<Scan> {
    let exclude = exclude_set(&config.exclude)?;
    let adapters = lang::adapters(root, config)?;
    let mut paths = Vec::new();
    let mut all_files = Vec::new();
    let mut deps = Vec::new();
    let mut dynamic = Vec::new();
    let mut suppressions = Vec::new();
    let mut seen = HashSet::new();
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
        all_files.push(slash(&rel));
        let Some(adapter) = adapters.iter().find(|a| a.handles(path)) else {
            continue;
        };
        let meta = entry.metadata()?;
        let modified = meta.modified().ok();
        let fresh = cache
            .0
            .get(&rel)
            .is_some_and(|p| p.len == meta.len() && p.modified == modified && modified.is_some());
        if !fresh {
            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue, // binary or unreadable
            };
            let parsed = Parsed {
                modified,
                len: meta.len(),
                imports: adapter.imports(path, &source).with_context(|| {
                    format!("{} adapter failed on {}", adapter.name(), rel.display())
                })?,
                dynamic: adapter.dynamic_imports(path, &source).with_context(|| {
                    format!("{} adapter failed on {}", adapter.name(), rel.display())
                })?,
                suppressions: suppress::parse(&slash(&rel), &source),
            };
            cache.0.insert(rel.clone(), parsed);
        }
        let parsed = &cache.0[&rel];
        seen.insert(rel.clone());
        paths.push(slash(&rel));
        suppressions.extend(parsed.suppressions.iter().cloned());
        dynamic.extend(
            parsed
                .dynamic
                .iter()
                .map(|l| format!("{}:{l}", slash(&rel))),
        );
        for import in &parsed.imports {
            deps.push(Dep {
                file: rel.clone(),
                line: import.line,
                specifier: import.specifier.clone(),
                target: adapter.resolve(path, &import.specifier),
            });
        }
    }
    cache.0.retain(|path, _| seen.contains(path));
    Ok(Scan {
        all_files,
        dynamic,
        paths,
        deps,
        suppressions,
    })
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
