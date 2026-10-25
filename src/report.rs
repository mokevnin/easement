use serde::Serialize;

use crate::baseline::Verdict;
use crate::check::{Key, Stats, Violation};

#[derive(Serialize)]
struct JsonReport<'a> {
    passed: bool,
    violations: &'a [Violation],
    stale: &'a [Key],
    legalized: usize,
    stats: &'a Stats,
}

pub fn json(verdict: &Verdict, stats: &Stats) -> String {
    let report = JsonReport {
        passed: verdict.passed(),
        violations: &verdict.new,
        stale: &verdict.stale,
        legalized: verdict.legalized,
        stats,
    };
    serde_json::to_string_pretty(&report).expect("report serializes")
}

pub fn text(verdict: &Verdict, stats: &Stats) -> String {
    let mut out = String::new();
    for v in &verdict.new {
        out.push_str(&format!(
            "{}:{}: {} -> {} is not allowed (imports {})\n",
            v.file, v.line, v.from, v.to, v.specifier
        ));
    }
    if !verdict.stale.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("Stale baseline entries (fixed, run `easement baseline` to shrink it):\n");
        for k in &verdict.stale {
            out.push_str(&format!(
                "  {}: {} -> {} ({})\n",
                k.file, k.from, k.to, k.target
            ));
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    let status = if verdict.passed() { "passed" } else { "failed" };
    out.push_str(&format!(
        "easement {status}: {} violation(s), {} stale, {} legalized; {} files, {} internal deps",
        verdict.new.len(),
        verdict.stale.len(),
        verdict.legalized,
        stats.files,
        stats.internal_deps,
    ));
    if stats.unresolved_deps > 0 {
        out.push_str(&format!(", {} unresolved", stats.unresolved_deps));
    }
    if stats.unmapped_files > 0 {
        out.push_str(&format!(
            ", {} files outside components",
            stats.unmapped_files
        ));
    }
    out.push('\n');
    out
}
