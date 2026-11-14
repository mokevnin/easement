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
        scan::scan(&self.root, &self.config().scan_policy(), cache)
    }

    /// What `policy` says of the scanned code: its violations, and what its
    /// rules could not judge. Any policy with the scan-relevant part of the
    /// project's can be judged on its scan.
    pub fn judge(&self, policy: &Policy, scan: &scan::Scan) -> Result<Judgment> {
        self.judgment(policy, scan, true)
    }

    /// As [`Project::judge`], but a diagram that cannot be read leaves the
    /// `diagram` rule unjudged, and says why in `unread_diagram`, instead of
    /// failing: for a command that tells it itself, as `review` does.
    pub fn judge_despite_unread_diagram(
        &self,
        policy: &Policy,
        scan: &scan::Scan,
    ) -> Result<Judgment> {
        self.judgment(policy, scan, false)
    }

    fn judgment(
        &self,
        policy: &Policy,
        scan: &scan::Scan,
        unread_diagram_fails: bool,
    ) -> Result<Judgment> {
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
        let mut unread_diagram = None;
        match diagram::check(policy, &edges) {
            Ok(drawn) => violations.extend(drawn),
            Err(err) if unread_diagram_fails => return Err(err),
            Err(err) => unread_diagram = Some(format!("{err:#}")),
        }
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
            unresolved: &scan.unresolved_imports,
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
            unread_diagram,
        })
    }

    /// The analysis of a scan under the policy the project was opened with,
    /// over the whole project: what a run reports on narrows its violations
    /// afterwards, never what was read, so every evidence gap counts.
    pub fn analysis(&self, scan: scan::Scan) -> Result<Analysis> {
        let judgment = self.judge(&self.policy, &scan)?;
        self.analysis_of(judgment, scan)
    }

    /// As [`Project::analysis`], but a diagram that cannot be read leaves the
    /// `diagram` rule unjudged, as [`Project::judge_despite_unread_diagram`] does.
    pub fn analysis_despite_unread_diagram(&self, scan: scan::Scan) -> Result<Analysis> {
        let judgment = self.judge_despite_unread_diagram(&self.policy, &scan)?;
        self.analysis_of(judgment, scan)
    }

    /// The analysis of a scan from the judgment of the project's own policy on it.
    fn analysis_of(&self, judgment: Judgment, scan: scan::Scan) -> Result<Analysis> {
        let Judgment {
            violations,
            edges,
            mut stats,
            map,
            unread_diagram,
        } = judgment;
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
        let accepted = graph_unread(self.config(), &map, &scan, &mut stats);
        Ok(Analysis {
            unread_diagram,
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
    /// Why the diagram the policy names could not be read, when the judging
    /// went on without it: the `diagram` rule was not judged.
    pub unread_diagram: Option<String>,
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
    /// As [`Judgment::unread_diagram`].
    pub unread_diagram: Option<String>,
}

/// How many files a line about test code that was not told apart names.
const PARTS_SHOWN: usize = 3;

/// One line for each kind of doubt about test code, with the files it holds.
fn parts_reasons(doubts: &[(String, lang::parts::Doubt)]) -> Vec<check::Unjudged> {
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
            let named = check::name_first(
                &files,
                PARTS_SHOWN,
                ", ",
                |file| file.to_string(),
                |more| format!(" and {more} more"),
            );
            check::Unjudged::new(doubt.reason(&named)).in_files(files)
        })
        .collect()
}

/// What `scan` left out of the graph, into `stats`: the files no indexer
/// covered, the imports it did not resolve and, when tests are parts, the files
/// whose parts it could not tell. Every answer about the graph reports them; the
/// files `uncovered_ok` accepts are returned apart.
pub fn graph_unread(
    config: &Config,
    map: &ComponentMap,
    scan: &scan::Scan,
    stats: &mut check::Stats,
) -> Vec<components::AcceptedGap> {
    let accepted;
    (stats.uncovered, accepted) = map.split_uncovered(&scan.uncovered);
    stats.imports_partial = unresolved_reasons(&scan.unresolved_imports);
    if config.tests.is_some() {
        stats.parts_partial = parts_reasons(&scan.parts_doubts);
    }
    accepted
}

/// How many imports the line about those the indexer did not resolve names.
const IMPORTS_SHOWN: usize = 3;

/// One line for each cause of the imports whose dependency is not in the graph, as
/// the language of each explains it: how many, what usually brings the cause about,
/// the first few by file and line, the place of every one of them and, where the
/// language says it, how to close it.
fn unresolved_reasons(imports: &[scan::Unresolved]) -> Vec<check::Unjudged> {
    type Key = (lang::Cause, Option<&'static lang::registry::Explained>);
    let mut by_cause: std::collections::BTreeMap<Key, Vec<&scan::Unresolved>> =
        std::collections::BTreeMap::new();
    for unresolved in imports {
        by_cause
            .entry((unresolved.cause, unresolved.explained))
            .or_default()
            .push(unresolved);
    }
    by_cause
        .into_iter()
        .map(|((cause, explained), imports)| {
            let mut places: Vec<check::Place> =
                imports.iter().map(|unresolved| unresolved.place()).collect();
            // Named in the order `places` keeps them in, each once.
            places.sort();
            places.dedup();
            let named =
                check::name_first(&places, IMPORTS_SHOWN, ", ", ToString::to_string, |more| {
                    format!(" and {more} more")
                });
            let n = places.len();
            let what = match cause {
                lang::Cause::NoTarget => format!("the indexer did not resolve {n} import(s)"),
                lang::Cause::Wildcard => format!(
                    "the index ties {n} import(s) only to a wildcard declaration, and none names a file of the project or a package"
                ),
                lang::Cause::SideEffect => {
                    format!("the index holds nothing of {n} import(s) for side effects alone")
                }
                lang::Cause::Require => format!("the index holds no `require()` of {n} import(s)"),
                lang::Cause::Computed => {
                    format!("the target of {n} import(s) is computed at run time")
                }
            };
            let because = explained.map_or(String::new(), |explained| {
                format!(" ({})", explained.because)
            });
            let unjudged = check::Unjudged::new(format!(
                "{what}, so the dependencies they write were not checked{because}: {named}"
            ))
            .in_files(imports.iter().map(|unresolved| unresolved.file.as_str()))
            .at(places);
            match explained.and_then(|explained| explained.close) {
                Some(close) => unjudged.closed_by(close),
                None => unjudged,
            }
        })
        .collect()
}
