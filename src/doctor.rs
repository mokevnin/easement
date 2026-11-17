//! `easement doctor`: which indexers the project needs, and whether they can run;
//! under `--cache`, which of their indexes a run would read from the cache.

use serde::Serialize;

use crate::indexer::{Diagnosis, Probe};
use crate::report;

#[derive(Serialize)]
struct Report<'a> {
    ok: bool,
    languages: &'a [Diagnosis],
}

pub fn ok(diagnoses: &[Diagnosis]) -> bool {
    diagnoses.iter().all(Diagnosis::ok)
}

pub fn json(diagnoses: &[Diagnosis]) -> String {
    let report = Report {
        ok: ok(diagnoses),
        languages: diagnoses,
    };
    serde_json::to_string_pretty(&report).expect("report serializes") + "\n"
}

pub fn text(diagnoses: &[Diagnosis]) -> String {
    if diagnoses.is_empty() {
        return "No source files of a language that needs an indexer.\n".to_string();
    }
    let mut out = String::new();
    for d in diagnoses {
        if let Some(cannot_run) = &d.cannot_run {
            out.push_str(&format!(
                "{}: {} cannot run on {}: {}; a check reports its files as uncovered, so {}\n",
                d.language,
                d.binary,
                cannot_run.platform.name(),
                cannot_run.reason,
                report::CLOSE_CANNOT_RUN
            ));
            continue;
        }
        if !d.found {
            out.push_str(&format!(
                "{}: {} not found; install it with `{}`\n",
                d.language,
                d.binary,
                d.install.unwrap_or_default()
            ));
        } else {
            let version = d.version.as_deref().unwrap_or("version unknown");
            out.push_str(&format!(
                "{}: {} ({version}) at {}{}\n",
                d.language,
                d.binary,
                d.path.as_deref().unwrap_or("?"),
                if d.problems.is_empty() { ", ok" } else { "" }
            ));
        }
        for problem in &d.problems {
            out.push_str(&format!("  problem: {problem}\n"));
        }
    }
    let missing: Vec<&str> = diagnoses
        .iter()
        .filter(|d| !d.found && d.cannot_run.is_none())
        .filter_map(|d| d.mise)
        .collect();
    if !missing.is_empty() {
        out.push_str("\nFor your mise.toml (pin a version to keep runs reproducible):\n[tools]\n");
        for tool in missing {
            out.push_str(&format!("\"{tool}\" = \"latest\"\n"));
        }
    }
    let cannot_run: Vec<&str> = diagnoses
        .iter()
        .filter(|d| d.cannot_run.is_some())
        .map(|d| d.language)
        .collect();
    out.push_str(&if !ok(diagnoses) {
        "\neasement doctor: something is missing\n".to_string()
    } else if cannot_run.is_empty() {
        "\neasement doctor: ok\n".to_string()
    } else {
        format!(
            "\neasement doctor: ok, but the {} files will be uncovered here\n",
            crate::check::in_words(&cannot_run)
        )
    });
    out
}

#[derive(Serialize)]
struct CacheReport<'a> {
    /// Whether every index is in the cache, so that a run indexes nothing.
    cached: bool,
    indexes: &'a [Probe],
}

/// Whether every index is in the cache, so that a run indexes nothing.
pub fn all_cached(probes: &[Probe]) -> bool {
    probes.iter().all(|p| p.cached)
}

pub fn cache_json(probes: &[Probe]) -> String {
    let report = CacheReport {
        cached: all_cached(probes),
        indexes: probes,
    };
    serde_json::to_string_pretty(&report).expect("report serializes") + "\n"
}

pub fn cache_text(probes: &[Probe]) -> String {
    if probes.is_empty() {
        return "No source files of a language that needs an indexer.\n".to_string();
    }
    let mut out = String::new();
    for p in probes {
        match p.reason {
            None => out.push_str(&format!("{} ({}): cached\n", p.language, p.dir)),
            Some(reason) => out.push_str(&format!(
                "{} ({}): would run: {}\n",
                p.language,
                p.dir,
                reason.text()
            )),
        }
    }
    let missing = probes.iter().filter(|p| !p.cached).count();
    out.push_str(&if all_cached(probes) {
        "\neasement doctor --cache: every index is cached, so a run indexes nothing\n".to_string()
    } else {
        format!(
            "\neasement doctor --cache: {missing} index(es) not cached; a run indexes them, so set up the toolchains of their indexers first\n"
        )
    });
    out
}
