mod baseline;
mod check;
mod components;
mod config;
mod diff;
mod explain;
mod graph;
mod init;
mod lang;
mod report;
mod scan;
mod scope;
mod suggest;
mod suppress;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use baseline::{Baseline, Verdict};
use components::ComponentMap;
use config::Config;

/// Architecture quality gate: fails when code dependencies cross component
/// boundaries that easement.yml does not allow.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check the project. Exit code 0: passed, 1: violations, 2: error.
    Check {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Ignore the baseline and report every violation.
        #[arg(long)]
        no_baseline: bool,
        /// Judge only imports on lines added since the merge-base with this
        /// revision (e.g. `origin/main`), including uncommitted changes.
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
    },
    /// Record current violations as legalized debt in the baseline file.
    Baseline {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Write a starting easement.yml from the layout and imports of the project.
    Init {
        /// Overwrite an existing easement.yml.
        #[arg(long)]
        force: bool,
    },
    /// Show which component a file belongs to and what that component may do.
    Explain {
        /// File or directory, relative to the working directory or to the config.
        path: PathBuf,
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Draw the components and the dependencies between them.
    Graph {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = graph::GraphFormat::Mermaid)]
        format: graph::GraphFormat,
    },
    /// Say what the current dependency graph suggests for the config: allowances
    /// nothing uses, imports nothing allows, instances that could be isolated.
    Suggest {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Print the JSON Schema of easement.yml.
    Schema,
}

const SCHEMA: &str = include_str!("../easement.schema.json");

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

struct Project {
    root: PathBuf,
    config: Config,
}

impl Project {
    fn open(config_path: &Path) -> Result<Self> {
        let config = Config::load(config_path)?;
        let config_path = dunce::canonicalize(config_path)
            .with_context(|| format!("cannot resolve {}", config_path.display()))?;
        let root = config_path
            .parent()
            .context("config has no parent directory")?
            .to_path_buf();
        Ok(Self { root, config })
    }

    fn baseline_path(&self) -> PathBuf {
        let rel = self
            .config
            .baseline
            .clone()
            .unwrap_or_else(|| config::DEFAULT_BASELINE.into());
        self.root.join(rel)
    }

    fn analyze(&self, cache: &mut scan::Cache, scope: &scope::Scope) -> Result<Analysis> {
        let scan = scan::scan(&self.root, &self.config, cache)?;
        let map = ComponentMap::new(&self.config)?;
        let (mut violations, edges, mut stats) =
            check::evaluate(&self.config, &map, scope, &scan.deps);
        violations.extend(check::symbols(&self.config, &map, &scan.symbols));
        let (mut violations, suppressed) = suppress::apply(violations, &scan.suppressions);
        violations.extend(check::cycles(&self.config, &edges));
        violations.extend(check::metrics(&self.config, &edges));
        violations.extend(check::orphans(&self.config, &map, &scan.paths));
        violations.extend(check::forbidden_files(&self.config, &map, &scan.all_files));
        violations.extend(check::required(
            &self.root,
            &self.config,
            &map,
            &scan.deps,
            &scan.paths,
            &scan.all_files,
        ));
        stats.files = scan
            .paths
            .iter()
            .filter(|f| scope.contains(&map, f))
            .count();
        stats.suppressed = suppressed;
        stats.dynamic = scan
            .dynamic
            .into_iter()
            .filter(|d| scope.contains(&map, d.rsplit_once(':').map_or(d, |(f, _)| f)))
            .collect();
        Ok(Analysis {
            violations,
            edges,
            deps: scan.deps,
            files: scan.paths,
            stats,
            map,
        })
    }

    /// Everything that changes the verdict: the files of the project, the
    /// config and the baseline, as a hash of their names, sizes and times.
    fn fingerprint(&self, config_path: &Path) -> u64 {
        let mut hasher = DefaultHasher::new();
        let mut files: Vec<PathBuf> = scan::walker(&self.root)
            .build()
            .flatten()
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.into_path())
            .collect();
        files.push(config_path.to_path_buf());
        files.push(self.baseline_path());
        files.sort();
        files.dedup();
        for file in files {
            if let Ok(meta) = std::fs::metadata(&file) {
                (&file, meta.len(), meta.modified().ok()).hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

struct Analysis {
    violations: Vec<check::Violation>,
    edges: Vec<check::Edge>,
    deps: Vec<scan::Dep>,
    files: Vec<String>,
    stats: check::Stats,
    map: ComponentMap,
}

struct CheckOptions {
    config: PathBuf,
    format: Format,
    no_baseline: bool,
    diff: Option<String>,
    components: Vec<String>,
    paths: Vec<String>,
}

fn check(options: &CheckOptions, cache: &mut scan::Cache) -> Result<bool> {
    let project = Project::open(&options.config)?;
    let scope = scope::Scope::new(&project.config, &options.components, &options.paths)?;
    let Analysis {
        violations,
        mut stats,
        map,
        edges,
        ..
    } = project.analyze(cache, &scope)?;
    let current = violations.iter().map(check::Violation::key).collect();
    // Cycles and limits come from the whole project, so a violation belongs to
    // the scope when any file behind it does.
    let violations: Vec<_> = violations
        .into_iter()
        .filter(|v| v.evidence.iter().any(|(f, _)| scope.contains(&map, f)))
        .collect();
    let changes = options
        .diff
        .as_ref()
        .map(|base| diff::Changes::collect(&project.root, base))
        .transpose()?;
    if let Some(changes) = &changes {
        stats.new_dependencies = check::new_dependencies(&edges, |f, l| changes.contains(f, l));
    }
    let judged = match &changes {
        Some(changes) => violations
            .into_iter()
            .filter(|v| v.evidence.iter().any(|(f, l)| changes.contains(f, *l)))
            .collect(),
        None => violations,
    };
    let baseline = if options.no_baseline {
        None
    } else {
        Baseline::load(&project.baseline_path())?
    };
    let mut verdict = Verdict::judge(check::dedupe(judged), &current, baseline.as_ref());
    verdict.stale.retain(|k| scope.contains(&map, &k.file));
    verdict
        .new
        .sort_by(|a, b| (&a.file, a.line, a.rule).cmp(&(&b.file, b.line, b.rule)));
    let changes = changes.as_ref();
    match options.format {
        Format::Text => print!("{}", report::text(&verdict, &stats, changes)),
        Format::Json => println!("{}", report::json(&verdict, &stats, changes)),
        Format::Sarif => println!("{}", report::sarif(&verdict)),
        Format::Github => print!("{}", report::github(&verdict, &stats)),
        Format::Junit => print!("{}", report::junit(&verdict)),
    }
    Ok(verdict.passed())
}

/// Runs the check, then again after every change of the project. Never returns
/// on its own; errors (a broken config mid-edit) are shown and waited out.
fn watch(options: &CheckOptions) -> Result<bool> {
    let mut cache = scan::Cache::default();
    let mut last = None;
    loop {
        let stamp = Project::open(&options.config)
            .ok()
            .map(|p| p.fingerprint(&options.config));
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

fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Check {
            config,
            format,
            no_baseline,
            diff,
            watch: watching,
            component,
            scope,
        } => {
            let options = CheckOptions {
                config,
                format,
                no_baseline,
                diff,
                components: component,
                paths: scope,
            };
            if watching {
                watch(&options)
            } else {
                check(&options, &mut scan::Cache::default())
            }
        }
        Command::Baseline { config } => {
            let project = Project::open(&config)?;
            let violations = check::dedupe(
                project
                    .analyze(&mut scan::Cache::default(), &scope::Scope::default())?
                    .violations,
            );
            let path = project.baseline_path();
            Baseline::from_violations(&violations).save(&path)?;
            println!(
                "easement: {} violation(s) written to {}",
                violations.len(),
                path.display()
            );
            Ok(true)
        }
        Command::Init { force } => {
            let root = std::env::current_dir()?;
            println!("{}", init::run(&root, force)?);
            Ok(true)
        }
        Command::Explain { path, config } => {
            let project = Project::open(&config)?;
            let map = ComponentMap::new(&project.config)?;
            let rel = explain::root_relative(&project.root, &path);
            print!("{}", explain::explain(&project.config, &map, &rel)?);
            Ok(true)
        }
        Command::Graph { config, format } => {
            let project = Project::open(&config)?;
            let analysis =
                project.analyze(&mut scan::Cache::default(), &scope::Scope::default())?;
            print!(
                "{}",
                graph::render(
                    format,
                    &project.config,
                    &analysis.edges,
                    &analysis.violations
                )
            );
            Ok(true)
        }
        Command::Suggest { config } => {
            let project = Project::open(&config)?;
            let analysis =
                project.analyze(&mut scan::Cache::default(), &scope::Scope::default())?;
            print!(
                "{}",
                suggest::suggest(
                    &project.config,
                    &analysis.map,
                    &analysis.edges,
                    &analysis.deps,
                    &analysis.files
                )
            );
            Ok(true)
        }
        Command::Schema => {
            print!("{SCHEMA}");
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            eprintln!("easement: error: {err:#}");
            ExitCode::from(2)
        }
    }
}
