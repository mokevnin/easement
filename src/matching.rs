//! How the policy and the files read match each other: a pattern of the policy
//! that matches nothing (`unmatched`), and a file that no component claims
//! (`orphan`).

use std::collections::{BTreeMap, BTreeSet};

use crate::check::{Place, RuleId, Violation};
use crate::components::ComponentMap;
use crate::config::Config;
use crate::lang::{Cause, Dep, Target, registry};
use crate::policy::Policy;

/// In strict mode, every checked file must belong to a component.
pub fn orphans(config: &Config, map: &ComponentMap, files: &[String]) -> Vec<Violation> {
    if !config.strict {
        return Vec::new();
    }
    files
        .iter()
        .filter(|f| map.component_of(f).is_none() && !map.is_unowned(f))
        .map(|file| Violation {
            file: file.clone(),
            line: 1,
            message: "file belongs to no component (strict mode)".to_string(),
            target: file.clone(),
            evidence: vec![Place::new(file.clone(), 1)],
            ..Violation::of(RuleId::Orphan)
        })
        .collect()
}

/// One violation per `in` pattern that matches no file in `files`, at its
/// line in the configuration the policy was loaded from, or line 1 when a
/// file it `extends` defines the component. The
/// `tests` component of `tests: auto` and components marked `optional` may
/// match nothing.
pub fn unmatched(
    policy: &Policy,
    map: &ComponentMap,
    files: &[String],
    deps: &[Dep],
    config_file: &str,
) -> Vec<Violation> {
    let config = &policy.config;
    map.unmatched_patterns(files)
        .into_iter()
        .filter(|(component, _)| !config.components[*component].optional)
        .map(|(component, pattern)| Violation {
            file: config_file.to_string(),
            line: policy.config_line(&["components", component, "in"], pattern),
            message: format!(
                "component `{component}` matches no file (in: {pattern}); fix the pattern, or set `optional: true` if the component may be empty (files that `exclude` or .gitignore hide do not count)"
            ),
            from: component.to_string(),
            target: pattern.to_string(),
            ..Violation::of(RuleId::Unmatched)
        })
        .chain(
            unmatched_entries(config, files)
                .unwrap_or_default()
                .into_iter()
                .map(|pattern| Violation {
                    file: config_file.to_string(),
                    line: policy.config_line(&["entry"], pattern),
                    message: format!(
                        "`entry` pattern `{pattern}` matches no file, so everything it would reach looks unused; fix the pattern or drop it"
                    ),
                    target: pattern.to_string(),
                    ..Violation::of(RuleId::Unmatched)
                }),
        )
        .chain(unmatched_names(policy, config_file))
        .chain(unmatched_parts(policy, deps, config_file))
        .chain(unmatched_grants(policy, map, files, config_file))
        .collect()
}

/// An entry of `accepted_gaps` that accepts nothing, at its line: a typo, or a gap
/// gone. Like every `unmatched`, `--diff` keeps it and the baseline never accepts it.
pub fn unmatched_accepted(
    policy: &Policy,
    map: &ComponentMap,
    uncovered: &[String],
    cannot_run: &[String],
    config_file: &str,
) -> Vec<Violation> {
    map.decide_uncovered(uncovered, cannot_run)
        .stale
        .into_iter()
        .map(|entry| {
            let globs = policy.config.accepted_gaps[entry]
                .uncovered_patterns()
                .join(", ");
            Violation {
                file: config_file.to_string(),
                line: policy.config_item_line("accepted_gaps", entry),
                message: format!(
                    "`accepted_gaps[{entry}]` accepts no uncovered file (files: {globs}): an indexer reads every file it names, or none is left; drop the entry, or fix its globs (files that `exclude` or .gitignore hide do not count)"
                ),
                target: globs,
                ..Violation::of(RuleId::Unmatched)
            }
        })
        .collect()
}

/// An import whose dependency is not in the graph, as an `unresolved_imports` entry
/// of `accepted_gaps` matches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnresolvedImport<'a> {
    /// The file that writes it.
    pub file: &'a str,
    /// Its text, as the report gives it.
    pub specifier: &'a str,
    /// Why its dependency is not in the graph.
    pub cause: Cause,
    /// How its gap closes, as the report says it.
    pub close: &'a str,
}

/// What the `unresolved_imports` entries of `accepted_gaps` decide of the imports
/// whose dependency is not in the graph, each by its index among them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportsDecision {
    /// The imports no entry accepts.
    pub open: Vec<usize>,
    /// By the index in `accepted_gaps` of the entry that accepts them, in that order,
    /// the imports it accepts.
    pub accepted: Vec<(usize, Vec<usize>)>,
    /// The entries that accept none, by index in `accepted_gaps`, each with an import
    /// of its file and specifier that is unresolved for another cause, when it meets one.
    pub stale: Vec<(usize, Option<usize>)>,
}

/// What the `unresolved_imports` entries of `config` decide of `imports`. An entry
/// accepts every import of its `file` written as its `specifier`, exactly, whose
/// target is computed at run time: every other cause has a fix in the code.
pub fn decide_imports(config: &Config, imports: &[UnresolvedImport]) -> ImportsDecision {
    let mut decided = ImportsDecision::default();
    let mut accepted_by: Vec<Option<usize>> = vec![None; imports.len()];
    for (entry, accepted) in config.accepted_gaps.iter().enumerate() {
        let Some((file, specifier)) = accepted.import() else {
            continue;
        };
        let named: Vec<usize> = imports
            .iter()
            .enumerate()
            .filter(|(_, import)| import.file == file && import.specifier == specifier)
            .map(|(i, _)| i)
            .collect();
        let computed: Vec<usize> = named
            .iter()
            .copied()
            .filter(|&i| imports[i].cause == Cause::Computed)
            .collect();
        if computed.is_empty() {
            decided.stale.push((entry, named.first().copied()));
            continue;
        }
        for &i in &computed {
            accepted_by[i].get_or_insert(entry);
        }
    }
    let mut by_entry: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, entry) in accepted_by.into_iter().enumerate() {
        match entry {
            Some(entry) => by_entry.entry(entry).or_default().push(i),
            None => decided.open.push(i),
        }
    }
    decided.accepted = by_entry.into_iter().collect();
    decided
}

/// An `unresolved_imports` entry of `accepted_gaps` that accepts nothing, at its
/// line: a typo, an import gone or resolved now, or one unresolved for another
/// cause, which has a fix in the code its gap names. An entry for a file in
/// `unread` (no indexer read it, or easement could not) is not judged, since what
/// it imports is unknown.
pub fn unmatched_imports(
    policy: &Policy,
    imports: &[UnresolvedImport],
    unread: &[String],
    config_file: &str,
) -> Vec<Violation> {
    let config = &policy.config;
    decide_imports(config, imports)
        .stale
        .into_iter()
        .filter_map(|(entry, other)| {
            let (file, specifier) = config.accepted_gaps[entry].import()?;
            if unread.iter().any(|f| f == file) {
                return None;
            }
            let message = match other {
                Some(other) => format!(
                    "`accepted_gaps[{entry}]` names `{specifier}` in {file}, whose dependency is not in the graph for another cause than a target computed at run time, which cannot be accepted: {}",
                    imports[other].close
                ),
                None => format!(
                    "`accepted_gaps[{entry}]` accepts no unresolved import: {file} writes no import `{specifier}` whose target is computed at run time (it is gone, or resolved now); drop the entry, or fix its `file` and `specifier` to what the report gives in `places`"
                ),
            };
            Some(Violation {
                file: config_file.to_string(),
                line: policy.config_item_line("accepted_gaps", entry),
                message,
                target: format!("{file}: {specifier}"),
                ..Violation::of(RuleId::Unmatched)
            })
        })
        .collect()
}

/// A glob or a set of a list of components, or a key of `rules`, that covers no
/// component, and a set nothing names: each says nothing, so it is a typo or a
/// component gone.
fn unmatched_names(policy: &Policy, config_file: &str) -> Vec<Violation> {
    policy
        .config
        .expansion
        .unmatched
        .iter()
        .map(|unmatched| {
            let path: Vec<&str> = unmatched.path.iter().map(String::as_str).collect();
            let line = if unmatched.is_key {
                policy.config_key_line(&path, &unmatched.item)
            } else {
                policy.config_line(&path, &unmatched.item)
            };
            Violation {
                file: config_file.to_string(),
                line,
                message: unmatched.message.clone(),
                target: unmatched.item.clone(),
                ..Violation::of(RuleId::Unmatched)
            }
        })
        .collect()
}

/// An `allow_in` pattern of the top-level `forbid_symbol` or `forbid_external`
/// that matches no file: it lets no one use what the item forbids, so it is a
/// typo or a path gone.
fn unmatched_grants(
    policy: &Policy,
    map: &ComponentMap,
    files: &[String],
    config_file: &str,
) -> Vec<Violation> {
    map.unmatched_grants(files)
        .into_iter()
        .map(|(forbid, item, pattern)| {
            let key = forbid.key();
            let name = policy.config.top_forbidden(forbid)[item].name();
            Violation {
                file: config_file.to_string(),
                line: policy.config_line(&[key, &item.to_string(), "allow_in"], pattern),
                message: format!(
                    "`{key}` `{name}` is allowed in `{pattern}`, which matches no file; fix the pattern or drop it (files that `exclude` or .gitignore hide do not count)"
                ),
                target: pattern.to_string(),
                ..Violation::of(RuleId::Unmatched)
            }
        })
        .collect()
}

/// A `part` that names no part of the files read, unless its component is
/// `optional`: it owns nothing, so no rule on it judges anything. A typo, or
/// `packages` left off.
fn unmatched_parts(policy: &Policy, deps: &[Dep], config_file: &str) -> Vec<Violation> {
    let config = &policy.config;
    let found: BTreeSet<&str> = deps
        .iter()
        .flat_map(|dep| {
            let target = match &dep.target {
                Target::Internal(unit) => unit.part_name(),
                Target::External(_) => None,
            };
            [dep.from.part_name(), target]
        })
        .flatten()
        .collect();
    let list = if found.is_empty() {
        "none".to_string()
    } else {
        found.iter().copied().collect::<Vec<_>>().join(", ")
    };
    config
        .components
        .iter()
        .filter(|(_, component)| !component.optional)
        .filter_map(|(name, component)| {
            let part = component.part.as_deref()?;
            if found.contains(part) {
                return None;
            }
            let guess = crate::config::closest(part, found.iter().copied())
                .map(|p| format!("did you mean `{p}`? "))
                .unwrap_or_default();
            Some(Violation {
                file: config_file.to_string(),
                line: policy.config_line(&["components", name, "part"], part),
                message: format!(
                    "component `{name}`: `part: {part}` matches no part of the files read (found: {list}), so it owns nothing and no rule on it judges anything; {guess}parts come from {}; fix the part, or set `optional: true` if it may own nothing",
                    registry::part_sources()
                ),
                from: name.clone(),
                target: part.to_string(),
                ..Violation::of(RuleId::Unmatched)
            })
        })
        .collect()
}

/// Patterns of `entry` that match no file of the project, with the files
/// excluded ones count too: a stale entry makes everything below it unused.
fn unmatched_entries<'a>(config: &'a Config, files: &[String]) -> anyhow::Result<Vec<&'a str>> {
    let mut stale = Vec::new();
    for pattern in &config.entry {
        let set = crate::relpath::glob_set(std::slice::from_ref(pattern), "entry")?;
        if !files.iter().any(|f| set.is_match(f)) {
            stale.push(pattern.as_str());
        }
    }
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_accepts_each_computed_import_of_its_file_and_specifier() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  a: { in: src }
accepted_gaps:
  - { kind: uncovered, files: tools/**, because: helpers }
  - { kind: unresolved_imports, file: src/i18n.ts, specifier: './locales/*.json', because: by language }
  - { kind: unresolved_imports, file: src/i18n.ts, specifier: './locales/*.json', because: twice }
  - { kind: unresolved_imports, file: src/other.ts, specifier: './locales/*.json', because: gone }
  - { kind: unresolved_imports, file: src/side.ts, specifier: ./missing, because: not computed }
",
        )
        .unwrap();
        let import = |file, specifier, cause| UnresolvedImport {
            file,
            specifier,
            cause,
            close: "fix it",
        };
        let imports = [
            import("src/i18n.ts", "./locales/*.json", Cause::Computed),
            import("src/i18n.ts", "./plugins/*.ts", Cause::Computed),
            import("src/side.ts", "./missing", Cause::NoTarget),
            import("src/i18n.ts", "./locales/*.json", Cause::Computed),
        ];
        // The same text twice in the file: both are accepted, by the first entry
        // that names them, and the second still meets them, so it is not stale.
        assert_eq!(
            decide_imports(&config, &imports),
            ImportsDecision {
                open: vec![1, 2],
                accepted: vec![(1, vec![0, 3])],
                stale: vec![(3, None), (4, Some(2))],
            }
        );
    }

    #[test]
    fn orphans_only_in_strict_mode() {
        let yaml = |strict: bool| -> Config {
            serde_norway::from_str(&format!(
                "version: 1\ncomponents:\n  a: {{ in: a }}\nstrict: {strict}\n"
            ))
            .unwrap()
        };
        let files = ["a/x.go".to_string(), "b/y.go".to_string()];
        let strict = yaml(true);
        let map = ComponentMap::new(&strict).unwrap();
        let found = orphans(&strict, &map, &files);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "b/y.go");
        assert!(orphans(&yaml(false), &map, &files).is_empty());
    }
}
