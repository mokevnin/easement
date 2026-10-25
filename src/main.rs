mod baseline;
mod check;
mod components;
mod config;
mod lang;
mod report;
mod scan;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
    },
    /// Record current violations as legalized debt in the baseline file.
    Baseline {
        #[arg(long, short, default_value = config::DEFAULT_CONFIG)]
        config: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
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

    fn violations(&self) -> Result<(Vec<check::Violation>, check::Stats)> {
        let scan = scan::scan(&self.root, &self.config)?;
        let map = ComponentMap::new(&self.config)?;
        let (violations, mut stats) = check::evaluate(&self.config, &map, &scan.deps);
        stats.files = scan.files;
        Ok((violations, stats))
    }
}

fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Check {
            config,
            format,
            no_baseline,
        } => {
            let project = Project::open(&config)?;
            let (violations, stats) = project.violations()?;
            let baseline = if no_baseline {
                None
            } else {
                Baseline::load(&project.baseline_path())?
            };
            let verdict = Verdict::judge(violations, baseline.as_ref());
            match format {
                Format::Text => print!("{}", report::text(&verdict, &stats)),
                Format::Json => println!("{}", report::json(&verdict, &stats)),
            }
            Ok(verdict.passed())
        }
        Command::Baseline { config } => {
            let project = Project::open(&config)?;
            let (violations, _) = project.violations()?;
            let path = project.baseline_path();
            Baseline::from_violations(&violations).save(&path)?;
            println!(
                "easement: {} violation(s) written to {}",
                violations.len(),
                path.display()
            );
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
