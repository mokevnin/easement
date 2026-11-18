//! The index cache: when an index is reused (another checkout, a touched file,
//! the other language of the project) and when it is built again, how many
//! languages are indexed at once, `CacheUse::Only`, which runs no indexer, and
//! `doctor --cache`, which says what a run would read from the cache.

use std::fs;
use std::path::Path;

use easement::commands::{self, CheckOptions, Outcome};
use easement::{Cache, CacheUse, Environment, Indexing, Jobs, NotCached};
use tempfile::TempDir;

use crate::common::{fixture, project, test_cache_dir};
use crate::run::{cannot_index_here, check_in, config, indexing_in, json, parse, pinned, text};

/// What a run says about the code, without how the indexes were obtained.
fn verdict(root: &Path, cache: &Path) -> (serde_json::Value, serde_json::Value) {
    let report = json(&check_in(root, cache, CacheUse::On).unwrap());
    (report["violations"].clone(), report["stats"].clone())
}

#[test]
fn another_checkout_of_the_same_code_reuses_the_indexes() {
    let cache = TempDir::new().unwrap();
    let checkout = |name: &str| {
        let (tmp, root) = fixture(name);
        if name == "rust" {
            // Not stored in the fixture, see `layered`.
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
        }
        (tmp, root)
    };
    for name in ["go", "ts", "python", "rust", "java"] {
        if cannot_index_here(&[name]) {
            continue;
        }
        let (_first_tmp, first) = checkout(name);
        let (violations, stats) = verdict(&first, cache.path());
        assert!(
            stats["indexed"].as_u64().unwrap_or(0) >= 1,
            "{name}: {stats}"
        );
        // A copy is another path with other modification times.
        let (_second_tmp, second) = checkout(name);
        let (again, cached) = verdict(&second, cache.path());
        assert!(cached.get("indexed").is_none(), "{name}: {cached}");
        assert_eq!(cached["from_cache"], stats["indexed"], "{name}");
        assert_eq!(again, violations, "{name}");
        assert_eq!(cached["internal_deps"], stats["internal_deps"], "{name}");
    }
}

#[test]
fn a_touched_file_hits_the_cache_and_a_changed_file_above_the_project_misses_it() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  pkg: { in: pkg }\n",
        ),
        ("base.json", "{\"compilerOptions\": {}}\n"),
        (
            "pkg/tsconfig.json",
            "{\"extends\": \"../base.json\", \"include\": [\"src\"]}\n",
        ),
        ("pkg/src/a.ts", "export const a = 1;\n"),
    ]);
    let cache = TempDir::new().unwrap();
    let first = verdict(&root, cache.path()).1["indexed"].clone();
    assert!(first.as_u64().unwrap_or(0) >= 1, "{first}");
    // The same bytes with a new modification time: the content is what counts.
    let file = root.join("pkg/src/a.ts");
    fs::write(&file, fs::read(&file).unwrap()).unwrap();
    assert_eq!(verdict(&root, cache.path()).1["from_cache"], first);
    // The project's `tsconfig.json` extends a file that sits above it.
    fs::write(
        root.join("base.json"),
        "{\"compilerOptions\": {\"strict\": true}}\n",
    )
    .unwrap();
    let after = verdict(&root, cache.path()).1;
    assert!(after["indexed"].as_u64().unwrap_or(0) >= 1, "{after}");
}

#[test]
fn a_change_to_one_language_reuses_the_index_of_the_other() {
    // Go and TypeScript share the project root, as in a repository with a Go backend and a
    // frontend: each index is keyed by the files its indexer reads.
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  api: { in: api }\n  web: { in: web }\n",
        ),
        ("go.mod", "module example.com/shop\n\ngo 1.22\n"),
        ("api/api.go", "package api\n\nconst Name = \"shop\"\n"),
        ("web/a.ts", "export const a = 1;\n"),
        ("README.md", "# shop\n"),
    ]);
    let cache = TempDir::new().unwrap();
    let stats = || verdict(&root, cache.path()).1;
    assert_eq!(stats()["indexed"], 2);
    let edit = |path: &str| {
        let file = root.join(path);
        let text = fs::read_to_string(&file).unwrap();
        fs::write(&file, format!("{text}\n// changed\n")).unwrap();
    };
    // Neither indexer reads the README.
    edit("README.md");
    let after = stats();
    assert!(after.get("indexed").is_none(), "{after}");
    assert_eq!(after["from_cache"], 2);
    edit("web/a.ts");
    let after = stats();
    assert_eq!(
        (after["indexed"].clone(), after["from_cache"].clone()),
        (1.into(), 1.into())
    );
    edit("api/api.go");
    let after = stats();
    assert_eq!(
        (after["indexed"].clone(), after["from_cache"].clone()),
        (1.into(), 1.into())
    );
}

/// A vendored dependency is no code of the project, but scip-go type-checks
/// against it: re-vendoring a module that a `replace` takes from a directory
/// outside the project changes `vendor/` alone, and the index is built again.
#[test]
fn a_change_to_vendored_code_misses_the_cache() {
    let (_tmp, root) = fixture("go_vendor");
    let cache = TempDir::new().unwrap();
    let first = verdict(&root, cache.path()).1;
    assert_eq!(first["indexed"], 1, "{first}");
    let file = root.join("vendor/example.com/dep/dep.go");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{text}\n// Bye is new.\nfunc Bye() {{}}\n")).unwrap();
    let after = verdict(&root, cache.path()).1;
    assert_eq!(after["indexed"], 1, "{after}");
}

/// A file `.gitignore` hides is still one the indexer reads (generated code an
/// import names), so it keys the index: changing it, or generating it later,
/// misses the cache, and the verdict is the one a fresh index gives.
#[test]
fn a_file_gitignore_hides_that_the_indexer_reads_keys_the_index() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  app: { in: src/app }\n  b: { in: src/b }\n  c: { in: src/c }\nrules:\n  app: { may_depend_on: [b] }\n",
        ),
        ("package.json", "{\"name\": \"app\", \"private\": true}\n"),
        (".gitignore", "src/gen/\n"),
        ("src/b/x.ts", "export const x = 1;\n"),
        ("src/c/x.ts", "export const x = 2;\n"),
        (
            "src/app/a.ts",
            "import { x } from \"../gen/index\";\nexport const a = x;\n",
        ),
    ]);
    let cache = TempDir::new().unwrap();
    let messages = |root: &Path| -> Vec<String> {
        let (violations, _) = verdict(root, cache.path());
        violations
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["message"].as_str().unwrap().to_string())
            .collect()
    };
    let to_c = ["app -> c is not allowed (written as src/c/x.ts)".to_string()];
    let generated = root.join("src/gen/index.ts");
    // Generated after a first check: nothing to reach yet.
    assert_eq!(messages(&root), Vec::<String>::new());
    fs::create_dir_all(generated.parent().unwrap()).unwrap();
    fs::write(&generated, "export { x } from \"../c/x\";\n").unwrap();
    assert_eq!(messages(&root), to_c);
    // Generated again, from another component.
    fs::write(&generated, "export { x } from \"../b/x\";\n").unwrap();
    assert_eq!(messages(&root), Vec::<String>::new());
    fs::write(&generated, "export { x } from \"../c/x\";\n").unwrap();
    assert_eq!(messages(&root), to_c);
    // What the indexer never reads there keys nothing, nor does an install, which a
    // lock file keys.
    fs::write(root.join("src/gen/build.log"), "built\n").unwrap();
    let (_, stats) = verdict(&root, cache.path());
    assert_eq!(stats["from_cache"], 1, "{stats}");
    fs::create_dir_all(root.join("node_modules/kit")).unwrap();
    fs::write(
        root.join("node_modules/kit/index.d.ts"),
        "export declare const kit: number;\n",
    )
    .unwrap();
    let (_, stats) = verdict(&root, cache.path());
    assert_eq!(stats["from_cache"], 1, "{stats}");
}

#[test]
fn jobs_index_languages_at_once_with_the_same_report() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  api: { in: api }\n  web: { in: web }\n",
        ),
        ("go.mod", "module example.com/shop\n\ngo 1.22\n"),
        ("api/api.go", "package api\n\nconst Name = \"shop\"\n"),
        ("web/a.ts", "export const a = 1;\n"),
    ]);
    let report = |jobs: &str| {
        let jobs: Jobs = jobs.parse().unwrap();
        let mut cache = Cache::new(Indexing {
            jobs,
            ..indexing_in(test_cache_dir(), CacheUse::Off)
        });
        let checked = commands::check(&CheckOptions::new(config(&root)), &mut cache).unwrap();
        assert_eq!(checked.outcome(), Outcome::Passed);
        checked.report().json()
    };
    let one = report("1");
    assert!(one.contains("\"indexed\": 2"), "{one}");
    assert_eq!(report("2"), one);
    assert_eq!(report("cpus"), one);
}

/// Under `CacheUse::Only` no indexer runs: a miss stops the command with
/// `NotCached` (which the binary maps to exit code 3), a hit checks as usual.
#[test]
fn cache_only_runs_no_indexer_and_stops_on_a_miss() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    // Nothing cached yet: no indexer runs, and the miss is an error of its own.
    let Err(missed) = check_in(&root, cache.path(), CacheUse::Only) else {
        panic!("a check of an empty cache under CacheUse::Only did not stop");
    };
    assert!(missed.downcast_ref::<NotCached>().is_some(), "{missed:#}");
    assert!(format!("{missed:#}").contains("not cached"), "{missed:#}");
    // The fixture has violations: it fails with the index read from the cache.
    let checked = check_in(&root, cache.path(), CacheUse::On).unwrap();
    assert_eq!(checked.outcome(), Outcome::Failed);
    let checked = check_in(&root, cache.path(), CacheUse::Only).unwrap();
    assert_eq!(checked.outcome(), Outcome::Failed);
    assert_eq!(json(&checked)["stats"]["from_cache"], 1);
}

#[test]
fn indexes_are_cached_until_the_project_changes() {
    let (_tmp, root) = fixture("go");
    let cache = TempDir::new().unwrap();
    let stats = |cache_use: CacheUse| -> serde_json::Value {
        let checked = check_in(&root, cache.path(), cache_use).unwrap();
        assert_eq!(checked.outcome(), Outcome::Failed);
        json(&checked)["stats"].clone()
    };
    let first = stats(CacheUse::On);
    assert_eq!(first["indexed"], 1);
    assert!(first.get("from_cache").is_none());

    // Nothing changed: the index comes from the cache, and the verdict is the same.
    let second = stats(CacheUse::On);
    assert!(second.get("indexed").is_none());
    assert_eq!(second["from_cache"], 1);
    assert_eq!(second["internal_deps"], first["internal_deps"]);

    // CacheUse::Off runs the indexer again.
    assert_eq!(stats(CacheUse::Off)["indexed"], 1);

    // A changed file misses the cache.
    let file = root.join("internal/models/book.go");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{text}\n// changed\n")).unwrap();
    assert_eq!(stats(CacheUse::On)["indexed"], 1);
    assert_eq!(stats(CacheUse::On)["from_cache"], 1);
}

/// What the indexer inherits is in the key of its index: an index built with
/// `GOFLAGS=-tags=race` brings the race file in, and is not reused by a run
/// without it, which leaves the file out and caches an index of its own.
#[test]
fn an_index_built_with_other_goflags_is_not_reused() {
    let (_tmp, root) = fixture("gotags");
    let check_with_env = |environment: Environment| {
        let mut cache = Cache::new(Indexing {
            environment,
            ..indexing_in(test_cache_dir(), CacheUse::On)
        });
        let checked = commands::check(&CheckOptions::new(config(&root)), &mut cache).unwrap();
        assert_eq!(checked.outcome(), Outcome::Passed);
        text(&checked)
    };
    let race = check_with_env(Environment::of_process().with("GOFLAGS", "-tags=race"));
    assert!(!race.contains("not covered"), "{race}");
    // The race file is not built without the tag the first run inherited.
    let plain = check_with_env(Environment::of_process().with("GOFLAGS", ""));
    assert!(plain.contains("  pkg/race.go"), "{plain}");
    // The same run again reads its index from the cache, and says so.
    let again = check_with_env(Environment::of_process().with("GOFLAGS", ""));
    assert!(again.contains("came from the cache"), "{again}");
    assert!(again.contains("--no-cache"), "{again}");
}

/// `doctor --cache` says, for each index, whether a run would read it from the
/// cache, and why not, with the key `check` computes: it agrees with `check`
/// before and after a change, and it reads no index and runs no indexer.
#[test]
fn doctor_cache_says_which_indexes_a_check_would_read_from_the_cache() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  api: { in: api }\n  web: { in: web }\nexclude: [web/gen]\n",
        ),
        ("go.mod", "module example.com/shop\n\ngo 1.22\n"),
        ("api/api.go", "package api\n\nconst Name = \"shop\"\n"),
        ("web/a.ts", "export const a = 1;\n"),
        // Excluded, but in the key: the index resolves imports of it.
        ("web/gen/g.ts", "export const g = 1;\n"),
    ]);
    let cache = TempDir::new().unwrap();
    let probe = |environment: Environment| {
        let indexing = Indexing {
            environment,
            ..indexing_in(cache.path(), CacheUse::On)
        };
        commands::doctor_cache(&config(&root), &indexing).unwrap()
    };
    let states = |probed: &commands::Probed| -> Vec<(String, serde_json::Value)> {
        parse(&probed.json())["indexes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["language"].as_str().unwrap().to_string(),
                    p["reason"].clone(),
                )
            })
            .collect()
    };
    let missed = |reason: &str| serde_json::Value::from(reason);
    let hit = serde_json::Value::Null;

    let empty = probe(Environment::of_process());
    assert_eq!(empty.outcome(), Outcome::NotCached);
    assert_eq!(
        states(&empty),
        [
            ("go".to_string(), missed("no_entry")),
            ("typescript".to_string(), missed("no_entry"))
        ]
    );
    // Nothing ran: the cache is still empty.
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);

    verdict(&root, cache.path());
    let warm = probe(Environment::of_process());
    assert_eq!(warm.outcome(), Outcome::Passed, "{}", warm.text());
    assert_eq!(
        states(&warm),
        [
            ("go".to_string(), hit.clone()),
            ("typescript".to_string(), hit.clone())
        ]
    );
    pinned(|| insta::assert_snapshot!("doctor_cache_text_warm", warm.text()));

    let file = root.join("web/a.ts");
    fs::write(&file, "export const a = 2;\n").unwrap();
    let changed = probe(Environment::of_process());
    assert_eq!(changed.outcome(), Outcome::NotCached);
    assert_eq!(
        states(&changed),
        [
            ("go".to_string(), hit.clone()),
            ("typescript".to_string(), missed("files_changed"))
        ]
    );
    pinned(|| insta::assert_snapshot!("doctor_cache_text_changed", changed.text()));
    // `check` agrees: it indexes TypeScript again and reads Go from the cache.
    let stats = verdict(&root, cache.path()).1;
    assert_eq!(
        (&stats["indexed"], &stats["from_cache"]),
        (&1.into(), &1.into())
    );

    let goflags = probe(Environment::of_process().with("GOFLAGS", "-tags=race"));
    assert_eq!(
        states(&goflags),
        [
            ("go".to_string(), missed("indexer_changed")),
            ("typescript".to_string(), hit.clone())
        ]
    );

    // An index that cannot be read still counts: `doctor --cache` reads none.
    for entry in fs::read_dir(cache.path()).unwrap() {
        fs::write(entry.unwrap().path(), "not an index").unwrap();
    }
    assert_eq!(probe(Environment::of_process()).outcome(), Outcome::Passed);
}
