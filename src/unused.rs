//! The `unused` rule: files of components that no chain of imports from an
//! entry point reaches. An agent acting on it deletes the file, so every
//! doubt counts a file as reached.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;

use crate::chains::{file_graph, reach};
use crate::check::{self, Place, RuleId, Unjudged, Violation};
use crate::components::ComponentMap;
use crate::config::{Config, TESTS};
use crate::lang::Dep;
use crate::lang::registry::{self, LANGUAGES, Language, UnindexedUse};
use crate::relpath::{self, extension, glob_set, parent};
use crate::scan::Unresolved;

/// What the rule reads from one scan.
pub struct Input<'a> {
    pub config: &'a Config,
    pub map: &'a ComponentMap,
    pub root: &'a Path,
    /// Imports between checked files, from the index or an adapter.
    pub deps: &'a [Dep],
    /// Imports the checks do not judge but that reach a file all the same:
    /// those in `target_only` and excluded files.
    pub unread_deps: &'a [Dep],
    /// Files the checks do not read: `target_only` and excluded ones.
    pub unread: &'a [String],
    pub paths: &'a [String],
    pub all_files: &'a [String],
    pub uncovered: &'a [String],
    /// Imports of checked files whose dependency is not in the graph: any of
    /// them a reached file writes may reach a file the walk does not.
    pub unresolved: &'a [Unresolved],
}

/// How many lost imports the reason of the files they may reach names.
const LOST_SHOWN: usize = 3;

pub struct Unused {
    pub violations: Vec<Violation>,
    /// Why some files were taken as used without being judged.
    pub reasons: Vec<Unjudged>,
}

/// The language of a file the rule judges: each is checked against its indexer
/// (C# and PHP by the CI tests), whose index reports a use of another file
/// wherever it happens.
pub(crate) fn family(file: &str) -> Option<&'static str> {
    judged_language(file).map(|language| language.name)
}

/// The language of a file the rule judges, if it judges the file.
fn judged_language(file: &str) -> Option<&'static Language> {
    registry::of(file).filter(|language| language.unused.contains(&extension(file)))
}

pub fn unused(input: &Input) -> Result<Unused> {
    let config = input.config;
    if config.entry.is_empty() {
        return Ok(Unused {
            violations: Vec::new(),
            reasons: Vec::new(),
        });
    }
    let owned = |patterns: &[&str]| patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    let entry = glob_set(&config.entry, "entry")?;
    let tests = glob_set(&owned(&registry::test_patterns()), "tests")?;
    let conventional: Vec<&str> = LANGUAGES
        .iter()
        .flat_map(|language| language.conventional.iter().copied())
        .collect();
    let conventional = glob_set(&owned(&conventional), "conventional entries")?;
    // Files that could not be read: each is taken as used, or as what makes
    // others used, and the report says so.
    let mut unreadable: BTreeSet<String> = BTreeSet::new();
    let packaged = package_entries(input.root, input.all_files, &mut unreadable);

    // A family is judged only where the graph sees all of it: not when an
    // indexer left some of its files out, and not beside a language whose files
    // use its files without an import.
    let mut skipped: BTreeMap<&str, String> = BTreeMap::new();
    for file in input.uncovered {
        if let Some(f) = family(file) {
            skipped.insert(f, "some of its files were not covered by an indexer".into());
        }
    }
    for language in LANGUAGES {
        if language.uses_unimported.is_empty()
            || !input
                .paths
                .iter()
                .any(|f| registry::is(Path::new(f), language))
        {
            continue;
        }
        for (used, why) in language.uses_unimported {
            skipped.insert(used.name, why.to_string());
        }
    }
    let mut unjudged: BTreeSet<&str> = BTreeSet::new();
    let mut roots: Vec<&str> = input.unread.iter().map(String::as_str).collect();
    let mut candidates = Vec::new();
    for file in input.paths {
        let judged = family(file).filter(|f| !skipped.contains_key(f));
        if judged.is_none() && family(file).is_none() {
            unjudged.insert(extension(file));
        }
        let root = entry.is_match(file)
            || tests.is_match(file)
            || conventional.is_match(file)
            || registry::is_declaration(file)
            || packaged.contains(file.as_str())
            || program(input.root, file, &mut unreadable)
            || input.map.component_of(file).is_none_or(|c| c == TESTS)
            || judged.is_none();
        if root {
            roots.push(file);
        } else {
            candidates.push(file);
        }
    }

    let mut graph = file_graph(input.deps.iter().chain(input.unread_deps), input.all_files);
    let found = unindexed_uses(input.root, input.paths, input.all_files, &mut unreadable);
    for (from, used) in &found {
        graph
            .entry(from.clone())
            .or_default()
            .insert((used.to.clone(), 0));
    }
    let reached = reached_from(roots, &graph, input.paths);

    // An import the index did not resolve is lost to the walk unless the rule
    // follows it itself; one a reached file writes may reach any file the walk
    // did not, so none of those is called unused.
    let followed: BTreeSet<Place> = found
        .iter()
        .map(|(from, used)| Place {
            file: from.clone(),
            line: used.line,
            specifier: Some(used.specifier.clone()),
        })
        .collect();
    let lost: BTreeSet<Place> = input
        .unresolved
        .iter()
        .filter(|u| reached.contains(u.file.as_str()))
        .map(Unresolved::place)
        .filter(|place| !followed.contains(place))
        .collect();
    let unreached = candidates
        .into_iter()
        .filter(|f| !reached.contains(f.as_str()));
    let (unreached, held): (Vec<&String>, Vec<&String>) = if lost.is_empty() {
        (unreached.collect(), Vec::new())
    } else {
        (Vec::new(), unreached.collect())
    };

    let violations = unreached
        .into_iter()
        .map(|file| Violation {
            file: file.clone(),
            line: 1,
            message: "not reachable from any `entry`; if a framework, DI, reflection or a build tool loads it, add it to `entry`".to_string(),
            from: input.map.component_of(file).unwrap_or_default().to_string(),
            evidence: vec![Place::new(file.clone(), 1)],
            ..Violation::of(RuleId::Unused)
        })
        .collect();
    let mut reasons: Vec<Unjudged> = skipped
        .into_iter()
        .map(|(f, why)| {
            Unjudged::new(format!("`unused` did not judge the {f} files: {why}"))
                .in_files(input.paths.iter().filter(|p| family(p) == Some(f)).cloned())
        })
        .collect();
    if !held.is_empty() {
        let lost: Vec<Place> = lost.into_iter().collect();
        let named = check::name_first(&lost, LOST_SHOWN, ", ", ToString::to_string, |more| {
            format!(" and {more} more")
        });
        reasons.push(
            Unjudged::new(format!(
                "`unused` did not judge {} file(s) that no `entry` reaches, as an import the indexer did not resolve may reach them: {named}",
                held.len()
            ))
            .in_files(held.into_iter().cloned())
            .at(lost)
            .closed_by("have the indexer resolve the imports the message names (the `unresolved_imports` gap says how), then check again"),
        );
    }
    if !unreadable.is_empty() {
        reasons.push(
            Unjudged::new(format!(
                "`unused` could not read {}: it took each as used, a manifest with every file under it, and a file one of them uses may be reported unused",
                unreadable.iter().cloned().collect::<Vec<_>>().join(", ")
            ))
            .in_files(unreadable),
        );
    }
    if !unjudged.is_empty() {
        let exts: Vec<&str> = unjudged.into_iter().collect();
        reasons.push(
            Unjudged::new(format!(
                "`unused` cannot judge .{} files, whose dependency graph misses uses, so it took them as used",
                exts.join(", .")
            ))
            .in_extensions(exts),
        );
    }
    Ok(Unused {
        violations,
        reasons,
    })
}

/// Every file a chain of imports from `roots` reaches. A language that builds a
/// directory whole (a Go package) reaches all of it from one of its files, a
/// test file too; one that runs a file of every directory above a module on its
/// import (Python's `__init__.py`) reaches those.
fn reached_from<'a>(
    roots: Vec<&'a str>,
    graph: &'a BTreeMap<String, BTreeSet<(String, usize)>>,
    paths: &'a [String],
) -> BTreeSet<&'a str> {
    let mut in_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for file in paths {
        in_dir.entry(parent(file)).or_default().push(file);
    }
    let files: Vec<&str> = paths.iter().map(String::as_str).collect();
    let together: BTreeMap<(&str, &str), Vec<&str>> = registry::built_together(&files)
        .into_iter()
        .filter_map(|group| {
            let first = *group.first()?;
            Some(((parent(first), registry::of(first)?.name), group))
        })
        .collect();
    reach(roots, |file| {
        let mut next: Vec<&'a str> = graph
            .get(file)
            .into_iter()
            .flatten()
            .map(|(to, _)| to.as_str())
            .collect();
        let dir = parent(file);
        let language = registry::of(file);
        if let Some(language) = language.filter(|language| language.whole_directory) {
            next.extend(together.get(&(dir, language.name)).into_iter().flatten());
        }
        if let Some(runs) = language.and_then(|language| language.runs_on_import) {
            let mut dir = dir;
            loop {
                let init = relpath::join(dir, runs);
                next.extend(
                    in_dir
                        .get(dir)
                        .and_then(|fs| fs.iter().find(|f| **f == init)),
                );
                if dir.is_empty() {
                    break;
                }
                dir = parent(dir);
            }
        }
        next
    })
}

/// Whether a file is a program of its own by its text, as its language tells;
/// one whose text cannot be read may be one, and goes to `unreadable`.
fn program(root: &Path, file: &str, unreadable: &mut BTreeSet<String>) -> bool {
    judged_language(file)
        .and_then(|language| language.program)
        .is_some_and(|program| match std::fs::read(root.join(file)) {
            Ok(bytes) => program(&String::from_utf8_lossy(&bytes)),
            Err(_) => {
                unreadable.insert(file.to_string());
                true
            }
        })
}

/// The uses of a file that each judged file writes and its index may not
/// report, as its language finds them in its text.
fn unindexed_uses(
    root: &Path,
    paths: &[String],
    all_files: &[String],
    unreadable: &mut BTreeSet<String>,
) -> Vec<(String, UnindexedUse)> {
    let files: BTreeSet<&str> = all_files.iter().map(String::as_str).collect();
    let mut found = Vec::new();
    for file in paths {
        let Some(find) = judged_language(file).and_then(|language| language.unindexed_uses) else {
            continue;
        };
        // What a file names is ASCII, whatever the encoding of the file.
        let Ok(bytes) = std::fs::read(root.join(file)) else {
            unreadable.insert(file.clone());
            continue;
        };
        found.extend(
            find(&String::from_utf8_lossy(&bytes), file, &files)
                .into_iter()
                .map(|used| (file.clone(), used)),
        );
    }
    found
}

/// The files the manifests of the project name as what their packages run or
/// export.
fn package_entries(
    root: &Path,
    all_files: &[String],
    unreadable: &mut BTreeSet<String>,
) -> BTreeSet<String> {
    let files: BTreeSet<&str> = all_files.iter().map(String::as_str).collect();
    let mut entries = BTreeSet::new();
    for manifest in all_files {
        let Some(kind) = registry::manifest_of(manifest) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(root.join(manifest)) else {
            // What it exports is not known: every file under it may be.
            unreadable.insert(manifest.clone());
            let dir = parent(manifest);
            entries.extend(
                all_files
                    .iter()
                    .filter(|f| relpath::is_under(f, dir))
                    .cloned(),
            );
            continue;
        };
        entries.extend(kind.entries(&text, parent(manifest), &files));
    }
    entries
}

/// For the tests: a `Target` that names a project file.
#[cfg(test)]
fn internal(path: &str) -> crate::lang::Target {
    crate::lang::Target::Internal(path.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_packages_and_python_packages_come_whole() {
        let paths: Vec<String> = [
            "app/a.go",
            "app/b.go",
            "pkg/__init__.py",
            "pkg/sub/m.py",
            "pkg/sub/__init__.py",
        ]
        .map(String::from)
        .to_vec();
        let graph = BTreeMap::new();
        let reached = reached_from(vec!["app/a.go", "pkg/sub/m.py"], &graph, &paths);
        assert!(reached.contains("app/b.go"));
        assert!(reached.contains("pkg/sub/__init__.py"));
        assert!(reached.contains("pkg/__init__.py"));
        let deps = [Dep::module("x.go", 1, "", internal("app/a.go"))];
        assert_eq!(file_graph(&deps, &paths).len(), 1);
    }

    #[test]
    fn finds_side_effect_imports_and_package_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let write = |path: &str, text: &str| {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write(
            "src/main.ts",
            "import \"./polyfill\";\nconst x = require('./legacy');\n",
        );
        write("src/polyfill.ts", "");
        write("src/legacy.js", "");
        write(
            "package.json",
            r#"{"bin": {"tool": "bin/tool.js"}, "exports": {".": "./src/main.ts"}}"#,
        );
        write("bin/tool.js", "");
        let files: Vec<String> = [
            "src/main.ts",
            "src/polyfill.ts",
            "src/legacy.js",
            "package.json",
            "bin/tool.js",
        ]
        .map(String::from)
        .to_vec();
        let found = unindexed_uses(dir.path(), &files, &files, &mut BTreeSet::new());
        let used = |line, specifier: &str, to: &str| {
            (
                "src/main.ts".to_string(),
                UnindexedUse {
                    line,
                    specifier: specifier.into(),
                    to: to.into(),
                },
            )
        };
        assert_eq!(
            found,
            [
                used(1, "./polyfill", "src/polyfill.ts"),
                used(2, "./legacy", "src/legacy.js"),
            ]
        );
        let entries = package_entries(dir.path(), &files, &mut BTreeSet::new());
        assert!(entries.contains("bin/tool.js") && entries.contains("src/main.ts"));
    }
}
