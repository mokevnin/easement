//! The report of `check` in the formats a CI reads: GitHub annotations,
//! SARIF and JUnit.

use easement::CacheUse;
use easement::commands::{self, CheckOptions, Outcome};
use tempfile::TempDir;

use crate::common::fixture;
use crate::run::{cache_in, check, config, parse, pinned};

#[test]
fn the_report_reads_in_every_ci_format() {
    let (_tmp, root) = fixture("policy");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = checked.report();

    let github = report.github();
    assert!(
        github.contains(
            "::error file=src/web/page.ts,line=2,title=easement dependency::web -> data is forbidden"
        ),
        "{github}"
    );

    let sarif = parse(&report.sarif());
    assert_eq!(sarif["version"], "2.1.0");
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 6);
    assert!(results.iter().any(|r| {
        r["ruleId"] == "orphan"
            && r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"] == "src/stray.ts"
    }));

    let junit = report.junit();
    assert!(junit.contains(
        "<testsuite name=\"easement\" tests=\"6\" skipped=\"0\" errors=\"0\" failures=\"6\">"
    ));
    assert!(junit.contains("type=\"external\""));
}

/// The JUnit XML a CI reads, as it is written: the suite, a failed case per
/// violation, and how each failure is told.
#[test]
fn the_junit_report_is_pinned() {
    let (_tmp, root) = fixture("policy");
    // Indexed afresh, so that no other run decides whether the report says it is cached.
    let own = TempDir::new().unwrap();
    let mut fresh = cache_in(own.path(), CacheUse::Off);
    let checked = commands::check(&CheckOptions::new(config(&root)), &mut fresh).unwrap();
    let junit = checked.report().junit();
    pinned(|| insta::assert_snapshot!("junit_report", junit));
}
