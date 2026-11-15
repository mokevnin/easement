//! How the policy and the files read match each other: a pattern of the policy
//! that matches nothing (`unmatched`), and a file that no component claims
//! (`orphan`).

use std::collections::BTreeSet;

use crate::check::{Place, RuleId, Violation};
use crate::components::ComponentMap;
use crate::config::Config;
use crate::lang::{Dep, Target, registry};
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
        .map(|(ban, item, pattern)| {
            let key = ban.key();
            let name = policy.config.top_bans(ban)[item].name();
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
