//! Each command of easement run through the library, as the binary runs it,
//! on scratch projects indexed by the real indexers. A command answers with
//! data; the helpers here only shorten the calls and read the answer.

use std::path::{Path, PathBuf};

use easement::commands::{self, CheckOptions, Checked, Outcome};
use easement::{Cache, CacheUse, Environment, Indexing, Jobs, Platform};

use crate::common::test_cache_dir;

/// A cache that reads and keeps the indexes in the shared test cache.
pub fn cache() -> Cache {
    cache_in(test_cache_dir(), CacheUse::On)
}

/// A cache that reads and keeps the indexes in `dir` as `cache_use` says.
pub fn cache_in(dir: &Path, cache_use: CacheUse) -> Cache {
    Cache::new(indexing_in(dir, cache_use))
}

/// Indexing with the cache in `dir` as `cache_use` says, by indexers that
/// run in the environment of the tests, one language at a time.
pub fn indexing_in(dir: &Path, cache_use: CacheUse) -> Indexing {
    Indexing {
        cache: cache_use,
        cache_dir: Some(dir.to_path_buf()),
        environment: Environment::of_process(),
        jobs: Jobs::default(),
    }
}

/// The policy of the project at `root`: its `easement.yml`.
pub fn config(root: &Path) -> PathBuf {
    root.join("easement.yml")
}

/// The check of the whole project at `root`.
pub fn check(root: &Path) -> Checked {
    check_with(root, |_| {})
}

/// The check of the project at `root` with the options `set` changes.
pub fn check_with(root: &Path, set: impl FnOnce(&mut CheckOptions)) -> Checked {
    try_check_with(root, set).unwrap()
}

/// The check of the project at `root` with the options `set` changes, or the
/// error it stops on.
pub fn try_check_with(root: &Path, set: impl FnOnce(&mut CheckOptions)) -> anyhow::Result<Checked> {
    let mut options = CheckOptions::new(config(root));
    set(&mut options);
    commands::check(&options, &mut cache())
}

/// The report of a check as text, without colour.
pub fn text(checked: &Checked) -> String {
    checked.report().text()
}

/// The report of a check as data.
pub fn json(checked: &Checked) -> serde_json::Value {
    parse(&checked.report().json())
}

/// A JSON answer as data.
pub fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).unwrap()
}

/// The error a command stops on, with its causes, as the binary prints it.
pub fn error<T>(result: anyhow::Result<T>) -> String {
    match result {
        Ok(_) => panic!("the command did not fail"),
        Err(err) => format!("{err:#}"),
    }
}

/// Runs `assert` with its snapshots kept in `tests/snapshots`, beside the
/// others agents read.
pub fn pinned(assert: impl FnOnce()) {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    settings.bind(assert);
}

/// Whether a report says it leaves something out: the run was narrowed, or it
/// has an evidence gap.
pub fn leaves_out(report: &serde_json::Value) -> bool {
    let has = |key: &str| report[key].as_array().is_some_and(|list| !list.is_empty());
    has("narrowed") || has("evidence_gaps")
}

/// The messages of the evidence gaps of `kind` in a report.
pub fn gap_messages(report: &serde_json::Value, kind: &str) -> Vec<serde_json::Value> {
    report["evidence_gaps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|gap| gap["kind"] == kind)
        .map(|gap| gap["message"].clone())
        .collect()
}

/// The report of the check of the project at `root` as data; the check fails.
pub fn failed_json(root: &Path) -> serde_json::Value {
    let checked = check(root);
    let report = json(&checked);
    assert_eq!(checked.outcome(), Outcome::Failed, "{report}");
    report
}

/// The report of the check of the project at `root`, which ends in `outcome`.
pub fn report_of(root: &Path, outcome: Outcome) -> String {
    let checked = check(root);
    let said = text(&checked);
    assert_eq!(checked.outcome(), outcome, "{said}");
    said
}

/// Installs the PHP dependencies of the project at `root`, which scip-php reads.
pub fn composer_install(root: &Path) {
    // `which` finds composer.bat on Windows, which `Command` alone does not.
    let composer = which::which("composer").expect("composer is needed for the PHP indexer");
    let status = std::process::Command::new(composer)
        .args(["install", "--no-interaction", "--quiet"])
        .current_dir(root)
        .status()
        .expect("composer runs");
    assert!(status.success(), "composer install");
}

/// Whether a test of any of `languages` (registry names) skips itself here, saying
/// why: the indexer cannot run on this platform (as the indexer itself says), or
/// scip-ruby, which has no Linux arm64 release (sourcegraph/scip-ruby#228), is
/// missing there. Anywhere else a missing indexer is still an error.
pub fn cannot_index_here(languages: &[&str]) -> bool {
    languages.iter().any(|language| {
        if let Some(reason) = Platform::current().cannot_index(language) {
            eprintln!("skipped: the {language} indexer cannot run here: {reason}");
            return true;
        }
        let no_scip_ruby = *language == "ruby"
            && cfg!(all(target_os = "linux", target_arch = "aarch64"))
            && which::which("scip-ruby").is_err();
        if no_scip_ruby {
            eprintln!("skipped: scip-ruby has no Linux arm64 release (sourcegraph/scip-ruby#228)");
        }
        no_scip_ruby
    })
}

/// The check of the whole project at `root` with the indexes of `dir`, taken as
/// `cache_use` says, or the error it stops on.
pub fn check_in(root: &Path, dir: &Path, cache_use: CacheUse) -> anyhow::Result<Checked> {
    commands::check(
        &CheckOptions::new(config(root)),
        &mut cache_in(dir, cache_use),
    )
}

/// The violations of a report as (file, line, from -> to).
pub fn found(report: &serde_json::Value) -> Vec<(String, u64, String)> {
    report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                format!(
                    "{} -> {}",
                    v["from"].as_str().unwrap(),
                    v["to"].as_str().unwrap()
                ),
            )
        })
        .collect()
}

/// Each violation of `report` as its file, line and rule.
pub fn by_rule(report: &serde_json::Value) -> Vec<(String, u64, String)> {
    report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["rule"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}
