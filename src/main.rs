//! The `easement` command line: it parses the arguments, asks the library
//! ([`easement::commands`]) for the answer, prints it and exits with the code its
//! outcome says. What a command decides is decided in the library.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{ColorChoice, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use easement::commands::{self, CheckOptions, Outcome, Proposed};
use easement::{Cache, CacheUse, DEFAULT_CONFIG, Environment, GraphFormat, Indexing, Jobs};

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
    jobs: Jobs,
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
    fn cache_use(self) -> CacheUse {
        CacheUse::of_flags(self.no_cache, self.cache_only)
    }
}

#[derive(Subcommand)]
enum Command {
    /// Check the project. Exit code 0: passed, 1: violations, 2: error, 3: an
    /// index not cached under `--cache-only`, 4: evidence required.
    Check {
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
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
        /// Fail on each `may_depend_on` entry no import uses, the ones `suggest --patch`
        /// would drop, as rule `unused-allowance` at its line; entries next to files no
        /// indexer covered are only listed. Judges the whole project, so not with
        /// `--diff`, `--component` or `--scope`.
        #[arg(long, conflicts_with_all = ["diff", "component", "scope"])]
        suggest: bool,
    },
    /// Record current violations as legalized debt in the baseline file. Once
    /// the file exists, it only shrinks: new violations are listed, not written.
    Baseline {
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
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
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// `json` answers for a path no component owns too, with its status.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Print the policy as loaded: `extends` merged, anchors, globs and sets
    /// expanded, each item with where the policy writes it under `--format json`.
    /// Reads no code and runs no indexer.
    Policy {
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Draw the components and the dependencies between them.
    Graph {
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = GraphFormat::Mermaid)]
        format: GraphFormat,
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
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
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
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        #[command(flatten)]
        cache: CacheFlags,
    },
    /// Say what the current dependency graph suggests for the config: allowances
    /// nothing uses, imports nothing allows, instances that could be isolated.
    Suggest {
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// Print a patch for the config instead: it drops the `may_depend_on` entries no
        /// import uses (never one next to files no indexer covered), and leaves
        /// comments and reasons alone. Apply it with `git apply` or `patch -p1`.
        #[arg(long)]
        patch: bool,
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
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
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
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
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
        #[arg(long, short, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Say instead, for each index, whether a run would read it from the cache, and
        /// why not, without reading an index or running an indexer. Exit code 0: every
        /// index is cached, 3: one is not. For CI, to set up an indexer's toolchain only
        /// when it has to run.
        #[arg(long)]
        cache: bool,
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

/// Prints the report of `check` in `format`, and says how it ends.
fn check(options: &CheckOptions, format: Format, cache: &mut Cache) -> Result<Outcome> {
    let checked = commands::check(options, cache)?;
    let report = checked.report();
    match format {
        Format::Text => anstream::print!("{}", report.colored()),
        Format::Json => println!("{}", report.json()),
        Format::Sarif => println!("{}", report.sarif()),
        Format::Github => print!("{}", report.github()),
        Format::Junit => print!("{}", report.junit()),
    }
    Ok(checked.outcome())
}

/// Runs the check, then again after every change of the project. Never returns
/// on its own; errors (a broken config mid-edit) are shown and waited out.
fn watch(options: &CheckOptions, format: Format, mut cache: Cache) -> Result<bool> {
    let mut last = None;
    loop {
        let stamp = commands::fingerprint(&options.config);
        if last != Some(stamp) {
            last = Some(stamp);
            if std::io::stdout().is_terminal() {
                print!("\x1b[2J\x1b[H");
            }
            if let Err(err) = check(options, format, &mut cache) {
                eprintln!("easement: error: {err:#}");
            }
            println!("easement: watching for changes, Ctrl-C to stop");
            std::io::stdout().flush()?;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Prints `text` or `json` as `format` asks.
fn print_as(
    format: OutputFormat,
    text: impl FnOnce() -> Result<String>,
    json: impl FnOnce() -> Result<String>,
) -> Result<()> {
    match format {
        OutputFormat::Text => print!("{}", text()?),
        OutputFormat::Json => println!("{}", json()?),
    }
    Ok(())
}

fn run(cli: Cli) -> Result<Outcome> {
    let jobs = cli.jobs;
    // The one read of the environment: the indexers run in it, and the
    // default cache directory comes from it.
    let environment = Environment::of_process();
    let cache = |flags: CacheFlags| {
        Cache::new(Indexing {
            cache: flags.cache_use(),
            cache_dir: easement::default_cache_dir(&environment),
            environment: environment.clone(),
            jobs,
        })
    };
    match cli.command {
        Command::Check {
            config,
            format,
            no_baseline,
            diff,
            watch: watching,
            component,
            scope,
            cache: flags,
            require_evidence,
            suggest,
        } => {
            let options = CheckOptions {
                config,
                no_baseline,
                diff,
                components: component,
                paths: scope,
                require_evidence,
                suggest,
            };
            if watching {
                watch(&options, format, cache(flags)).map(Outcome::from)
            } else {
                check(&options, format, &mut cache(flags))
            }
        }
        Command::Baseline {
            config,
            accept_new,
            cache: flags,
        } => {
            let recorded = commands::baseline(&config, accept_new, &mut cache(flags))?;
            print!("{}", recorded.text());
            Ok(recorded.outcome())
        }
        Command::Impact {
            paths,
            diff,
            config,
            format,
            cache: flags,
        } => {
            let impacted = commands::impact(&config, &paths, diff.as_deref(), &mut cache(flags))?;
            print_as(format, || Ok(impacted.text()), || impacted.json())?;
            Ok(Outcome::Passed)
        }
        Command::Why {
            from,
            to,
            config,
            format,
            cache: flags,
        } => {
            let why = commands::why(&config, &from, &to, &mut cache(flags))?;
            print_as(
                format,
                || Ok(why.text()),
                || Ok(serde_json::to_string_pretty(&why)?),
            )?;
            Ok(Outcome::Passed)
        }
        Command::Init {
            force,
            cache: flags,
        } => {
            let root = std::env::current_dir()?;
            println!("{}", commands::init(&root, force, &mut cache(flags))?);
            Ok(Outcome::Passed)
        }
        Command::Explain {
            path,
            config,
            format,
        } => {
            let explanation = commands::explain(&config, &path)?;
            print_as(
                format,
                || explanation.text(),
                || Ok(serde_json::to_string_pretty(&explanation)?),
            )?;
            Ok(Outcome::Passed)
        }
        Command::Policy { config, format } => {
            let policy = commands::policy(&config)?;
            print_as(
                format,
                || Ok(policy.text()),
                || Ok(serde_json::to_string_pretty(&policy)?),
            )?;
            Ok(Outcome::Passed)
        }
        Command::Graph {
            config,
            format,
            focus,
            cache: flags,
        } => {
            let drawn = commands::graph(&config, format, focus.as_deref(), &mut cache(flags))?;
            // On stderr: stdout is the diagram itself.
            for note in &drawn.notes {
                eprintln!("easement: {note}");
            }
            print!("{}", drawn.diagram);
            Ok(Outcome::Passed)
        }
        Command::Suggest {
            config,
            patch,
            cache: flags,
            format,
        } => {
            if patch {
                let patched = commands::suggest_patch(&config, &mut cache(flags))?;
                for note in &patched.notes {
                    eprintln!("easement: {note}");
                }
                print!("{}", patched.diff);
                return Ok(Outcome::Passed);
            }
            let found = commands::suggest(&config, &mut cache(flags))?;
            print_as(
                format,
                || Ok(found.text()),
                || Ok(serde_json::to_string_pretty(&found)?),
            )?;
            Ok(Outcome::Passed)
        }
        Command::Review {
            base,
            config,
            format,
            cache: flags,
        } => {
            let review = commands::review(&config, &base, &mut cache(flags))?;
            print_as(format, || Ok(review.text()), || Ok(review.json()))?;
            Ok(Outcome::Passed)
        }
        Command::Propose {
            source,
            target,
            external,
            plan,
            config,
            format,
            no_graph,
            cache: flags,
        } => {
            let proposed = match (plan, source) {
                (Some(plan), _) => Proposed::Plan(if plan.as_os_str() == "-" {
                    std::io::read_to_string(std::io::stdin())
                        .context("cannot read the plan from stdin")?
                } else {
                    std::fs::read_to_string(&plan)
                        .with_context(|| format!("cannot read the plan {}", plan.display()))?
                }),
                (None, Some(source)) => Proposed::One {
                    source,
                    target,
                    external,
                },
                (None, None) => anyhow::bail!("name the source and the target, or pass `--plan`"),
            };
            let report = commands::propose(&config, &proposed, no_graph, &mut cache(flags))?;
            print_as(format, || Ok(report.text()), || Ok(report.json()))?;
            Ok(Outcome::from(report.passed()))
        }
        Command::Doctor {
            config,
            format,
            cache: true,
        } => {
            let indexing = Indexing {
                cache: CacheUse::On,
                cache_dir: easement::default_cache_dir(&environment),
                environment,
                jobs,
            };
            let probed = commands::doctor_cache(&config, &indexing)?;
            print!(
                "{}",
                match format {
                    OutputFormat::Text => probed.text(),
                    OutputFormat::Json => probed.json(),
                }
            );
            Ok(probed.outcome())
        }
        Command::Doctor {
            config,
            format,
            cache: false,
        } => {
            let diagnosed = commands::doctor(&config, &environment)?;
            print!(
                "{}",
                match format {
                    OutputFormat::Text => diagnosed.text(),
                    OutputFormat::Json => diagnosed.json(),
                }
            );
            Ok(diagnosed.outcome())
        }
        Command::Schema => {
            println!("{}", easement::schema_text());
            Ok(Outcome::Passed)
        }
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "easement",
                &mut std::io::stdout(),
            );
            Ok(Outcome::Passed)
        }
    }
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
        Ok(Outcome::NotCached) => ExitCode::from(3),
        Err(err) => {
            let red = anstyle::AnsiColor::Red.on_default().bold();
            anstream::eprintln!("easement: {red}error{red:#}: {err:#}");
            if err.downcast_ref::<easement::NotCached>().is_some() {
                ExitCode::from(3)
            } else {
                ExitCode::from(2)
            }
        }
    }
}
