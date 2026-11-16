use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use tempfile::TempDir;

mod common;

use common::{WEB_AND_STORAGE, fixture, project, shop, test_cache_dir, web_and_storage};

fn easement(root: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    easement_cached(root, args, test_cache_dir())
}

fn easement_cached(root: &Path, args: &[&str], cache: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("easement")
        .unwrap()
        .current_dir(root)
        .env("EASEMENT_CACHE_DIR", cache)
        .args(args)
        .assert()
}

/// A git hook or `git rebase --exec` runs easement with `GIT_DIR`, `GIT_WORK_TREE`
/// and `GIT_INDEX_FILE` pointing at the repository that runs it. easement asks git
/// about the project it checks, not about the repository the environment names.
#[test]
fn a_run_under_another_repository_reads_the_repository_of_its_project() {
    let (_tmp, root) = web_and_storage(WEB_AND_STORAGE);
    let other = TempDir::new().unwrap();
    common::git(other.path(), &["init", "-q", "-b", "other"]);
    Command::cargo_bin("easement")
        .unwrap()
        .current_dir(&root)
        .env("EASEMENT_CACHE_DIR", test_cache_dir())
        .env("GIT_DIR", other.path().join(".git"))
        .env("GIT_WORK_TREE", other.path())
        .env("GIT_INDEX_FILE", other.path().join(".git/index"))
        .args(["check", "--diff", "main"])
        .assert()
        .success();
}

/// The report is what agents read, so all of it is pinned, not only the parts a test
/// looks for: a field that appears, disappears or changes shows up in the snapshot.
#[test]
fn the_report_of_check_is_pinned() {
    let (_tmp, root) = fixture("go");
    // A cache of its own: the first run indexes, the second reads the cache.
    let cache = TempDir::new().unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout_of(
        easement_cached(&root, &["check", "--format", "json"], cache.path()).code(1),
    ))
    .unwrap();
    insta::assert_json_snapshot!("check_json", report);
    insta::assert_snapshot!(
        "check_text",
        stdout_of(easement_cached(&root, &["check"], cache.path()).code(1))
    );
}

/// What `suggest`, `graph`, `baseline` and `init` print is read by agents as much
/// as the report of `check`, so it is pinned whole too. A cache of its own, filled
/// first, gives every run the same note.
#[test]
fn the_output_of_suggest_graph_baseline_and_init_is_pinned() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    let run = |args: &[&str]| {
        let out = easement_cached(&root, args, cache.path())
            .get_output()
            .clone();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // Where the temporary directory is differs by machine, and so does whether a
        // symbolic link leads to it (`/var` of macOS) or a short name does (`RUNNER~1`
        // of Windows); a file directly in it is joined with `\` on Windows.
        let real = dunce::canonicalize(&root).unwrap();
        text.replace(real.to_str().unwrap(), "<root>")
            .replace(root.to_str().unwrap(), "<root>")
            .replace("<root>\\", "<root>/")
    };
    easement_cached(&root, &["check"], cache.path()).code(1);
    insta::assert_snapshot!("suggest_text", run(&["suggest"]));
    insta::assert_snapshot!("graph_text", run(&["graph"]));
    insta::assert_snapshot!("baseline_text", run(&["baseline"]));
    insta::assert_snapshot!("init_text", run(&["init", "--force"]));
}

/// A policy the check cannot use is an error, exit code 2, said on stderr.
#[test]
fn config_errors_exit_with_2() {
    let (_tmp, root) = fixture("go");
    fs::write(
        root.join("easement.yml"),
        "version: 1\ncomponents:\n  a: { in: a }\nrules:\n  a: { may_depend_on: [ghost] }\n",
    )
    .unwrap();
    easement(&root, &["check"])
        .code(2)
        .stderr(contains("unknown component `ghost`"));
    easement(&root, &["check", "-c", "missing.yml"]).code(2);
}

/// The review agents read is pinned, over a change that touches every section.
#[test]
fn the_report_of_review_is_pinned() {
    let (_tmp, root) = web_and_storage(WEB_AND_STORAGE);
    fs::write(
        root.join("easement.yml"),
        WEB_AND_STORAGE
            .replace(
                "web: { may_depend_on: [] }",
                "web: { must_not_depend_on: [{ name: storage, because: pages go through the api }] }",
            )
            .replace(
                "storage: { in: src/storage }",
                "storage: { in: src/storage, public: src/storage/api.ts }\n  docs: { in: docs, optional: true }",
            )
            .replace(
                "storage: { may_depend_on: [],",
                "storage: { may_depend_on: [], forbid_external: [lodash],",
            )
            + "exclude: [docs]\ntarget_only: ['**/*.gen.ts']\nwarn: [unused]\nuncovered_ok: [{ name: legacy/**, because: rewritten next quarter }]\n",
    )
    .unwrap();
    fs::write(
        root.join("easement.baseline.yml"),
        baseline_of(["src/web/page.ts".to_string()]),
    )
    .unwrap();
    let cache = TempDir::new().unwrap();
    let mut report: serde_json::Value = serde_json::from_str(&stdout_of(
        easement_cached(
            &root,
            &["review", "--base", "main", "--format", "json"],
            cache.path(),
        )
        .success(),
    ))
    .unwrap();
    let merge_base = report["merge_base"].as_str().unwrap().to_string();
    report["merge_base"] = "[merge-base]".into();
    // Legalized in the new baseline, the violation the change introduces fails nothing.
    assert_eq!(
        report["delta"]["introduced"],
        serde_json::json!([]),
        "{report}"
    );
    assert_eq!(
        report["delta"]["legalized"][0]["file"], "src/web/page.ts",
        "{report}"
    );
    insta::assert_json_snapshot!("review_json", report);
    insta::assert_snapshot!(
        "review_text",
        stdout_of(easement_cached(&root, &["review", "--base", "main"], cache.path()).success())
            .replace(&merge_base[..12], "[merge-base]")
    );
}

/// A baseline of legalized `web -> storage` imports from the given files.
fn baseline_of(files: impl IntoIterator<Item = String>) -> String {
    let mut out = String::from("violations:\n");
    for file in files {
        out.push_str(&format!(
            "- rule: dependency\n  file: {file}\n  target: src/storage/db.ts\n  from: web\n  to: storage\n"
        ));
    }
    out
}

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

/// `--format` picks the format the report is printed in; what each says is the
/// library's (`tests/library/formats.rs`).
#[test]
fn ci_formats() {
    let (_tmp, root) = fixture("policy");
    let github = stdout_of(easement(&root, &["check", "--format", "github"]).code(1));
    assert!(github.starts_with("::error "), "{github}");
    let sarif: serde_json::Value = serde_json::from_str(&stdout_of(
        easement(&root, &["check", "--format", "sarif"]).code(1),
    ))
    .unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let junit = stdout_of(easement(&root, &["check", "--format", "junit"]).code(1));
    assert!(junit.contains("<testsuite name=\"easement\""), "{junit}");
}

#[test]
fn watch_checks_again_after_a_change() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b }\n",
        ),
        ("src/a/x.ts", "export {};\n"),
        ("src/b/y.ts", "export {};\n"),
    ]);
    watch_until(&root, "a -> b is not allowed", || {
        fs::write(root.join("src/a/x.ts"), "import '../b/y';\n").unwrap();
    });
}

/// Runs `check --watch` in `root`, makes `change` once the first check passes,
/// and waits for a check that prints `after`.
fn watch_until(root: &Path, after: &str, change: impl FnOnce()) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let mut child = Command::new(assert_cmd::cargo::cargo_bin("easement"))
        .current_dir(root)
        .args(["check", "--watch"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let wait_for = |text: &str| -> bool {
        loop {
            match rx.recv_timeout(Duration::from_secs(20)) {
                Ok(line) if line.contains(text) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    };
    let passed = wait_for("easement passed");
    if passed {
        change();
    }
    let failed = passed && wait_for(after);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(passed, "the first check did not pass");
    assert!(failed, "the change was not picked up");
}

/// The gap a symbol rule over Kotlin leaves fails `--require-evidence` with exit
/// code 4, and its text is pinned; the gap itself is judged in the library tests.
#[test]
fn the_text_of_a_symbols_gap_is_pinned() {
    let (_tmp, root) = fixture("kotlin");
    let config = root.join("easement.yml");
    let yaml = fs::read_to_string(&config).unwrap().replace(
        "web: { may_depend_on: [domain] }",
        "web: { may_depend_on: [domain, infra] }",
    ) + "\nforbid_symbol: [com.acme.shop.domain.Order]\n";
    fs::write(&config, yaml).unwrap();
    insta::assert_snapshot!(
        "symbols_gap_text",
        stdout_of(easement(&root, &["check", "--require-evidence"]).code(4))
    );
}

/// What the text report says of evidence gaps and accepted gaps is read by
/// agents as much as the JSON, so it is pinned too.
#[test]
fn the_text_of_evidence_and_accepted_gaps_is_pinned() {
    let (_tmp, root) = fixture("uncovered");
    let config = fs::read_to_string(root.join("easement.yml")).unwrap();
    fs::write(
        root.join("easement.yml"),
        format!("{config}uncovered_ok:\n  - {{ name: tools/gen.go, because: generated }}\n"),
    )
    .unwrap();
    // A cache of its own, so the report never says it came from the cache.
    let cache = TempDir::new().unwrap();
    insta::assert_snapshot!(
        "check_gaps_text",
        stdout_of(easement_cached(&root, &["check"], cache.path()).success())
    );
}

#[test]
fn completions_cover_every_shell_and_subcommand() {
    let tmp = TempDir::new().unwrap();
    for (shell, marker) in [
        ("bash", "_easement()"),
        ("zsh", "#compdef easement"),
        ("fish", "complete -c easement"),
        ("powershell", "Register-ArgumentCompleter"),
        ("elvish", "edit:completion:arg-completer"),
    ] {
        let script = stdout_of(easement(tmp.path(), &["completions", shell]).success());
        assert!(script.contains(marker), "{shell}: {script}");
        assert!(script.contains("baseline"), "{shell} lacks subcommands");
    }
    easement(tmp.path(), &["completions", "tcsh"])
        .code(2)
        .stderr(contains("invalid value"));
}

#[test]
fn color_follows_the_flag_and_the_environment() {
    let (_tmp, root) = fixture("go");
    let run = |args: &[&str], envs: &[(&str, &str)]| {
        let mut cmd = Command::cargo_bin("easement").unwrap();
        cmd.current_dir(&root).args(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        stdout_of(cmd.assert().code(1))
    };
    // Piped output carries no escape codes by default.
    assert!(!run(&["check"], &[]).contains('\x1b'));
    // `--color always` forces them; `NO_COLOR` and `--color never` remove them.
    assert!(run(&["check", "--color", "always"], &[]).contains('\x1b'));
    assert!(!run(&["check", "--color", "never"], &[("CLICOLOR_FORCE", "1")]).contains('\x1b'));
    assert!(!run(&["check"], &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]).contains('\x1b'));
    // The structured formats are never coloured.
    assert!(!run(&["check", "--format", "json", "--color", "always"], &[]).contains('\x1b'));
}

/// A miss under `--cache-only` has an exit code of its own, 3, and the flag
/// conflicts with `--no-cache`; what the miss is, `NotCached`, is tested
/// through the library.
#[test]
fn cache_only_runs_no_indexer_and_exits_3_on_a_miss() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    easement_cached(&root, &["check", "--cache-only"], cache.path())
        .code(3)
        .stderr(contains("not cached"));
    easement_cached(
        &root,
        &["check", "--cache-only", "--no-cache"],
        cache.path(),
    )
    .code(2);
}

/// Every command that reads the indexes takes the cache flags its cache note
/// tells to use: `--cache-only` misses with exit code 3 on an empty cache, and
/// `--no-cache` indexes again.
#[test]
fn every_command_that_reads_the_indexes_takes_the_cache_flags() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    let commands: [&[&str]; 6] = [
        &["impact", "internal/models"],
        &["why", "handler", "model"],
        &["suggest"],
        &["graph"],
        &["baseline"],
        &["init", "--force"],
    ];
    for command in commands {
        easement_cached(&root, &[command, &["--cache-only"]].concat(), cache.path())
            .code(3)
            .stderr(contains("not cached"));
    }
    for command in &commands[..4] {
        easement_cached(&root, &[*command, &["--no-cache"]].concat(), cache.path()).success();
    }
}

/// `policy` prints the policy on stdout, as text or as JSON, and reads no code:
/// its cache stays empty although the project holds files an indexer would read.
#[test]
fn policy_prints_the_policy_and_runs_no_indexer() {
    let (_tmp, root) = fixture("policy");
    let cache = TempDir::new().unwrap();
    let text = stdout_of(easement_cached(&root, &["policy"], cache.path()).success());
    assert!(text.starts_with("policy: easement.yml\n"), "{text}");
    let policy =
        json_of(easement_cached(&root, &["policy", "--format", "json"], cache.path()).success());
    assert_eq!(policy["inputs"][0]["path"], "easement.yml", "{policy}");
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

fn json_of(assert: assert_cmd::assert::Assert) -> serde_json::Value {
    serde_json::from_str(&stdout_of(assert)).unwrap()
}

#[test]
fn the_report_of_propose_is_pinned() {
    let (_tmp, root) = shop();
    let cache = TempDir::new().unwrap();
    let plan = r#"[{"from": "src/checkout/pay.ts", "to": "src/billing/internal/ledger.ts"},
                   {"from": "src/checkout/fresh.ts", "to": "src/notify/mail.ts"},
                   {"from": "src/shipping/ship.ts", "to": "src/notify/mail.ts"},
                   {"from": "src/notify/mail.ts", "to": "src/billing/api/charge.ts"},
                   {"from": "src/notify/mail.ts", "external": "nodemailer"}]"#;
    fs::write(root.join("plan.json"), plan).unwrap();
    let args = ["propose", "--plan", "plan.json"];
    insta::assert_json_snapshot!(
        "propose_json",
        json_of(
            easement_cached(
                &root,
                &[&args[..], &["--format", "json"]].concat(),
                cache.path()
            )
            .code(1)
        )
    );
    insta::assert_snapshot!(
        "propose_text",
        stdout_of(easement_cached(&root, &args, cache.path()).code(1))
    );
}

/// `--jobs` takes a number of 1 or more, or `cpus`; anything else is an error
/// that names the flag.
#[test]
fn jobs_of_zero_is_an_error_that_names_the_flag() {
    let (_tmp, root) = project(&[(
        "easement.yml",
        "version: 1\ncomponents:\n  api: { in: api }\n",
    )]);
    easement(&root, &["check", "--jobs", "0"])
        .code(2)
        .stderr(contains("--jobs"));
}

/// A run that `--require-evidence` fails for an evidence gap exits 4.
#[test]
fn require_evidence_exits_4_on_an_evidence_gap() {
    let (_tmp, root) = fixture("uncovered");
    easement(&root, &["check", "--require-evidence"]).code(4);
}

/// `check --suggest` fails on unused allowances with exit code 1; it judges the
/// whole project, so a narrowed run is a usage error. `suggest --check`, which it
/// replaces, is gone.
#[test]
fn check_suggest_exits_1_on_an_unused_allowance_and_refuses_narrowing() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\n  lib: { in: src/lib }\nrules:\n  app: { may_depend_on: [lib] }\n",
        ),
        ("src/app/a.ts", "export const a = 1;\n"),
        ("src/lib/l.ts", "export const l = 1;\n"),
    ]);
    easement(&root, &["check"]).success();
    easement(&root, &["check", "--suggest"])
        .code(1)
        .stdout(contains("easement.yml:6: `app` -> `lib`"))
        .stdout(contains("[unused-allowance]"));
    for narrowed in [
        &["--diff", "HEAD"][..],
        &["--scope", "src"],
        &["--component", "app"],
    ] {
        easement(&root, &[&["check", "--suggest"][..], narrowed].concat())
            .code(2)
            .stderr(contains("cannot be used with"));
    }
    easement(&root, &["suggest", "--check"])
        .code(2)
        .stderr(contains("unexpected argument '--check'"));
}

/// `doctor --cache` exits 3 while an index is not cached and 0 once every one
/// is, and says so in text and JSON; why an index misses is tested through the
/// library.
#[test]
fn doctor_cache_exits_3_until_every_index_is_cached() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    easement_cached(&root, &["doctor", "--cache"], cache.path())
        .code(3)
        .stdout(contains(
            "go (.): would run: no cached index of this directory",
        ));
    easement_cached(&root, &["check"], cache.path());
    easement_cached(&root, &["doctor", "--cache"], cache.path())
        .success()
        .stdout(contains("go (.): cached"));
    let output = easement_cached(
        &root,
        &["doctor", "--cache", "--format", "json"],
        cache.path(),
    )
    .success()
    .get_output()
    .stdout
    .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["cached"], true, "{report}");
    assert_eq!(report["indexes"][0]["language"], "go", "{report}");
}

/// `--plan -` reads the plan from stdin.
#[test]
fn propose_reads_the_plan_from_stdin() {
    let (_tmp, root) = shop();
    Command::cargo_bin("easement")
        .unwrap()
        .current_dir(&root)
        .env("EASEMENT_CACHE_DIR", test_cache_dir())
        .args(["propose", "--plan", "-"])
        .write_stdin(
            r#"[{"from": "src/shipping/ship.ts", "to": "src/notify/mail.ts"},
                {"from": "src/notify/mail.ts", "to": "src/shipping/ship.ts"}]"#,
        )
        .assert()
        .code(1)
        .stdout(contains(
            "component dependency cycle: notify -> shipping -> notify [cycle]",
        ));
}

/// `propose` takes a plan or one proposal, never both.
#[test]
fn propose_takes_a_plan_or_a_source_not_both() {
    let (_tmp, root) = shop();
    fs::write(
        root.join("plan.json"),
        r#"[{"from": "src/checkout/pay.ts", "to": "src/billing/api/charge.ts"}]"#,
    )
    .unwrap();
    easement(
        &root,
        &[
            "propose",
            "src/checkout/pay.ts",
            "src/billing/api/charge.ts",
            "--plan",
            "plan.json",
        ],
    )
    .code(2);
    easement(
        &root,
        &["propose", "--plan", "plan.json", "--external", "lodash"],
    )
    .code(2)
    .stderr(contains("cannot be used with"));
}

/// `check --plan` reads the plan from a file or stdin, as `propose --plan` does,
/// reports how the change departs from it without changing the exit code, and
/// needs `--diff`.
#[test]
fn check_plan_compares_the_change_with_a_plan_and_needs_diff() {
    let (_tmp, root) = web_and_storage(WEB_AND_STORAGE);
    fs::write(
        root.join("src/web/new.ts"),
        "import { save } from \"../storage/db\";\nexport const fresh = save;\n",
    )
    .unwrap();
    let plan = r#"[{"from": "src/web/page.ts", "to": "src/storage/db.ts"},
                   {"from": "src/web/page.ts", "external": "zod"}]"#;
    fs::write(root.join("plan.json"), plan).unwrap();
    let said = "Not in the plan:\n  src/web/new.ts:1 -> src/storage/db.ts\n\nPlanned, not in the code:\n  src/web/page.ts -> external zod\n";
    easement(&root, &["check", "--diff", "main", "--plan", "plan.json"])
        .success()
        .stdout(contains(said));
    Command::cargo_bin("easement")
        .unwrap()
        .current_dir(&root)
        .env("EASEMENT_CACHE_DIR", test_cache_dir())
        .args(["check", "--diff", "main", "--plan", "-", "--format", "json"])
        .write_stdin(plan)
        .assert()
        .success()
        .stdout(contains(r#""unplanned": ["#));
    easement(&root, &["check", "--plan", "plan.json"])
        .code(2)
        .stderr(contains("--diff <BASE>"));
    easement(
        &root,
        &["check", "--diff", "main", "--plan", "missing.json"],
    )
    .code(2)
    .stderr(contains("cannot read the plan missing.json"));
}

/// `graph` and `suggest --patch` print their output on stdout, so their notes,
/// the one that the indexes came from the cache among them, go to stderr.
#[test]
fn graph_and_suggest_patch_print_their_notes_on_stderr() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    let run = |args: &[&str]| easement_cached(&root, args, cache.path());
    run(&["check"]).code(1);
    run(&["suggest", "--patch"])
        .success()
        .stdout(contains("came from the cache").not())
        .stderr(contains("came from the cache"));
    run(&["graph"])
        .success()
        .stdout(contains("came from the cache").not())
        .stderr(contains("came from the cache"));
}
