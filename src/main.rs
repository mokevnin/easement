mod baseline;
mod chains;
mod check;
mod components;
mod config;
mod decision;
mod diagram;
mod diff;
mod doctor;
mod examples;
mod explain;
mod flow;
mod graph;
mod impact;
mod indexer;
mod init;
mod lang;
mod manifests;
mod matching;
mod packages;
mod patch;
mod policy;
mod project;
mod propose;
mod relpath;
mod report;
mod review;
mod scan;
mod scip;
mod scope;
mod stdlib;
mod suggest;
mod suppress;
mod symbols;
mod unused;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{ColorChoice, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use baseline::{Baseline, Verdict};
use components::ComponentMap;
use policy::Policy;
use project::{Analysis, Project};

/// Architecture quality gate: fails when code dependencies cross component
/// boundaries that easement.yml does not allow.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// When to colour the output. `auto` colours a terminal and honours
    /// NO_COLOR and CLICOLOR_FORCE.
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,
    /// How many languages to index at once: a number, or `cpus` for one per
    /// processor. Indexers can take gigabytes each, so the default is one.
    #[arg(long, global = true, value_name = "N|cpus", default_value = "1")]
    jobs: indexer::Jobs,
    #[command(subcommand)]
    command: Command,
}

/// How a command takes the indexes: from the cache, never from it, or only from it.
#[derive(clap::Args, Debug, Clone, Copy)]
struct CacheFlags {
    /// Run the indexers even when an index of the unchanged project is cached.
    #[arg(long, conflicts_with = "cache_only")]
    no_cache: bool,
    /// Run no indexer: read every index from the cache, and exit 3 when one is not
    /// cached. For CI, to set up an indexer's toolchain only when it has to run.
    #[arg(long)]
    cache_only: bool,
}

impl CacheFlags {
    fn cache_use(self) -> indexer::CacheUse {
        indexer::CacheUse::of_flags(self.no_cache, self.cache_only)
    }
}

#[derive(Subcommand)]
enum Command {
    /// Check the project. Exit code 0: passed, 1: violations, 2: error, 3: an
    /// index not cached under `--cache-only`, 4: evidence required.
    Check {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Ignore the baseline and report every violation.
        #[arg(long)]
        no_baseline: bool,
        /// Judge only imports on lines added since the merge-base with this
        /// revision (e.g. `origin/main`), including uncommitted changes, and
        /// what a change to the policy since then causes.
        #[arg(long, value_name = "BASE")]
        diff: Option<String>,
        /// Check again whenever a file changes, re-reading only what changed.
        #[arg(long)]
        watch: bool,
        /// Report only on the files of this component; repeat for several.
        #[arg(long, value_name = "NAME")]
        component: Vec<String>,
        /// Report only on files under this path (a glob, relative to the config); repeat for several.
        #[arg(long, value_name = "PATH")]
        scope: Vec<String>,
        #[command(flatten)]
        cache: CacheFlags,
        /// Fail a run without violations, with exit code 4, when its verdict rests
        /// on an evidence gap: code the check could not read or judge. Narrowing
        /// (`--diff`, `--component`, `--scope`), `uncovered_ok` and the cache never do.
        #[arg(long)]
        require_evidence: bool,
    },
    /// Record current violations as legalized debt in the baseline file. Once
    /// the file exists, it only shrinks: new violations are listed, not written.
    Baseline {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        /// Write new violations too, legalizing new debt. A human decision.
        #[arg(long)]
        accept_new: bool,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Write a starting easement.yml from the layout and imports of the project.
    Init {
        /// Overwrite an existing easement.yml.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Show which component a file belongs to and what that component may do:
    /// each grant and prohibition, with where the policy writes it under `--format json`.
    Explain {
        /// File or directory, relative to the working directory or to the config.
        path: PathBuf,
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        /// `json` answers for a path no component owns too, with its status.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Draw the components and the dependencies between them.
    Graph {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = graph::GraphFormat::Mermaid)]
        format: graph::GraphFormat,
        /// Draw only this component and the ones it shares an arrow with.
        #[arg(long)]
        focus: Option<String>,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// List every file that depends on the given files, directly or through
    /// others: what a change can break, and which tests to run.
    Impact {
        /// Files or directories, relative to the working directory or to the config.
        paths: Vec<PathBuf>,
        /// Take the files changed since the merge-base with this branch too.
        #[arg(long, value_name = "BASE")]
        diff: Option<String>,
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Show why one component depends on another: whether the config allows it,
    /// and the imports behind it, or the shortest chain of imports.
    Why {
        from: String,
        to: String,
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Say what the current dependency graph suggests for the config: allowances
    /// nothing uses, imports nothing allows, instances that could be isolated.
    Suggest {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        /// Print a patch for the config instead: it drops the `may_depend_on` entries no
        /// import uses (never one next to files no indexer covered), and leaves
        /// comments and reasons alone. Apply it with `git apply` or `patch -p1`.
        #[arg(long)]
        patch: bool,
        /// Exit 1 when `may_depend_on` has entries no import uses (the ones `--patch` would
        /// drop), so that CI keeps the allowances from piling up. Entries next to files no
        /// indexer covered are only reported, never fail it.
        #[arg(long, conflicts_with = "patch")]
        check: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text, conflicts_with = "patch")]
        format: OutputFormat,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Say what a change to the policy since the merge-base with a base revision
    /// permits, forbids and accepts, over the current code. Informs, never fails:
    /// gating stays with `check`.
    Review {
        /// The revision to compare with; the policy at its merge-base with HEAD is the old one.
        #[arg(long, default_value = "origin/main")]
        base: String,
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Judge dependencies before their code exists: the decision on each, and the
    /// cycles, reach chains, crossed limits and missing diagram arrows they cause
    /// together. The baseline and suppressions never apply. Exit code 0: every
    /// decision and consequence passes, 1: one fails, 2: error, 3: an index not
    /// cached under `--cache-only`.
    Propose {
        /// The file that would hold the import; it need not exist.
        #[arg(required_unless_present = "plan", conflicts_with = "plan")]
        source: Option<PathBuf>,
        /// The path of the project it would import; it need not exist.
        #[arg(required_unless_present_any = ["external", "plan"], conflicts_with_all = ["external", "plan"])]
        target: Option<PathBuf>,
        /// The external module it would import, instead of a path.
        #[arg(long, value_name = "MODULE", conflicts_with = "plan")]
        external: Option<String>,
        /// Several proposals, judged together, from a file or `-` for stdin: a JSON
        /// list of {"from": <path>, "to": <path>} and {"from": <path>, "external": <module>}.
        #[arg(long, value_name = "FILE|-")]
        plan: Option<PathBuf>,
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Judge only the decision of each proposal, not what they do to the
        /// graph; the project is not scanned.
        #[arg(long)]
        no_graph: bool,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Check that the indexers the project needs are installed and can run.
    Doctor {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Print the JSON Schema of easement.yml.
    Schema,
    /// Print a shell completion script, e.g. `easement completions zsh > _easement`.
    Completions { shell: Shell },
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    /// SARIF 2.1.0, for GitHub Code Scanning.
    Sarif,
    /// GitHub Actions annotations.
    Github,
    /// JUnit XML.
    Junit,
}

struct CheckOptions {
    config: PathBuf,
    format: Format,
    no_baseline: bool,
    diff: Option<String>,
    components: Vec<String>,
    paths: Vec<String>,
    index_cache: indexer::CacheUse,
    jobs: indexer::Jobs,
    require_evidence: bool,
}

/// How a command ends, which its exit code says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    /// No violations, but `--require-evidence` and an evidence gap.
    EvidenceRequired,
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

fn check(options: &CheckOptions, cache: &mut scan::Cache) -> Result<Outcome> {
    let project = Project::open(&options.config)?;
    let scope = scope::Scope::new(project.config(), &options.components, &options.paths)?;
    let scan = project.scan(cache)?;
    let diff = options
        .diff
        .as_ref()
        .map(|base| -> Result<diff::Diff> {
            let changes = diff::Changes::collect(&project.root, base)?;
            // The policy at the merge-base, judged on the code scanned now.
            let attribution =
                diff::Attribution::of(&project.policy, &project.root, &changes, |policy| {
                    Ok(project.judge(policy, &scan)?.violations)
                });
            Ok(diff::Diff {
                changes,
                attribution,
            })
        })
        .transpose()?;
    let Analysis {
        violations,
        mut stats,
        map,
        edges,
        accepted,
        ..
    } = project.analysis(scan)?;
    let current = violations.iter().map(check::Violation::key).collect();
    // Cycles and limits come from the whole project, so a violation belongs to
    // the scope when any file behind it does.
    let violations: Vec<_> = violations
        .into_iter()
        .filter(|v| {
            v.rule.is_config_rule() || v.evidence.iter().any(|(f, _)| scope.contains(&map, f))
        })
        .collect();
    if let Some(diff) = &diff {
        stats.new_dependencies =
            check::new_dependencies(&edges, |f, l| diff.changes.contains(f, l));
    }
    let judged = match &diff {
        Some(diff) => violations.into_iter().filter(|v| diff.keeps(v)).collect(),
        None => violations,
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
    verdict.stale.retain(|k| scope.contains(&map, &k.file));
    verdict.sort();
    let diff = diff.as_ref();
    // Judged in full, the change narrows nothing.
    let narrowed = report::narrowed(
        diff.filter(|d| !d.attribution.whole()).map(|d| &d.changes),
        &options.components,
        &options.paths,
    );
    stats.limited_to = report::limited_to(&narrowed);
    let coverage = report::Coverage::of_check(&stats, narrowed, accepted);
    let report = report::Report {
        verdict: &verdict,
        stats: &stats,
        coverage: &coverage,
        diff,
        require_evidence: options.require_evidence,
    };
    match options.format {
        Format::Text => anstream::print!("{}", report.colored()),
        Format::Json => println!("{}", report.json()),
        Format::Sarif => println!("{}", report.sarif()),
        Format::Github => print!("{}", report.github()),
        Format::Junit => print!("{}", report.junit()),
    }
    Ok(if !verdict.passed() {
        Outcome::Failed
    } else if report.evidence_required() {
        Outcome::EvidenceRequired
    } else {
        Outcome::Passed
    })
}

/// What the change to the policy since the merge-base with `base` permits,
/// forbids and accepts, over the code of the working tree.
fn review(config: &Path, base: &str, cache: &mut scan::Cache) -> Result<review::Review> {
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
        // The old code under the old policy: its components are the old ones.
        let map = ComponentMap::new(&old.config)?;
        Some(scan::scan(
            &project.root,
            &old.config.scan_policy(),
            &map,
            cache,
        )?)
    };
    let old_judgment = project.judge(&old, old_scan.as_ref().unwrap_or(&scan))?;
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
    let mut head = project.analysis(scan)?;
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
    let base_map = ComponentMap::new(&old.config)?;
    let head_map = ComponentMap::new(project.config())?;
    let sides = review::Sides {
        base: &old.config,
        head: project.config(),
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
        delta: review::Delta::of(
            &sides,
            old_judgment.violations,
            head.violations,
            &new_baseline,
        ),
        debt: review::Debt::of(&sides, &old_baseline, &new_baseline),
    })
}

/// What `easement propose` was asked.
struct ProposeOptions {
    config: PathBuf,
    source: Option<PathBuf>,
    target: Option<PathBuf>,
    external: Option<String>,
    plan: Option<PathBuf>,
    no_graph: bool,
    index_cache: indexer::CacheUse,
    jobs: indexer::Jobs,
}

/// The decision on each proposed dependency and, unless `--no-graph`, what they
/// do together to the graph of the project.
fn propose(options: &ProposeOptions) -> Result<propose::Report> {
    let project = Project::open(&options.config)?;
    let relative = |path: &str| explain::root_relative(&project.root, Path::new(path));
    let proposals = match (&options.plan, &options.source) {
        (Some(plan), _) => {
            let text = if plan.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin())
                    .context("cannot read the plan from stdin")?
            } else {
                std::fs::read_to_string(plan)
                    .with_context(|| format!("cannot read the plan {}", plan.display()))?
            };
            propose::plan(&text, relative)?
        }
        (None, Some(source)) => {
            let to = match (&options.target, &options.external) {
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
        (None, None) => anyhow::bail!("name the source and the target, or pass `--plan`"),
    };
    let config = project.config();
    let map = ComponentMap::new(config)?;
    let judged = propose::judge(config, &map, &proposals);
    let mut stats = check::Stats {
        std_unknown: propose::std_unknown(&map, &judged),
        ..Default::default()
    };
    let (consequences, narrowed, accepted, cached) = if options.no_graph {
        let narrowed = vec![report::Narrowing {
            kind: report::NarrowingKind::NoGraph,
            limited_to: "dependency decisions".to_string(),
        }];
        (None, narrowed, Vec::new(), None)
    } else {
        let scan = project.scan(&mut scan::Cache::new(options.index_cache, options.jobs))?;
        let graph = propose::Graph {
            policy: &project.policy,
            map: &map,
            deps: &scan.deps,
            files: &scan.all_files,
            flow: scan.flow.as_ref(),
        };
        let consequences = propose::consequences(&graph, &proposals)?;
        // What the graph could not read is what its consequences cannot see.
        let accepted;
        (stats.uncovered, accepted) = map.split_uncovered(&scan.uncovered);
        if config.tests.is_some() {
            stats.parts_partial = project::parts_reasons(&scan.parts_doubts);
        }
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

/// Runs the check, then again after every change of the project. Never returns
/// on its own; errors (a broken config mid-edit) are shown and waited out.
fn watch(options: &CheckOptions) -> Result<bool> {
    let mut cache = scan::Cache::new(options.index_cache, options.jobs);
    let mut last = None;
    loop {
        let stamp = Project::open(&options.config).ok().map(|p| p.fingerprint());
        if last != Some(stamp) {
            last = Some(stamp);
            if std::io::stdout().is_terminal() {
                print!("\x1b[2J\x1b[H");
            }
            if let Err(err) = check(options, &mut cache) {
                eprintln!("easement: error: {err:#}");
            }
            println!("easement: watching for changes, Ctrl-C to stop");
            std::io::stdout().flush()?;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn run(cli: Cli) -> Result<Outcome> {
    let jobs = cli.jobs;
    match cli.command {
        Command::Check {
            config,
            format,
            no_baseline,
            diff,
            watch: watching,
            component,
            scope,
            cache,
            require_evidence,
        } => {
            let index_cache = cache.cache_use();
            let options = CheckOptions {
                config,
                format,
                no_baseline,
                diff,
                components: component,
                paths: scope,
                index_cache,
                jobs,
                require_evidence,
            };
            return if watching {
                watch(&options).map(Outcome::from)
            } else {
                check(
                    &options,
                    &mut scan::Cache::new(options.index_cache, options.jobs),
                )
            };
        }
        Command::Baseline {
            config,
            accept_new,
            cache,
        } => {
            let project = Project::open(&config)?;
            let analysis = project.analyze(&mut scan::Cache::new(cache.cache_use(), jobs))?;
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
            println!(
                "easement: {} violation(s) written to {}",
                violations.len(),
                path.display()
            );
            if let Some(note) = report::cache_note(&analysis.stats) {
                println!("easement: {note}");
            }
            if kept > 0 {
                println!(
                    "easement: {kept} entr(ies) kept for files no indexer covered, so their debt is not known to be paid; `easement check` lists those files"
                );
            }
            for v in &refused {
                print!(
                    "{}",
                    report::violation_lines(v, None, Default::default(), false)
                );
            }
            if !refused.is_empty() {
                println!(
                    "easement: {} new violation(s) not written: the baseline only shrinks. Fix them, or legalize them with `easement baseline --accept-new` if a human agrees",
                    refused.len()
                );
            }
            if !config_rules.is_empty() {
                println!(
                    "easement: {} violation(s) of the config left out, the baseline never accepts them; `easement check` lists them",
                    config_rules.len()
                );
            }
            Ok(refused.is_empty())
        }
        Command::Impact {
            paths,
            diff,
            config,
            format,
            cache,
        } => {
            let project = Project::open(&config)?;
            if paths.is_empty() && diff.is_none() {
                anyhow::bail!("name the changed files or directories, or pass `--diff <base>`");
            }
            let analysis = project.analyze(&mut scan::Cache::new(cache.cache_use(), jobs))?;
            let named: Vec<String> = paths
                .iter()
                .map(|p| explain::root_relative(&project.root, p))
                .collect();
            let mut changed = impact::files_under(&named, &analysis.all_files)?;
            let mut deleted = Vec::new();
            if let Some(base) = &diff {
                let changes = diff::Changes::collect(&project.root, base)?;
                let known: std::collections::HashSet<&str> =
                    analysis.all_files.iter().map(String::as_str).collect();
                changed.extend(
                    changes
                        .files()
                        .filter(|f| known.contains(f))
                        .map(str::to_string),
                );
                deleted = changes.deleted;
            }
            let mut result = impact::impact(
                &analysis.map,
                &analysis.deps,
                &analysis.all_files,
                changed,
                deleted,
                report::graph_gaps(&analysis.stats, &analysis.unindexed),
                analysis.accepted,
            );
            result.cached = report::cache_note(&analysis.stats);
            match format {
                OutputFormat::Text => print!("{}", impact::render(&result, &analysis.map)),
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
            }
            Ok(true)
        }
        Command::Why {
            from,
            to,
            config,
            format,
            cache,
        } => {
            let project = Project::open(&config)?;
            let analysis = project.analyze(&mut scan::Cache::new(cache.cache_use(), jobs))?;
            let evidence_gaps = report::graph_gaps(&analysis.stats, &analysis.unindexed);
            let graph = impact::Graph {
                map: &analysis.map,
                deps: &analysis.deps,
                files: &analysis.all_files,
                violations: &analysis.violations,
                evidence_gaps: &evidence_gaps,
                accepted: &analysis.accepted,
            };
            let mut why = impact::why(&project.policy, &graph, &from, &to)?;
            why.cached = report::cache_note(&analysis.stats);
            match format {
                OutputFormat::Text => print!("{}", impact::render_why(&why)),
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&why)?),
            }
            Ok(true)
        }
        Command::Init { force, cache } => {
            let root = std::env::current_dir()?;
            println!("{}", init::run(&root, force, cache.cache_use(), jobs)?);
            Ok(true)
        }
        Command::Explain {
            path,
            config,
            format,
        } => {
            let project = Project::open(&config)?;
            let map = ComponentMap::new(project.config())?;
            let rel = explain::root_relative(&project.root, &path);
            let explanation = explain::explain(&project.policy, &map, &rel);
            match format {
                OutputFormat::Text => print!("{}", explain::render(&explanation)?),
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&explanation)?);
                }
            }
            Ok(true)
        }
        Command::Graph {
            config,
            format,
            focus,
            cache,
        } => {
            let project = Project::open(&config)?;
            if let Some(name) = &focus {
                project.config().require_component(name)?;
            }
            let analysis = project.analyze(&mut scan::Cache::new(cache.cache_use(), jobs))?;
            // On stderr: stdout is the diagram itself.
            for gap in report::evidence_gaps(&analysis.stats) {
                eprintln!("easement: partial: {}", gap.message);
            }
            if let Some(note) = report::cache_note(&analysis.stats) {
                eprintln!("easement: {note}");
            }
            print!(
                "{}",
                graph::render(
                    format,
                    project.config(),
                    &analysis.edges,
                    &analysis.violations,
                    focus.as_deref(),
                )
            );
            Ok(true)
        }
        Command::Suggest {
            config,
            patch,
            check,
            cache,
            format,
        } => {
            let project = Project::open(&config)?;
            let analysis = project.analyze(&mut scan::Cache::new(cache.cache_use(), jobs))?;
            if patch {
                let text = &project.policy.config_input().content;
                let gone = suggest::removable(
                    project.config(),
                    &analysis.map,
                    &analysis.edges,
                    &analysis.unindexed,
                );
                let (new, skipped) = patch::remove_allowances(text, &gone);
                for note in skipped
                    .into_iter()
                    .chain(report::cache_note(&analysis.stats))
                {
                    eprintln!("easement: {note}");
                }
                let name = config.to_string_lossy().replace('\\', "/");
                print!("{}", patch::unified_diff(&name, text, &new));
                return Ok(Outcome::Passed);
            }
            let found = suggest::Report {
                suggestions: suggest::suggest(
                    project.config(),
                    &analysis.map,
                    &analysis.edges,
                    &analysis.deps,
                    &analysis.files,
                    &analysis.unindexed,
                    &diagram::unused_arrows(&project.policy, &analysis.edges)?,
                ),
                evidence_gaps: report::graph_gaps(&analysis.stats, &analysis.unindexed),
                accepted_gaps: analysis.accepted,
                cached: report::cache_note(&analysis.stats),
            };
            match format {
                OutputFormat::Text => print!("{}", found.text()),
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&found)?),
            }
            Ok(!check || found.suggestions.removable.is_empty())
        }
        Command::Review {
            base,
            config,
            format,
            cache,
        } => {
            let review = review(
                &config,
                &base,
                &mut scan::Cache::new(cache.cache_use(), jobs),
            )?;
            match format {
                OutputFormat::Text => print!("{}", review.text()),
                OutputFormat::Json => println!("{}", review.json()),
            }
            Ok(true)
        }
        Command::Propose {
            source,
            target,
            external,
            plan,
            config,
            format,
            no_graph,
            cache,
        } => {
            let index_cache = cache.cache_use();
            let report = propose(&ProposeOptions {
                config,
                source,
                target,
                external,
                plan,
                no_graph,
                index_cache,
                jobs,
            })?;
            match format {
                OutputFormat::Text => print!("{}", report.text()),
                OutputFormat::Json => println!("{}", report.json()),
            }
            Ok(report.passed())
        }
        Command::Doctor { config, format } => {
            let project = Project::open(&config)?;
            let files = scan::project_files(&project.root, &project.config().scan_policy())?;
            let diagnoses = indexer::diagnose(&project.root, &files);
            print!(
                "{}",
                match format {
                    OutputFormat::Text => doctor::text(&diagnoses),
                    OutputFormat::Json => doctor::json(&diagnoses),
                }
            );
            Ok(doctor::ok(&diagnoses))
        }
        Command::Schema => {
            println!("{}", schema_text());
            Ok(true)
        }
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "easement",
                &mut std::io::stdout(),
            );
            Ok(true)
        }
    }
    .map(Outcome::from)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    anstream::ColorChoice::write_global(match cli.color {
        ColorChoice::Auto => anstream::ColorChoice::Auto,
        ColorChoice::Always => anstream::ColorChoice::Always,
        ColorChoice::Never => anstream::ColorChoice::Never,
    });
    match run(cli) {
        Ok(Outcome::Passed) => ExitCode::SUCCESS,
        Ok(Outcome::Failed) => ExitCode::from(1),
        Ok(Outcome::EvidenceRequired) => ExitCode::from(4),
        Err(err) => {
            let red = anstyle::AnsiColor::Red.on_default().bold();
            anstream::eprintln!("easement: {red}error{red:#}: {err:#}");
            if err.downcast_ref::<indexer::NotCached>().is_some() {
                ExitCode::from(3)
            } else {
                ExitCode::from(2)
            }
        }
    }
}

/// The JSON Schema of easement.yml as `easement schema` prints it and the
/// repository keeps it in `easement.schema.json`.
fn schema_text() -> String {
    serde_json::to_string_pretty(&config::schema()).expect("a schema is JSON")
}

#[cfg(test)]
mod tests {
    /// Editors read `easement.schema.json` from the repository, so it must be
    /// the schema the config types give.
    #[test]
    fn the_schema_file_is_the_generated_schema() {
        let file = include_str!("../easement.schema.json");
        assert!(
            file.trim_end() == super::schema_text(),
            "easement.schema.json is out of date: run `cargo run -q -- schema > easement.schema.json`"
        );
    }
}
