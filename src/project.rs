//! A project as a run opens it: its policy, the scan of its files, what the
//! policy judges of the scan, and the analysis every command reads.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::components::{self, ComponentMap};
use crate::config::Config;
use crate::policy::Policy;
use crate::{
    chains, check, diagram, examples, indexer, lang, manifests, matching, relpath, scan, scip,
    suppress, symbols, unused,
};

pub struct Project {
    pub root: PathBuf,
    pub policy: Policy,
    /// The config file, resolved.
    pub config_path: PathBuf,
}

impl Project {
    pub fn open(config_path: &Path) -> Result<Self> {
        let policy = Policy::load(config_path, None)?;
        let config_path = dunce::canonicalize(config_path)
            .with_context(|| format!("cannot resolve {}", config_path.display()))?;
        let root = config_path
            .parent()
            .context("config has no parent directory")?
            .to_path_buf();
        Ok(Self {
            root,
            policy,
            config_path,
        })
    }

    /// The config of the policy the project was opened with.
    pub fn config(&self) -> &Config {
        &self.policy.config
    }

    pub fn baseline_path(&self) -> PathBuf {
        self.root.join(self.config().baseline_file())
    }

    pub fn analyze(&self, cache: &mut scan::Cache) -> Result<Analysis> {
        let scan = self.scan(cache)?;
        self.analysis(scan)
    }

    /// The project as the policy it was opened with scans it.
    pub fn scan(&self, cache: &mut scan::Cache) -> Result<scan::Scan> {
        // The components are read by the scan: a name that more than one of
        // them declares is resolved for the component of the importing file.
        let map = ComponentMap::new(self.config())?;
        scan::scan(&self.root, &self.config().scan_policy(), &map, cache)
    }

    /// What `policy` says of the scanned code: its violations, and what its
    /// rules could not judge. Any policy with the scan-relevant part of the
    /// project's can be judged on its scan.
    pub fn judge(&self, policy: &Policy, scan: &scan::Scan) -> Result<Judgment> {
        let config = &policy.config;
        let map = ComponentMap::new(config)?;
        let (mut violations, edges, mut stats) = check::evaluate(config, &map, &scan.deps);
        violations.extend(symbols::symbols(config, &map, &scan.symbols));
        violations.extend(check::naming(config, &map, &scan.types)?);
        let undeclared =
            manifests::undeclared(config, &map, &self.root, &scan.deps, &scan.all_files);
        violations.extend(undeclared.violations);
        let (mut violations, suppressed) = suppress::apply(violations, &scan.suppressions);
        let config_file = crate::relpath::slash(self.config_path.strip_prefix(&self.root)?);
        violations.extend(matching::unmatched(
            policy,
            &map,
            &scan.all_files,
            &scan.deps,
            &config_file,
        ));
        violations.extend(symbols::unmatched_symbols(
            policy,
            &scan.known_symbols,
            scan.indexed_whole(),
            &config_file,
        ));
        violations.extend(examples::check(policy, &map, &self.root));
        violations.extend(chains::cycles(config, &edges));
        violations.extend(diagram::check(policy, &edges)?);
        violations.extend(chains::reaches(
            config,
            &map,
            &scan.deps,
            &scan.all_files,
            scan.flow.as_ref(),
        ));
        violations.extend(check::metrics(config, &edges));
        violations.extend(matching::orphans(config, &map, &scan.paths));
        violations.extend(check::forbidden_files(config, &map, &scan.all_files));
        violations.extend(check::required(
            &self.root,
            config,
            &map,
            &scan.deps,
            &scan.paths,
            &scan.all_files,
        ));
        let unused = unused::unused(&unused::Input {
            config,
            map: &map,
            root: &self.root,
            deps: &scan.deps,
            unread_deps: &scan.unread_deps,
            unread: &scan.unread,
            paths: &scan.paths,
            all_files: &scan.all_files,
            uncovered: &scan.uncovered,
        })?;
        violations.extend(unused.violations);
        chains::attribute(policy, &map, &scan.deps, &mut violations);
        stats.suppressed = suppressed;
        stats.unused_partial = unused.reasons;
        stats.manifests_partial = undeclared.reasons;
        Ok(Judgment {
            violations,
            edges,
            stats,
            map,
        })
    }

    /// The analysis of a scan under the policy the project was opened with,
    /// over the whole project: what a run reports on narrows its violations
    /// afterwards, never what was read, so every evidence gap counts.
    pub fn analysis(&self, scan: scan::Scan) -> Result<Analysis> {
        let Judgment {
            violations,
            edges,
            mut stats,
            map,
        } = self.judge(&self.policy, &scan)?;
        if self
            .config()
            .components
            .values()
            .any(|c| !c.defines.is_empty())
        {
            let unseen: std::collections::BTreeSet<String> = scan
                .paths
                .iter()
                .filter(|f| map.component_of(f).is_some())
                // Types come from an index; an adapter reads imports alone.
                .filter(|f| !indexer::indexed(f))
                .map(|f| relpath::extension(f))
                .filter(|ext| !ext.is_empty())
                .map(str::to_string)
                .collect();
            stats.naming_unseen = unseen.into_iter().collect();
        }
        if self.config().packages {
            stats.packages_partial = scan.packages_problems.clone();
        }
        stats.declarations_partial = scan.declarations_problems.clone();
        if self.config().tests.is_some() {
            stats.parts_partial = parts_reasons(&scan.parts_doubts);
        }
        stats.symbols_partial =
            symbols::symbols_partial(self.config(), &map, &scan.unnamed_uses, &scip::UNNAMED);
        let paths: Vec<&str> = scan.paths.iter().map(String::as_str).collect();
        stats.symbols_partial.extend(symbols::symbols_unverified(
            self.config(),
            &map,
            &paths,
            &scan.deps,
            &scan.unverified_symbols,
        ));
        stats.files = scan.paths.len();
        (stats.indexed, stats.from_cache) = (scan.indexed, scan.from_cache);
        let accepted;
        (stats.uncovered, accepted) = map.split_uncovered(&scan.uncovered);
        Ok(Analysis {
            accepted,
            unindexed: scan.uncovered,
            violations,
            all_files: scan.all_files,
            edges,
            deps: scan.deps,
            files: scan.paths,
            stats,
            map,
        })
    }

    /// Everything that changes the verdict: the files of the project and the
    /// baseline, as a hash of their names, sizes and times, and every policy
    /// input, wherever it is, as a hash of its content.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        let mut files: Vec<PathBuf> = crate::relpath::walker(&self.root)
            .build()
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.into_path())
            .collect();
        files.push(self.baseline_path());
        files.sort();
        files.dedup();
        for file in files {
            if let Ok(meta) = std::fs::metadata(&file) {
                (&file, meta.len(), meta.modified().ok()).hash(&mut hasher);
            }
        }
        for input in &self.policy.inputs {
            (&input.path, &input.content).hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// What a policy says of a scan.
pub struct Judgment {
    pub violations: Vec<check::Violation>,
    pub edges: Vec<check::Edge>,
    pub stats: check::Stats,
    pub map: ComponentMap,
}

pub struct Analysis {
    pub violations: Vec<check::Violation>,
    /// Every source file no indexer read, `uncovered_ok` or not: the report
    /// leaves the accepted ones out, but nobody saw their imports, so what
    /// `suggest`, `impact`, `why` and `baseline` say about them stays careful.
    pub unindexed: Vec<String>,
    /// The files of `unindexed` that `uncovered_ok` accepts, by entry.
    pub accepted: Vec<components::AcceptedGap>,
    /// Every file that is not excluded, source or not.
    pub all_files: Vec<String>,
    pub edges: Vec<check::Edge>,
    pub deps: Vec<lang::Dep>,
    pub files: Vec<String>,
    pub stats: check::Stats,
    pub map: ComponentMap,
}

/// How many files a line about test code that was not told apart names.
const PARTS_SHOWN: usize = 3;

/// One line for each kind of doubt about test code, with the files it holds.
pub fn parts_reasons(doubts: &[(String, lang::parts::Doubt)]) -> Vec<check::Unjudged> {
    let mut by_kind: std::collections::BTreeMap<lang::parts::Doubt, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (file, doubt) in doubts {
        by_kind.entry(*doubt).or_default().push(file);
    }
    by_kind
        .into_iter()
        .map(|(doubt, mut files)| {
            files.sort_unstable();
            // A file with several modules of one doubt is named once.
            files.dedup();
            let mut named = files
                .iter()
                .take(PARTS_SHOWN)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            if files.len() > PARTS_SHOWN {
                named.push_str(&format!(" and {} more", files.len() - PARTS_SHOWN));
            }
            check::Unjudged::new(doubt.reason(&named)).in_files(files)
        })
        .collect()
}
