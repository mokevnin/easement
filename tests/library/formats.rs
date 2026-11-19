//! The report of `check` in the formats a CI reads: GitHub annotations,
//! SARIF and JUnit.

use std::fs;

use easement::commands::{self, CheckOptions, Outcome, Proposed};
use easement::{CacheUse, GraphFormat};
use tempfile::TempDir;

use crate::common::{commit_all, fixture};
use crate::run::{cache, cache_in, check, check_with, config, parse, pinned};

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

/// The SARIF notifications of a report, as (descriptor id, level, files).
fn notifications(sarif: &str) -> Vec<(String, String, Vec<String>)> {
    parse(sarif)["runs"][0]["invocations"][0]["toolExecutionNotifications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            let files = n["locations"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| {
                    l["physicalLocation"]["artifactLocation"]["uri"]
                        .as_str()
                        .unwrap()
                        .to_string()
                })
                .collect();
            (
                n["descriptor"]["id"].as_str().unwrap().to_string(),
                n["level"].as_str().unwrap().to_string(),
                files,
            )
        })
        .collect()
}

/// The `uncovered` fixture, its tools accepted in `uncovered_ok` but `except`.
fn accepting_tools(except: &str) -> (TempDir, std::path::PathBuf) {
    let (tmp, root) = fixture("uncovered");
    let policy = fs::read_to_string(config(&root)).unwrap();
    let accepted = ["tools/gen.go", "tools/other.go"]
        .into_iter()
        .filter(|f| *f != except)
        .map(|f| format!("'{f}'"))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        config(&root),
        format!("{policy}uncovered_ok:\n  - {{ name: [{accepted}], because: build helpers }}\n"),
    )
    .unwrap();
    (tmp, root)
}

/// An accepted gap is a note in every CI format, one per `uncovered_ok` entry
/// with its reason, never a failed test case, `--require-evidence` or not.
#[test]
fn an_accepted_gap_is_a_note_in_every_ci_format() {
    let (_tmp, root) = accepting_tools("");
    for require_evidence in [false, true] {
        let checked = check_with(&root, |o| o.require_evidence = require_evidence);
        assert_eq!(checked.outcome(), Outcome::Passed);
        let report = checked.report();

        let accepted: Vec<_> = notifications(&report.sarif())
            .into_iter()
            .filter(|(kind, _, _)| kind == "accepted-gap")
            .collect();
        assert_eq!(
            accepted,
            [(
                "accepted-gap".to_string(),
                "note".to_string(),
                vec!["tools/gen.go".to_string(), "tools/other.go".to_string()],
            )]
        );

        let junit = report.junit();
        assert!(junit.contains("failures=\"0\""), "{junit}");
        assert!(
            junit.contains(
                "accepted gap (uncovered_ok: tools/gen.go, tools/other.go) because build helpers; files: tools/gen.go, tools/other.go"
            ),
            "{junit}"
        );

        let github = report.github();
        let notices: Vec<&str> = github
            .lines()
            .filter(|l| l.contains("title=easement accepted-gap::"))
            .collect();
        assert_eq!(notices.len(), 1, "{github}");
        assert!(notices[0].starts_with("::notice "), "{github}");
        assert!(notices[0].contains("because build helpers"), "{github}");
    }
}

/// A `--diff` run that could not read some code says the two apart: the
/// narrowing is a notice, the evidence gap a warning, or an error when
/// evidence is required.
#[test]
fn a_diff_run_names_the_narrowing_and_the_evidence_gap_apart() {
    let (_tmp, root) = fixture("uncovered");
    commit_all(&root);
    for (require_evidence, gap) in [(false, "warning"), (true, "error")] {
        let checked = check_with(&root, |o| {
            o.diff = Some("main".into());
            o.require_evidence = require_evidence;
        });
        let report = checked.report();
        let json = parse(&report.json());
        assert!(!json["narrowed"].as_array().unwrap().is_empty(), "{json}");
        assert!(
            !json["evidence_gaps"].as_array().unwrap().is_empty(),
            "{json}"
        );

        let github = report.github();
        assert!(
            github.contains(
                "::notice title=easement narrowed::only violations on lines changed since main"
            ),
            "{github}"
        );
        assert!(
            github.contains(&format!(
                "::{gap} title=easement evidence-gap::2 source files were not covered by an indexer"
            )),
            "{github}"
        );

        let kinds: Vec<(String, String)> = notifications(&report.sarif())
            .into_iter()
            // Whether another test indexed the fixture first does not matter here.
            .filter(|(kind, _, _)| kind != "cache")
            .map(|(kind, level, _)| (kind, level))
            .collect();
        assert_eq!(
            kinds,
            [
                ("narrowed".to_string(), "note".to_string()),
                ("evidence-gap".to_string(), gap.to_string()),
            ]
        );
    }
}

/// No output of any command calls what it leaves out `partial`: it names the
/// narrowing, the evidence gaps and the accepted gaps apart.
#[test]
fn no_output_says_partial() {
    // One file accepted, the other an evidence gap, the run narrowed.
    let (_tmp, root) = accepting_tools("tools/other.go");
    commit_all(&root);
    let config = config(&root);
    let mut said = Vec::new();
    for narrow in [
        &(|o: &mut CheckOptions| o.diff = Some("main".into())) as &dyn Fn(&mut CheckOptions),
        &|o| o.components = vec!["app".into()],
        &|o| o.paths = vec!["app".into()],
    ] {
        for require_evidence in [false, true] {
            let checked = check_with(&root, |o| {
                narrow(o);
                o.require_evidence = require_evidence;
            });
            let report = checked.report();
            let json = parse(&report.json());
            assert!(!json["accepted_gaps"].as_array().unwrap().is_empty());
            assert!(!json["evidence_gaps"].as_array().unwrap().is_empty());
            said.extend([
                report.text(),
                report.colored(),
                report.json(),
                report.sarif(),
                report.github(),
                report.junit(),
            ]);
        }
    }
    let drawn = commands::graph(&config, GraphFormat::Mermaid, None, &mut cache()).unwrap();
    said.extend([drawn.diagram.clone(), drawn.notes()]);
    for no_graph in [false, true] {
        let proposed = Proposed::One {
            source: root.join("app/main.go"),
            target: None,
            external: Some("fmt".into()),
        };
        let answer = commands::propose(&config, &proposed, no_graph, &mut cache()).unwrap();
        said.extend([answer.text(), answer.json()]);
    }
    let impacted = commands::impact(&config, &[root.join("app")], None, &mut cache()).unwrap();
    said.extend([impacted.text(), impacted.json().unwrap()]);
    let why = commands::why(&config, "app", "app", &mut cache()).unwrap();
    said.extend([why.text(), serde_json::to_string(&why).unwrap()]);
    said.push(commands::suggest(&config, &mut cache()).unwrap().text());
    let reviewed = commands::review(&config, "main", &mut cache()).unwrap();
    said.extend([reviewed.text(), reviewed.json()]);
    said.push(
        commands::baseline(&config, false, &mut cache())
            .unwrap()
            .text(),
    );
    // Last: it writes a policy of its own over the project's.
    said.push(commands::init(&root, true, &mut cache()).unwrap());
    for out in &said {
        assert!(!out.to_lowercase().contains("partial"), "{out}");
    }
}
