use serde::Serialize;

use crate::baseline::Verdict;
use crate::check::{Key, Stats, Violation};
use crate::diff::Changes;

#[derive(Serialize)]
struct JsonReport<'a> {
    passed: bool,
    violations: &'a [Violation],
    stale: &'a [Key],
    legalized: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<DiffInfo<'a>>,
    stats: &'a Stats,
}

#[derive(Serialize)]
struct DiffInfo<'a> {
    base: &'a str,
    merge_base: &'a str,
}

pub fn json(verdict: &Verdict, stats: &Stats, changes: Option<&Changes>) -> String {
    let report = JsonReport {
        passed: verdict.passed(),
        violations: &verdict.new,
        stale: &verdict.stale,
        legalized: verdict.legalized,
        diff: changes.map(|c| DiffInfo {
            base: &c.base,
            merge_base: &c.merge_base,
        }),
        stats,
    };
    serde_json::to_string_pretty(&report).expect("report serializes")
}

pub fn text(verdict: &Verdict, stats: &Stats, changes: Option<&Changes>) -> String {
    let mut out = String::new();
    if let Some(c) = changes {
        let short = &c.merge_base[..c.merge_base.len().min(12)];
        out.push_str(&format!(
            "Checking lines changed since {} (merge-base {short})\n\n",
            c.base
        ));
    }
    for v in &verdict.new {
        out.push_str(&format!(
            "{}:{}: {} [{}]\n",
            v.file,
            v.line,
            v.message,
            v.rule.as_str()
        ));
        if let Some(because) = &v.because {
            out.push_str(&format!("  because: {because}\n"));
        }
    }
    if !verdict.stale.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("Stale baseline entries (fixed, run `easement baseline` to shrink it):\n");
        for k in &verdict.stale {
            let what = match k.rule {
                crate::check::RuleId::Cycle => format!("cycle among {}", k.from),
                crate::check::RuleId::Dependency | crate::check::RuleId::Privacy => {
                    format!("{}: {} -> {} ({})", k.file, k.from, k.to, k.target)
                }
                _ => format!("{}: {}", k.file, k.target),
            };
            out.push_str(&format!("  [{}] {what}\n", k.rule.as_str()));
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
    if stats.suppressed > 0 {
        out.push_str(&format!(", {} suppressed", stats.suppressed));
    }
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
