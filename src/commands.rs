//! What each command computes, from what it was asked to its answer. The binary
//! (`main.rs`) only parses the arguments, prints the answer and turns its
//! outcome into an exit code; everything it says is decided here.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::baseline::{Baseline, Verdict};
use crate::components::ComponentMap;
use crate::policy::Policy;
use crate::project::{self, Analysis, Project};
use crate::{
    check, diagram, diff, doctor, drift, explain, graph, impact, indexer, patch, propose, report,
    resolved, review, scan, scope, suggest,
};

/// How a command ends, which its exit code says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// No violations, but `--require-evidence` and an evidence gap.
    EvidenceRequired,
    /// `doctor --cache`: an index is not in the cache, so a run would index again.
    NotCached,
}

impl From<bool> for Outcome {
    fn from(passed: bool) -> Self {
        if passed {
            Outcome::Passed
        } else {
            Outcome::Failed
        }
    }
}

/// What `easement check` was asked.
#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub config: PathBuf,
    /// Ignore the baseline and report every violation.
    pub no_baseline: bool,
    /// `--diff`: judge the change since the merge-base with this revision.
    pub diff: Option<String>,
    /// `--component`: report only on the files of these components.
    pub components: Vec<String>,
    /// `--scope`: report only on files under these paths.
    pub paths: Vec<String>,
    /// `--require-evidence`: an evidence gap fails a run without violations.
    pub require_evidence: bool,
    /// `--suggest`: a `may_depend_on` entry no import uses fails as `unused-allowance`.
    pub suggest: bool,
    /// `--plan`: the text of the plan `propose --plan` judged, which the imports the
    /// change added are compared with; it needs `diff`.
    pub plan: Option<String>,
}

impl CheckOptions {
    /// A check of the whole project under the policy in `config`.
    pub fn new(config: impl Into<PathBuf>) -> Self {
        CheckOptions {
            config: config.into(),
            no_baseline: false,
            diff: None,
            components: Vec::new(),
            paths: Vec::new(),
            require_evidence: false,
            suggest: false,
            plan: None,
        }
    }
}

/// The answer of `easement check`: the verdict, and what it leaves out.
pub struct Checked {
    pub verdict: Verdict,
    pub stats: check::Stats,
    pub coverage: report::Coverage,
    pub diff: Option<diff::Diff>,
    pub require_evidence: bool,
    /// Under `--suggest`, the `may_depend_on` entries no import uses that it does not
    /// fail on, since it cannot tell whether they can go.
    pub spared: suggest::Spared,
    /// Under `--plan`, how the change departs from the plan.
    pub drift: Option<drift::Drift>,
}

impl Checked {
    /// The report, in each of the formats it is printed in.
    pub fn report(&self) -> report::Report<'_> {
        report::Report {
            verdict: &self.verdict,
            stats: &self.stats,
            coverage: &self.coverage,
            diff: self.diff.as_ref(),
            require_evidence: self.require_evidence,
            spared: &self.spared,
            drift: self.drift.as_ref(),
        }
    }

    pub fn outcome(&self) -> Outcome {
        if !self.verdict.passed() {
            Outcome::Failed
        } else if self.report().evidence_required() {
            Outcome::EvidenceRequired
        } else {
            Outcome::Passed
        }
    }
}

/// Checks the project as `options` ask, reading the indexes through `cache`.
pub fn check(options: &CheckOptions, cache: &mut scan::Cache) -> Result<Checked> {
    if options.suggest
        && (options.diff.is_some() || !options.paths.is_empty() || !options.components.is_empty())
    {
        anyhow::bail!(
            "`--suggest` judges the whole project, since whether an allowance is used depends on every import; drop `--diff`, `--scope` and `--component`, or `--suggest`"
        );
    }
    if options.plan.is_some() && options.diff.is_none() {
        anyhow::bail!(
            "`--plan` compares the imports a change added with the plan, so it needs `--diff <base>`"
        );
    }
    let project = Project::open(&options.config)?;
    let scope = scope::Scope::new(project.config(), &options.components, &options.paths)?;
    let relative = |path: &str| explain::root_relative(&project.root, Path::new(path));
    let proposals = options
        .plan
        .as_deref()
        .map(|text| propose::plan(text, relative))
        .transpose()?;
    let scan = project.scan(cache)?;
    let diff = options
        .diff
        .as_ref()
        .map(|base| -> Result<diff::Diff> {
            let changes = diff::Changes::collect(&project.root, base)?;
            // The policy at the merge-base, judged on the code scanned now.
            let attribution =
                diff::Attribution::of(&project.policy, &project.root, &changes, |policy| {
                    Ok(project.judge(policy, &scan)?.produced().cloned().collect())
                });
            Ok(diff::Diff {
                changes,
                attribution,
            })
        })
        .transpose()?;
    let Analysis {
        mut violations,
        suppressions,
        mut stats,
        map,
        component_deps,
        deps,
        accepted,
        unindexed,
        ..
    } = project.analysis(scan)?;
    let mut spared = suggest::Spared::default();
    if options.suggest {
        let (unused, left) = suggest::unused_allowances(
            &project.policy,
            &map,
            &component_deps,
            &unindexed,
            &project.config_file()?,
        );
        violations.extend(unused);
        spared = left;
    }
    let current = violations.iter().map(check::Violation::key).collect();
    // Cycles and limits come from the whole project, so a violation belongs to
    // the scope when any file behind it does.
    let in_scope = |v: &check::Violation| {
        v.rule.is_config_rule() || v.evidence.iter().any(|at| scope.contains(&map, &at.file))
    };
    let violations: Vec<_> = violations.into_iter().filter(in_scope).collect();
    // A suppression, when anything it excused is.
    let suppressions = suppressions
        .into_iter()
        .filter(|s| s.excused.iter().any(in_scope));
    let mut drift = None;
    if let Some(diff) = &diff {
        stats.new_dependencies =
            check::new_dependencies(&component_deps, |f, l| diff.changes.contains(f, l));
        drift = proposals.as_deref().map(|plan| {
            drift::compare(&drift::Compared {
                plan,
                deps: &deps,
                map: &map,
                changed: |f: &str, l| diff.changes.contains(f, l),
                in_scope: |f: &str| scope.contains(&map, f),
                unindexed: &unindexed,
            })
        });
    }
    let (judged, suppressions) = match &diff {
        Some(diff) => (
            violations.into_iter().filter(|v| diff.keeps(v)).collect(),
            suppressions.filter(|s| diff.keeps_suppression(s)).collect(),
        ),
        None => (violations, suppressions.collect()),
    };
    let baseline = if options.no_baseline {
        None
    } else {
        Baseline::load(&project.baseline_path())?
    };
    let (warnings, judged): (Vec<_>, Vec<_>) = check::dedupe(judged)
        .into_iter()
        .partition(|v| project.config().warns(v.rule));
    let mut verdict = Verdict::judge(judged, warnings, &current, baseline.as_ref());
    verdict.owners = project.config().owners();
    verdict.suppressions = suppressions;
    verdict.warned = project
        .config()
        .warn
        .iter()
        .filter_map(|rule| check::RuleId::suppressible(rule))
        .collect();
    // A file no indexer read shows none of its debt, which is not paid for
    // that: `baseline` keeps its entries, so they are not stale (the evidence
    // gaps name the file).
    verdict
        .stale
        .retain(|k| scope.contains(&map, &k.file) && !unindexed.contains(&k.file));
    verdict.sort();
    // Judged in full, the change narrows nothing.
    let narrowed = report::narrowed(
        diff.as_ref()
            .filter(|d| !d.attribution.whole())
            .map(|d| &d.changes),
        &options.components,
        &options.paths,
    );
    stats.limited_to = report::limited_to(&narrowed);
    let coverage = report::Coverage::of_check(&stats, narrowed, accepted);
    Ok(Checked {
        verdict,
        stats,
        coverage,
        diff,
        require_evidence: options.require_evidence,
        spared,
        drift,
    })
}

/// Writes a starting `easement.yml` into `root` and says what it contains,
/// reading the indexes through `cache`.
pub fn init(root: &Path, force: bool, cache: &mut scan::Cache) -> Result<String> {
    crate::init::run(root, force, cache)
}

/// What changes the verdict of the project whose policy is in `config`, as a
/// hash; `None` while the policy cannot be read.
pub fn fingerprint(config: &Path) -> Option<u64> {
    Project::open(config).ok().map(|p| p.fingerprint())
}

/// What `easement baseline` wrote, and what it would not.
pub struct Recorded {
    pub path: PathBuf,
    /// How many violations were written.
    pub written: usize,
    /// Old entries kept for files no indexer read.
    pub kept: usize,
    /// New violations not written: the baseline only shrinks.
    pub refused: Vec<check::Violation>,
    /// Violations of the config, which the baseline never accepts.
    pub config_rules: usize,
    pub cached: Option<String>,
}

impl Recorded {
    pub fn text(&self) -> String {
        let mut out = format!(
            "easement: {} violation(s) written to {}\n",
            self.written,
            self.path.display()
        );
        if let Some(note) = &self.cached {
            out.push_str(&format!("easement: {note}\n"));
        }
        if self.kept > 0 {
            out.push_str(&format!(
                "easement: {} entr(ies) kept for files no indexer covered, so their debt is not known to be paid; `easement check` lists those files\n",
                self.kept
            ));
        }
        for v in &self.refused {
            out.push_str(&report::violation_lines(v, None, Default::default(), false));
        }
        if !self.refused.is_empty() {
            out.push_str(&format!(
                "easement: {} new violation(s) not written: the baseline only shrinks. Fix them, or legalize them with `easement baseline --accept-new` if a human agrees\n",
                self.refused.len()
            ));
        }
        if self.config_rules > 0 {
            out.push_str(&format!(
                "easement: {} violation(s) of the config left out, the baseline never accepts them; `easement check` lists them\n",
                self.config_rules
            ));
        }
        out
    }

    pub fn outcome(&self) -> Outcome {
        Outcome::from(self.refused.is_empty())
    }
}

/// Records the current violations in the baseline; new ones only with `accept_new`.
pub fn baseline(config: &Path, accept_new: bool, cache: &mut scan::Cache) -> Result<Recorded> {
    let project = Project::open(config)?;
    let analysis = project.analyze(cache)?;
    let (config_rules, violations): (Vec<_>, Vec<_>) = check::dedupe(analysis.violations)
        .into_iter()
        .partition(|v| v.rule.is_config_rule());
    // Warnings never fail the check, so there is nothing to legalize.
    let violations: Vec<_> = violations
        .into_iter()
        .filter(|v| !project.config().warns(v.rule))
        .collect();
    let path = project.baseline_path();
    let old = Baseline::load(&path)?;
    let (violations, refused) = match &old {
        Some(old) => old.admit(violations, accept_new),
        None => (violations, Vec::new()),
    };
    let mut baseline = Baseline::from_violations(&violations);
    // A file no indexer read may still hold its debt: keep its entries.
    let unread: Vec<_> = old
        .iter()
        .flat_map(|old| &old.violations)
        .filter(|k| analysis.unindexed.contains(&k.file))
        .filter(|k| !baseline.violations.contains(k))
        .cloned()
        .collect();
    let kept = unread.len();
    baseline.violations.extend(unread);
    baseline.save(&path)?;
    Ok(Recorded {
        path,
        written: violations.len(),
        kept,
        refused,
        config_rules: config_rules.len(),
        cached: report::cache_note(&analysis.stats),
    })
}

/// The answer of `easement impact`, with the components it is told by.
pub struct Impacted {
    pub impact: impact::Impact,
    map: ComponentMap,
}

impl Impacted {
    pub fn text(&self) -> String {
        impact::render(&self.impact, &self.map)
    }

    pub fn json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.impact)?)
    }
}

/// Every file that depends on `paths` (relative to the working directory or to the
/// config), and on the files changed since the merge-base with `diff`.
pub fn impact(
    config: &Path,
    paths: &[PathBuf],
    diff: Option<&str>,
    cache: &mut scan::Cache,
) -> Result<Impacted> {
    let project = Project::open(config)?;
    if paths.is_empty() && diff.is_none() {
        anyhow::bail!("name the changed files or directories, or pass `--diff <base>`");
    }
    // Only changed lines tell what a change touched, and only the flow who uses it.
    let analysis = match diff {
        Some(_) => project.analyze_with_flow(cache)?,
        None => project.analyze(cache)?,
    };
    let named: Vec<String> = paths
        .iter()
        .map(|p| explain::root_relative(&project.root, p))
        .collect();
    let mut changed = impact::files_under(&named, &analysis.all_files)?;
    let mut changes = None;
    if let Some(base) = diff {
        let collected = diff::Changes::collect(&project.root, base)?;
        let known: std::collections::HashSet<&str> =
            analysis.all_files.iter().map(String::as_str).collect();
        changed.extend(
            collected
                .files()
                .filter(|f| known.contains(f))
                .map(str::to_string),
        );
        changes = Some(collected);
    }
    let touched = match &changes {
        Some(changes) => {
            // Without the flow every caller list would be empty and look whole.
            let flow = analysis
                .flow
                .as_ref()
                .context("the scan of an `impact --diff` built no flow")?;
            Some(impact::touched(
                &analysis.map,
                flow,
                changes,
                &changed,
                &analysis.files,
            ))
        }
        None => None,
    };
    let mut result = impact::impact(
        &analysis.map,
        &analysis.deps,
        &analysis.all_files,
        changed,
        changes.map(|changes| changes.deleted).unwrap_or_default(),
        report::graph_gaps(&analysis.stats, &analysis.unindexed),
        analysis.accepted,
    );
    if let Some((touched, gaps)) = touched {
        result.touched = Some(touched);
        result.callers_leave_out = Some(impact::CALLERS_LEAVE_OUT);
        result.coverage.evidence_gaps.extend(gaps);
    }
    result.cached = report::cache_note(&analysis.stats);
    Ok(Impacted {
        impact: result,
        map: analysis.map,
    })
}

/// Why component `from` depends on `to`, or what keeps it from doing so.
pub fn why(config: &Path, from: &str, to: &str, cache: &mut scan::Cache) -> Result<impact::Why> {
    let project = Project::open(config)?;
    let analysis = project.analyze(cache)?;
    let evidence_gaps = report::graph_gaps(&analysis.stats, &analysis.unindexed);
    let graph = impact::Graph {
        map: &analysis.map,
        deps: &analysis.deps,
        files: &analysis.all_files,
        violations: &analysis.violations,
        evidence_gaps: &evidence_gaps,
        accepted: &analysis.accepted,
    };
    let mut why = impact::why(&project.policy, &graph, from, to)?;
    why.cached = report::cache_note(&analysis.stats);
    Ok(why)
}

/// Which component a path belongs to and what that component may do.
pub fn explain(config: &Path, path: &Path) -> Result<explain::Explanation> {
    let project = Project::open(config)?;
    let map = ComponentMap::new(project.config())?;
    let rel = explain::root_relative(&project.root, path);
    Ok(explain::explain(&project.policy, &map, &rel))
}

/// The policy in `config` as loaded: `extends` merged, the anchors, globs and
/// sets expanded, each item with where the policy writes it. It reads no code.
pub fn policy(config: &Path) -> Result<resolved::ResolvedPolicy> {
    let project = Project::open(config)?;
    let map = ComponentMap::new(project.config())?;
    Ok(resolved::ResolvedPolicy::of(&project.policy, &map))
}

/// A drawing of the components and the dependencies between them, with what
/// it leaves out apart from it.
pub struct Drawn {
    pub diagram: String,
    /// The evidence gaps and the cache note, for a channel other than the diagram's.
    pub notes: Vec<String>,
}

/// Draws the components, or only `focus` and the ones it shares an arrow with.
pub fn graph(
    config: &Path,
    format: graph::GraphFormat,
    focus: Option<&str>,
    cache: &mut scan::Cache,
) -> Result<Drawn> {
    let project = Project::open(config)?;
    if let Some(name) = focus {
        project.config().require_component(name)?;
    }
    let analysis = project.analyze(cache)?;
    let notes = report::evidence_gaps(&analysis.stats)
        .into_iter()
        .map(|gap| format!("partial: {}", gap.message))
        .chain(report::cache_note(&analysis.stats))
        .collect();
    Ok(Drawn {
        diagram: graph::render(
            format,
            project.config(),
            &analysis.component_deps,
            &analysis.violations,
            focus,
        ),
        notes,
    })
}

/// What the dependency graph suggests for the policy.
pub fn suggest(config: &Path, cache: &mut scan::Cache) -> Result<suggest::Report> {
    let project = Project::open(config)?;
    let analysis = project.analyze(cache)?;
    Ok(suggest::Report {
        suggestions: suggest::suggest(
            project.config(),
            &analysis.map,
            &analysis.component_deps,
            &analysis.deps,
            &analysis.files,
            &analysis.unindexed,
            &diagram::unused_arrows(&project.policy, &analysis.component_deps)?,
        ),
        evidence_gaps: report::graph_gaps(&analysis.stats, &analysis.unindexed),
        accepted_gaps: analysis.accepted,
        cached: report::cache_note(&analysis.stats),
    })
}

/// A patch of the config, with what it leaves alone and why.
pub struct Patched {
    pub diff: String,
    pub notes: Vec<String>,
}

/// The patch that drops the `may_depend_on` entries no import uses.
pub fn suggest_patch(config: &Path, cache: &mut scan::Cache) -> Result<Patched> {
    let project = Project::open(config)?;
    let analysis = project.analyze(cache)?;
    let text = &project.policy.config_input().content;
    let gone = suggest::removable(
        project.config(),
        &analysis.map,
        &analysis.component_deps,
        &analysis.unindexed,
    );
    let (gone, held) = patch::by_name(project.config(), gone);
    let (new, skipped) = patch::remove_allowances(text, &gone);
    // Named by its path in the project, not as the caller wrote it, so that
    // `git apply` in the root of the project takes it.
    let name = explain::root_relative(&project.root, &project.policy.config_input().path);
    Ok(Patched {
        diff: patch::unified_diff(&name, text, &new),
        notes: held
            .into_iter()
            .chain(skipped)
            .chain(report::cache_note(&analysis.stats))
            .collect(),
    })
}

/// The answer of `easement doctor`: each indexer the project needs, and whether it can run.
pub struct Diagnosed(pub Vec<indexer::Diagnosis>);

impl Diagnosed {
    pub fn text(&self) -> String {
        doctor::text(&self.0)
    }

    pub fn json(&self) -> String {
        doctor::json(&self.0)
    }

    pub fn outcome(&self) -> Outcome {
        Outcome::from(doctor::ok(&self.0))
    }
}

/// Whether the indexers the project needs are installed and can run in `environment`.
pub fn doctor(config: &Path, environment: &indexer::Environment) -> Result<Diagnosed> {
    let project = Project::open(config)?;
    let walked = scan::project_files(&project.root, &project.config().scan_policy())?;
    Ok(Diagnosed(indexer::diagnose(
        &project.root,
        &walked.files,
        environment,
    )))
}

/// The answer of `easement doctor --cache`: for each index a run would read,
/// whether it is in the cache, and why not.
pub struct Probed(pub Vec<indexer::Probe>);

impl Probed {
    pub fn text(&self) -> String {
        doctor::cache_text(&self.0)
    }

    pub fn json(&self) -> String {
        doctor::cache_json(&self.0)
    }

    /// `NotCached` while a run would index anything again.
    pub fn outcome(&self) -> Outcome {
        if doctor::all_cached(&self.0) {
            Outcome::Passed
        } else {
            Outcome::NotCached
        }
    }
}

/// Which indexes a run of the project would read from the cache as `indexing` keeps
/// it, by the key the run computes; it reads no index and judges nothing.
pub fn doctor_cache(config: &Path, indexing: &indexer::Indexing) -> Result<Probed> {
    let project = Project::open(config)?;
    let policy = project.config().scan_policy();
    let walked = scan::project_files(&project.root, &policy)?;
    Ok(Probed(indexer::probe(
        &project.root,
        &walked.files,
        &walked.excluded,
        indexing,
        &policy.indexers,
    )))
}

/// What the change to the policy since the merge-base with `base` permits,
/// forbids and accepts, over the code of the working tree.
pub fn review(config: &Path, base: &str, cache: &mut scan::Cache) -> Result<review::Review> {
    let project = Project::open(config)?;
    let merge_base = diff::merge_base(&project.root, base)?;
    let old = Policy::load(&project.policy.config_input().path, Some(&merge_base))
        .context("cannot read the policy at the merge-base")?;
    let keys = |b: Option<Baseline>| b.map(|b| b.violations).unwrap_or_default();
    let old_baseline = keys(Baseline::load_at(
        &project.root,
        &old.config.baseline_file(),
        &merge_base,
    )?);
    let new_baseline = keys(Baseline::load(&project.baseline_path())?);
    let scan = project.scan(cache)?;
    // The scan of the code under the old policy, when it reads the code otherwise.
    let rescanned = old
        .config
        .scan_policy()
        .differences(&project.config().scan_policy());
    let old_scan = if rescanned.is_empty() {
        None
    } else {
        Some(scan::scan(&project.root, &old.config.scan_policy(), cache)?)
    };
    // A diagram either policy cannot read is named by the review, not fatal.
    let old_judgment =
        project.judge_despite_unread_diagram(&old, old_scan.as_ref().unwrap_or(&scan))?;
    // What no indexer read under the old policy and it did not accept: the
    // delta says nothing of it either.
    let old_uncovered: Vec<String> = old_scan
        .as_ref()
        .map_or(&scan.uncovered, |s| &s.uncovered)
        .iter()
        .filter(|f| !old_judgment.map.is_uncovered_ok(f))
        .cloned()
        .collect();
    let old_files: &[String] = old_scan.as_ref().map_or(&[], |s| &s.paths);
    let old_from_cache = old_scan.as_ref().map_or(0, |s| s.from_cache);
    let mut head = project.analysis_despite_unread_diagram(scan)?;
    head.stats.uncovered.extend(old_uncovered);
    head.stats.uncovered.sort();
    head.stats.uncovered.dedup();
    let mut evidence_gaps = report::evidence_gaps(&head.stats);
    for gap in report::evidence_gaps(&old_judgment.stats) {
        if !evidence_gaps.contains(&gap) {
            evidence_gaps.push(gap);
        }
    }
    let files: std::collections::BTreeSet<&str> = head
        .files
        .iter()
        .chain(old_files)
        .map(String::as_str)
        .collect();
    let paths = |policy: &Policy| -> Vec<String> {
        policy
            .inputs
            .iter()
            .map(|i| diff::input_path(i, &project.root))
            .collect()
    };
    let inputs = review::Inputs {
        base: paths(&old),
        head: paths(&project.policy),
        changed: diff::changed_inputs(&project.policy.inputs, &old.inputs, &project.root),
        past_unknown: old
            .inputs
            .iter()
            .filter(|i| i.past_unknown)
            .map(|i| diff::input_path(i, &project.root))
            .collect(),
    };
    let unread_diagrams: Vec<review::UnreadDiagram> = [
        (review::Side::Base, old_judgment.unread_diagram.clone()),
        (review::Side::Head, head.unread_diagram.clone()),
    ]
    .into_iter()
    .filter_map(|(policy, reason)| {
        Some(review::UnreadDiagram {
            policy,
            reason: reason?,
        })
    })
    .collect();
    let base_map = ComponentMap::new(&old.config)?;
    let head_map = ComponentMap::new(project.config())?;
    let sides = review::Sides {
        base: &old,
        head: &project.policy,
        base_map: &base_map,
        head_map: &head_map,
    };
    Ok(review::Review {
        base: base.to_string(),
        merge_base,
        inputs,
        rescanned: (!rescanned.is_empty()).then_some(rescanned),
        components: review::Components::of(&sides),
        ownership: review::Ownership::of(&sides, &files),
        examples: review::Examples::of(&sides),
        evidence_gaps,
        accepted_gaps: head.accepted,
        cached: report::cache_note_of(head.stats.from_cache + old_from_cache),
        permissions: review::permissions(&sides),
        surfaces: review::surfaces(&sides, &head.files),
        constraints: review::constraints(&sides),
        delta: review::Delta::of(
            &sides,
            old_judgment.violations,
            head.violations,
            &new_baseline,
            !unread_diagrams.is_empty(),
        ),
        unread_diagrams,
        debt: review::Debt::of(&sides, &old_baseline, &new_baseline),
    })
}

/// What `easement propose` is asked to judge.
pub enum Proposed {
    /// One dependency: the file that would hold it, and the path or the external
    /// module it would name.
    One {
        source: PathBuf,
        target: Option<PathBuf>,
        external: Option<String>,
    },
    /// The text of a plan: a JSON list of proposals.
    Plan(String),
}

/// The decision on each proposed dependency and, unless `no_graph`, what they
/// do together to the graph of the project.
pub fn propose(
    config: &Path,
    proposed: &Proposed,
    no_graph: bool,
    cache: &mut scan::Cache,
) -> Result<propose::Report> {
    let project = Project::open(config)?;
    let relative = |path: &str| explain::root_relative(&project.root, Path::new(path));
    let proposals = match proposed {
        Proposed::Plan(text) => propose::plan(text, relative)?,
        Proposed::One {
            source,
            target,
            external,
        } => {
            let to = match (target, external) {
                (Some(target), _) => propose::To::Path(relative(&target.to_string_lossy())),
                (None, Some(module)) => propose::To::External(module.clone()),
                (None, None) => {
                    anyhow::bail!("name the target path, or the module with `--external`")
                }
            };
            vec![propose::Proposal {
                from: relative(&source.to_string_lossy()),
                to,
            }]
        }
    };
    let config = project.config();
    let map = ComponentMap::new(config)?;
    let judged = propose::judge(config, &map, &proposals);
    let mut stats = check::Stats {
        std_unknown: propose::std_unknown(&map, &judged),
        ..Default::default()
    };
    let (consequences, narrowed, accepted, cached) = if no_graph {
        let narrowed = vec![report::Narrowing {
            kind: report::NarrowingKind::NoGraph,
            limited_to: "dependency decisions".to_string(),
        }];
        (None, narrowed, Vec::new(), None)
    } else {
        let scan = project.scan(cache)?;
        let graph = propose::Graph {
            policy: &project.policy,
            map: &map,
            deps: &scan.deps,
            files: &scan.all_files,
            flow: scan.reach_flow(config),
        };
        let consequences = propose::consequences(&graph, &proposals)?;
        // What the graph could not read is what its consequences cannot see.
        let accepted = project::graph_unread(config, &map, &scan, &mut stats);
        (
            Some(consequences),
            Vec::new(),
            accepted,
            report::cache_note_of(scan.from_cache),
        )
    };
    Ok(propose::Report {
        notes: propose::notes(config, &map, &proposals),
        proposals,
        judged,
        consequences,
        coverage: report::Coverage::of_check(&stats, narrowed, accepted),
        cached,
    })
}
