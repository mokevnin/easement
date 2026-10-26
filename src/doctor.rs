//! `easement doctor`: which indexers the project needs, and whether they can run.

use serde::Serialize;

use crate::indexer::Diagnosis;

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
        if !d.found {
            out.push_str(&format!(
                "{}: {} not found; install it with `{}`\n",
                d.language, d.binary, d.install
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
        .filter(|d| !d.found)
        .filter_map(|d| d.mise)
        .collect();
    if !missing.is_empty() {
        out.push_str("\nFor your mise.toml (pin a version to keep runs reproducible):\n[tools]\n");
        for tool in missing {
            out.push_str(&format!("\"{tool}\" = \"latest\"\n"));
        }
    }
    out.push_str(if ok(diagnoses) {
        "\neasement doctor: ok\n"
    } else {
        "\neasement doctor: something is missing\n"
    });
    out
}
