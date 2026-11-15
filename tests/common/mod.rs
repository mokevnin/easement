//! What the integration tests share: scratch copies of the fixture projects.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tempfile::TempDir;

pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// Writes a stub package for each import specifier into `node_modules`, so the
/// TypeScript indexer sees the module as installed: `lodash/fp` becomes
/// `node_modules/lodash/package.json` and `node_modules/lodash/fp.d.ts`.
pub fn stub_packages(root: &Path, specifiers: &[&str]) {
    for specifier in specifiers {
        let mut parts = specifier.split('/');
        let first = parts.next().unwrap();
        let (name, rest): (String, Vec<&str>) = if first.starts_with('@') {
            (
                format!("{first}/{}", parts.next().unwrap()),
                parts.collect(),
            )
        } else {
            (first.to_string(), parts.collect())
        };
        let dir = root.join("node_modules").join(&name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            format!(r#"{{"name":"{name}","version":"1.0.0","types":"index.d.ts"}}"#),
        )
        .unwrap();
        let stub = "declare const stub: any;\nexport default stub;\n";
        fs::write(dir.join("index.d.ts"), stub).unwrap();
        if !rest.is_empty() {
            let file = dir.join(format!("{}.d.ts", rest.join("/")));
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, stub).unwrap();
        }
    }
}

/// A scratch copy of a fixture project, so tests can write baselines and edit code.
/// A `modules.txt` in the fixture lists third-party imports to stub as installed.
pub fn fixture(name: &str) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    copy_dir(&src, tmp.path());
    let root = tmp.path().to_path_buf();
    if let Ok(list) = fs::read_to_string(root.join("modules.txt")) {
        stub_packages(&root, &list.lines().collect::<Vec<_>>());
        fs::remove_file(root.join("modules.txt")).unwrap();
    }
    (tmp, root)
}

/// The TypeScript indexer needs a `tsconfig.json`; this one reads every file.
pub const TSCONFIG: &str = r#"{"compilerOptions":{"target":"es2022","module":"esnext","moduleResolution":"bundler","allowJs":true,"noEmit":true,"skipLibCheck":true},"include":["**/*"],"exclude":["node_modules"]}"#;

/// A scratch project from the given files; a `tsconfig.json` is added when
/// there are TypeScript or JavaScript files and none is given.
pub fn project(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let has_ts = files
        .iter()
        .any(|(p, _)| [".ts", ".tsx", ".js"].iter().any(|e| p.ends_with(e)));
    if has_ts && !files.iter().any(|(p, _)| *p == "tsconfig.json") {
        fs::write(tmp.path().join("tsconfig.json"), TSCONFIG).unwrap();
    }
    let root = tmp.path().to_path_buf();
    write_all(&root, files);
    (tmp, root)
}

/// Writes `files` under `root`, creating their directories.
pub fn write_all(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

/// git run in `root` on the repository of `root`: without the variables
/// (`GIT_DIR`, `GIT_INDEX_FILE`, …) through which a git that runs the tests, from
/// a hook or `git rebase --exec`, would have it write to its own repository.
pub fn git_in(root: &Path) -> std::process::Command {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()
            .unwrap();
        assert!(output.status.success(), "git rev-parse --local-env-vars");
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    });
    let mut command = std::process::Command::new("git");
    for name in names {
        command.env_remove(name);
    }
    command.current_dir(root);
    command
}

/// Runs git in `root` as a throwaway author, and fails the test when it fails.
pub fn git(root: &Path, args: &[&str]) {
    let status = git_in(root)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Makes `root` a repository whose one commit, on `main`, holds every file.
pub fn commit_all(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
}

/// Where the tests keep indexes, shared by every test of the run so a fixture is
/// indexed once, and apart from the cache of whoever runs them. `cargo test` runs
/// the tests of a binary as threads of one process, which share a temporary
/// directory. nextest runs each test in a process of its own, so the run shares a
/// directory under `target/tmp` named by its id (`NEXTEST_RUN_ID`): an index is
/// written to a temporary file and renamed into place, so processes never read
/// half of one. Each run starts empty, as a cache that outlives a run could hide
/// a change to how an index is made; what earlier runs left is removed.
pub fn test_cache_dir() -> &'static Path {
    enum Dir {
        Process(TempDir),
        Run(PathBuf),
    }
    static DIR: OnceLock<Dir> = OnceLock::new();
    let dir = DIR.get_or_init(|| match std::env::var("NEXTEST_RUN_ID") {
        Ok(run) => {
            let runs = Path::new(env!("CARGO_TARGET_TMPDIR")).join("test-cache");
            remove_old_runs(&runs, &run);
            let dir = runs.join(run);
            fs::create_dir_all(&dir).unwrap();
            Dir::Run(dir)
        }
        Err(_) => Dir::Process(TempDir::new().unwrap()),
    });
    match dir {
        Dir::Process(dir) => dir.path(),
        Dir::Run(dir) => dir,
    }
}

/// Removes the caches of earlier nextest runs: those not touched for an hour, so
/// that a run going on in the same checkout keeps its own.
fn remove_old_runs(runs: &Path, current: &str) {
    let Ok(entries) = fs::read_dir(runs) else {
        return;
    };
    let hour = std::time::Duration::from_secs(3600);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > hour));
        if entry.file_name() != current && old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Commits every file of `root` on `main`, then checks out `feature`.
pub fn on_feature(root: &std::path::Path) {
    commit_all(root);
    git(root, &["checkout", "-q", "-b", "feature"]);
}

/// A project of two components, committed on `main` and checked out on
/// `feature`, where `web` imports `storage` on unchanged lines.
pub fn web_and_storage(policy: &str) -> (TempDir, PathBuf) {
    let (tmp, root) = project(&[
        ("easement.yml", policy),
        (
            "src/web/page.ts",
            "import { save } from \"../storage/db\";\nexport const page = save;\n",
        ),
        ("src/storage/db.ts", "export const save = 1;\n"),
    ]);
    on_feature(&root);
    (tmp, root)
}

pub const WEB_AND_STORAGE: &str = "version: 1
components:
  web: { in: src/web }
  storage: { in: src/storage }
common: [storage]
rules:
  web: { may_depend_on: [] }
  storage: { may_depend_on: [], because: storage is the bottom }
";

/// A shop where `billing` already imports `checkout`, and nothing else
/// crosses a component: what `propose` judges before any import is written.
pub fn shop() -> (TempDir, PathBuf) {
    project(&[
        ("easement.yml", SHOP),
        ("src/checkout/cart.ts", "export const cart = 1;\n"),
        ("src/checkout/pay.ts", "export const pay = 1;\n"),
        (
            "src/billing/api/charge.ts",
            "import { cart } from \"../../checkout/cart\";\nexport const charge = cart;\n",
        ),
        (
            "src/billing/internal/ledger.ts",
            "export const ledger = 1;\n",
        ),
        ("src/shipping/ship.ts", "export const ship = 1;\n"),
        ("src/notify/mail.ts", "export const mail = 1;\n"),
    ])
}

pub const SHOP: &str = "version: 1
components:
  checkout: { in: src/checkout }
  billing:
    in: src/billing
    public: [src/billing/api]
    because: billing internals change without notice
  shipping: { in: src/shipping }
  notify: { in: src/notify }
no_cycles: true
rules:
  checkout: { may_depend_on: [billing], max_fan_out: 1 }
  billing: { may_depend_on: [checkout] }
  shipping:
    may_depend_on: [notify]
    must_not_reach: [{ name: billing, because: shipping learns of payments from events }]
  notify: { may_depend_on: [shipping, billing] }
";
