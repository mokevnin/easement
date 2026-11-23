use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::check::{ComponentDep, RuleId, Violation};
use crate::components::ComponentMap;
use crate::config::Config;
use crate::decision::{self, Grant, Refusal};
use crate::lang::Dep;
use crate::policy::Policy;
use crate::report::{self, AcceptedGap, EvidenceGap};
use crate::{patch, relpath};

/// What `suggest` says of a `may_depend_on` entry no import uses, and what
/// `check --suggest` says of each.
const REMOVABLE: &str =
    "allowed in may_depend_on but never depended on (remove to tighten the rules)";

/// What the current dependency graph says about the config: allowances nothing
/// uses, dependencies nothing allows, stale diagram arrows, and components that
/// look isolated already. An item next to files no indexer read says how many
/// (`unread`): the graph cannot vouch for it.
#[derive(Debug, Default, Serialize)]
pub struct Suggestions {
    /// `may_depend_on` entries no dependency uses, between components read in full:
    /// safe to remove.
    pub removable: Vec<Pair>,
    /// `may_depend_on` entries no dependency uses among the files read, next to files
    /// no indexer read: the dependency may be there.
    pub unreliable: Vec<Pair>,
    /// Dependencies between components that nothing allows.
    pub not_allowed: Vec<Used>,
    /// Dependencies between components that the policy forbids.
    pub forbidden: Vec<Used>,
    /// Arrows of the diagram that no dependency stands for.
    pub stale_arrows: Vec<Pair>,
    /// Components with instances that never depend on each other.
    pub isolation: Vec<Isolated>,
}

/// The suggestions with what the graph they come from could not read, and
/// whether its indexes came from the cache: what not to trust.
#[derive(Debug, Serialize)]
pub struct Report {
    #[serde(flatten)]
    pub suggestions: Suggestions,
    pub evidence_gaps: Vec<EvidenceGap>,
    pub accepted_gaps: Vec<AcceptedGap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<String>,
}

impl Report {
    /// The suggestions, then the gaps and the cache note.
    pub fn text(&self) -> String {
        let mut out = self.suggestions.text();
        out.push_str(&report::gap_lines(
            &self.evidence_gaps,
            &self.accepted_gaps,
            "a suggestion about this code may be wrong",
            false,
            false,
        ));
        if let Some(note) = &self.cached {
            out.push_str(&format!("easement: {note}\n"));
        }
        out
    }
}

/// Two components, and how many files of either no indexer read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pair {
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unread: BTreeMap<String, usize>,
}

/// The heading of the `may_depend_on` entries that no import among the files read
/// uses, next to files no indexer read.
const UNRELIABLE: &str = "Not depended on among the indexed files, but some files were not indexed, so the dependency may be there (do not remove without checking):";

impl Pair {
    /// `from -> to (not indexed: n file(s) of c)`, for an entry next to unread files.
    pub fn unreliable_line(&self) -> String {
        let unread = self
            .unread
            .iter()
            .map(|(c, n)| format!("{n} file(s) of {c}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} -> {} (not indexed: {unread})", self.from, self.to)
    }
}

/// The heading of the `may_depend_on` entries no import uses that come through a
/// glob or a set, which `suggest --patch` leaves alone.
const HELD: &str = "Not depended on by these components, but allowed through a glob or a set that covers others too, so `check --suggest` does not fail on them (remove one by hand if no component it covers needs it):";

/// The `may_depend_on` entries no import uses that `check --suggest` does not fail
/// on, since it cannot tell whether they can go: what a reader must not take as judged.
#[derive(Debug, Default, Serialize)]
pub struct Spared {
    /// Next to files no indexer read: the dependency may be in one of them.
    #[serde(
        rename = "unreliable_allowances",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub unreliable: Vec<Pair>,
    /// Allowed through a glob rule key or a set, which may let in another component
    /// that uses it.
    #[serde(rename = "held_allowances", skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<Pair>,
}

impl Spared {
    /// Each entry under its heading, as the text report gives them.
    pub fn text(&self) -> String {
        let mut out = String::new();
        section(
            &mut out,
            UNRELIABLE,
            self.unreliable.iter().map(Pair::unreliable_line).collect(),
        );
        section(
            &mut out,
            HELD,
            self.held
                .iter()
                .map(|p| format!("{} -> {}", p.from, p.to))
                .collect(),
        );
        out
    }

    /// One line per entry, for the formats that give notes one by one.
    pub fn notes(&self) -> Vec<String> {
        self.unreliable
            .iter()
            .map(|p| {
                format!(
                    "unused allowance not judged, next to files no indexer read: {}",
                    p.unreliable_line()
                )
            })
            .chain(self.held.iter().map(|p| {
                format!(
                    "unused allowance not judged, allowed through a glob or a set that covers other components too: {} -> {}",
                    p.from, p.to
                )
            }))
            .collect()
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What `check --suggest` judges of `may_depend_on`: a violation of
/// `unused-allowance` for each entry no import uses that `suggest --patch` would
/// remove, at the line of `config_file` that writes it, and the entries it spares:
/// those next to files no indexer read (`unindexed`), and those that come through
/// a glob or a set.
pub fn unused_allowances(
    policy: &Policy,
    map: &ComponentMap,
    component_deps: &[ComponentDep],
    unindexed: &[String],
    config_file: &str,
) -> (Vec<Violation>, Spared) {
    let config = &policy.config;
    let counts = counts(map, component_deps, unindexed);
    let found = unused(config, &counts.used, &counts.unindexed);
    let removable: Vec<(String, String)> = found
        .removable
        .into_iter()
        .map(|p| (p.from, p.to))
        .collect();
    // What comes through a glob or a set may be used by another component it
    // covers: the patch leaves it, and so does the check.
    let (plain, _notes) = patch::by_name(config, removable.clone());
    let held = removable
        .into_iter()
        .filter(|pair| !plain.contains(pair))
        .map(|(from, to)| Pair {
            from,
            to,
            unread: BTreeMap::new(),
        })
        .collect();
    let violations = plain
        .into_iter()
        .map(|(from, to)| Violation {
            file: config_file.to_string(),
            line: policy.config_line(&["rules", &from, "may_depend_on"], &to),
            message: format!("`{from}` -> `{to}`: {REMOVABLE}"),
            provenance: decision::grant_site(config, &from, &to, Grant::MayDependOn)
                .map(|site| policy.provenance(&site)),
            target: to.clone(),
            from,
            to,
            ..Violation::of(RuleId::UnusedAllowance)
        })
        .collect();
    let spared = Spared {
        unreliable: found.unreliable,
        held,
    };
    (violations, spared)
}

/// A dependency between two components, and how many there are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Used {
    pub from: String,
    pub to: String,
    pub count: usize,
}

/// A component whose instances never depend on each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Isolated {
    pub component: String,
    pub pattern: String,
    pub instances: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread: Option<usize>,
}

/// What the graph says about the config.
pub fn suggest(
    config: &Config,
    map: &ComponentMap,
    component_deps: &[ComponentDep],
    deps: &[Dep],
    files: &[String],
    uncovered: &[String],
    stale_arrows: &[(String, String)],
) -> Suggestions {
    let Counts { unindexed, used } = counts(map, component_deps, uncovered);
    let unread_of = |pair: [&str; 2]| unread_of(&unindexed, pair);
    let Unused {
        removable,
        unreliable,
    } = unused(config, &used, &unindexed);
    let mut found = Suggestions {
        removable,
        unreliable,
        ..Suggestions::default()
    };
    // A forbidden dependency cannot be allowed: the config would contradict itself,
    // or route around a closed or sibling layer.
    for ((from, to), count) in &used {
        let list = match decision::pair(config, map, from, to).grant {
            Ok(_) => continue,
            Err(Refusal::NotGranted { layers: None }) => &mut found.not_allowed,
            Err(_) => &mut found.forbidden,
        };
        list.push(Used {
            from: from.to_string(),
            to: to.to_string(),
            count: *count,
        });
    }
    found.stale_arrows = stale_arrows
        .iter()
        .map(|(from, to)| Pair {
            from: from.clone(),
            to: to.clone(),
            // A dependency of it may sit in a file no indexer read.
            unread: unread_of([from, to]),
        })
        .collect();
    found.isolation = isolation_candidates(config, map, deps, files, &unindexed);
    found
}

impl Suggestions {
    /// The suggestions as a reader takes them in, section by section.
    pub fn text(&self) -> String {
        let mut out = String::new();
        section(
            &mut out,
            &capitalized(&format!("{REMOVABLE}:")),
            self.removable
                .iter()
                .map(|p| format!("{} -> {}", p.from, p.to))
                .collect(),
        );
        section(
            &mut out,
            UNRELIABLE,
            self.unreliable.iter().map(Pair::unreliable_line).collect(),
        );
        let by_source = |list: &[Used]| -> Vec<String> {
            let mut grouped: BTreeMap<&str, Vec<String>> = BTreeMap::new();
            for u in list {
                grouped
                    .entry(&u.from)
                    .or_default()
                    .push(format!("{} ({} dependency(ies))", u.to, u.count));
            }
            grouped
                .into_iter()
                .map(|(from, targets)| format!("{from}: {}", targets.join(", ")))
                .collect()
        };
        section(
            &mut out,
            "Depended on but not allowed (add to may_depend_on, or fix the dependencies):",
            by_source(&self.not_allowed),
        );
        section(
            &mut out,
            "Depended on but forbidden by the config (fix the dependencies):",
            by_source(&self.forbidden),
        );
        section(
            &mut out,
            "Arrows of the diagram that no dependency stands for (drop them, or the dependency is still to come):",
            self.stale_arrows
                .iter()
                .map(|p| {
                    let n: usize = p.unread.values().sum();
                    if n == 0 {
                        format!("{} -> {}", p.from, p.to)
                    } else {
                        format!(
                            "{} -> {} (unreliable: {n} file(s) not indexed)",
                            p.from, p.to
                        )
                    }
                })
                .collect(),
        );
        section(
            &mut out,
            "Instances that never depend on each other (candidates for `isolated: true`):",
            self.isolation
                .iter()
                .map(|i| {
                    let note = match i.unread {
                        Some(n) => format!(" [unreliable: {n} file(s) not indexed]"),
                        None => String::new(),
                    };
                    format!(
                        "{} (`{}`): {}{note}",
                        i.component,
                        i.pattern,
                        i.instances.join(", ")
                    )
                })
                .collect(),
        );
        if out.is_empty() {
            out.push_str("Nothing to suggest: the config matches the dependency graph.\n");
        }
        out
    }
}

/// The `may_depend_on` entries that can go: no import uses them, and every file of both
/// components was read.
pub fn removable(
    config: &Config,
    map: &ComponentMap,
    component_deps: &[ComponentDep],
    uncovered: &[String],
) -> Vec<(String, String)> {
    let Counts { unindexed, used } = counts(map, component_deps, uncovered);
    unused(config, &used, &unindexed)
        .removable
        .into_iter()
        .map(|pair| (pair.from, pair.to))
        .collect()
}

/// The `may_depend_on` entries no import uses, apart by whether the graph can vouch for it.
struct Unused {
    /// Between components read in full: safe to remove.
    removable: Vec<Pair>,
    /// Next to files no indexer read: the dependency may be there.
    unreliable: Vec<Pair>,
}

/// The `may_depend_on` entries of `config` that no dependency in `used` takes.
fn unused(
    config: &Config,
    used: &BTreeMap<(&str, &str), usize>,
    unindexed: &BTreeMap<&str, usize>,
) -> Unused {
    let mut found = Unused {
        removable: Vec::new(),
        unreliable: Vec::new(),
    };
    for (from, rule) in &config.rules {
        for to in rule.may_depend_on.iter().map(|l| l.name()) {
            if used.contains_key(&(from.as_str(), to)) {
                continue;
            }
            let pair = Pair {
                from: from.clone(),
                to: to.to_string(),
                unread: unread_of(unindexed, [from, to]),
            };
            if pair.unread.is_empty() {
                found.removable.push(pair);
            } else {
                found.unreliable.push(pair);
            }
        }
    }
    found
}

/// How many files of each of `pair` no indexer read, for those with any.
fn unread_of(unindexed: &BTreeMap<&str, usize>, pair: [&str; 2]) -> BTreeMap<String, usize> {
    pair.into_iter()
        .filter_map(|c| Some((c.to_string(), *unindexed.get(c)?)))
        .collect()
}

/// What `suggest` reads off the graph: the files of each component that no indexer
/// read, and the dependencies between each pair of components.
struct Counts<'a> {
    /// Files no indexer saw have no dependencies: a missing one next to them proves nothing.
    unindexed: BTreeMap<&'a str, usize>,
    used: BTreeMap<(&'a str, &'a str), usize>,
}

fn counts<'a>(
    map: &'a ComponentMap,
    component_deps: &'a [ComponentDep],
    uncovered: &[String],
) -> Counts<'a> {
    let mut unindexed: BTreeMap<&str, usize> = BTreeMap::new();
    for file in uncovered {
        if let Some(component) = map.component_of(file) {
            *unindexed.entry(component).or_default() += 1;
        }
    }
    let mut used: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for component_dep in component_deps {
        *used
            .entry((&component_dep.from, &component_dep.to))
            .or_default() += 1;
    }
    Counts { unindexed, used }
}

fn section(out: &mut String, title: &str, items: Vec<String>) {
    if items.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(title);
    out.push('\n');
    for item in items {
        out.push_str(&format!("  {item}\n"));
    }
}

/// Components whose `in` pattern has a `*` (one directory per instance), that
/// are not isolated yet, have at least two instances and no import between them.
fn isolation_candidates(
    config: &Config,
    map: &ComponentMap,
    deps: &[Dep],
    files: &[String],
    unindexed: &BTreeMap<&str, usize>,
) -> Vec<Isolated> {
    // (component, pattern) -> instances seen, and whether instances import each other
    let mut groups: BTreeMap<(String, String), (BTreeSet<String>, bool)> = BTreeMap::new();
    let instance = |file: &str, pattern: &str| {
        let n = pattern.split('/').count();
        file.split('/').take(n).collect::<Vec<_>>().join("/")
    };
    let locate = |file: &str| {
        let (component, pattern) = map.component_and_pattern(file)?;
        // `**` spans directory levels, so it cannot tell instances apart.
        if config.components[component].isolated || !pattern.contains('*') || pattern.contains("**")
        {
            return None;
        }
        Some((
            (component.to_string(), pattern.to_string()),
            instance(file, pattern),
        ))
    };
    for file in files {
        if let Some((key, here)) = locate(file) {
            groups.entry(key).or_default().0.insert(here);
        }
    }
    for dep in deps {
        let targets = dep.targets();
        if targets.is_empty() {
            continue;
        }
        let file = dep.from.key();
        let Some((key, here)) = locate(&file) else {
            continue;
        };
        let entry = groups.entry(key.clone()).or_default();
        entry.0.insert(here.clone());
        // Every copy the import may mean: an instance that reaches another
        // through any of them depends on it.
        for target in &targets {
            if let Some((there_key, there)) = locate(&target.key())
                && there_key == key
                && there != here
            {
                entry.1 = true;
            }
        }
    }
    groups
        .into_iter()
        .filter(|(_, (instances, crossing))| instances.len() > 1 && !crossing)
        .map(|((component, pattern), (instances, _))| Isolated {
            unread: unindexed.get(component.as_str()).copied(),
            instances: instances
                .iter()
                .map(|i| relpath::file_name(i).to_string())
                .collect(),
            component,
            pattern,
        })
        .collect()
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::lang::Target;

    fn dep(file: &str, target: &str) -> Dep {
        Dep::module(file, 1, "", Target::Internal(target.into()))
    }

    fn component_dep(from: &str, to: &str) -> ComponentDep {
        ComponentDep {
            from: from.into(),
            to: to.into(),
            file: format!("{from}/x"),
            line: 1,
        }
    }

    #[test]
    fn suggests_removals_additions_and_isolation() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  web: { in: src/web }
  core: { in: src/core }
  data: { in: src/data }
  pages: { in: 'src/pages/*' }
rules:
  web: { may_depend_on: [core, data] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let component_deps = [component_dep("web", "core"), component_dep("core", "data")];
        let deps = [
            dep("src/pages/a/x.ts", "src/core/c.ts"),
            dep("src/pages/b/y.ts", "src/pages/b/z.ts"),
        ];
        let files = [
            "src/pages/a/x.ts".to_string(),
            "src/pages/b/y.ts".to_string(),
            "src/pages/b/z.ts".to_string(),
            "src/pages/c/leaf.ts".to_string(),
        ];
        let out = suggest(&config, &map, &component_deps, &deps, &files, &[], &[]).text();
        assert!(out.contains("  web -> data\n"), "{out}");
        assert!(!out.contains("web -> core"), "{out}");
        assert!(out.contains("  core: data (1 dependency(ies))\n"), "{out}");
        assert!(out.contains("  pages (`src/pages/*`): a, b, c\n"), "{out}");

        let crossing = [dep("src/pages/a/x.ts", "src/pages/b/y.ts")];
        assert!(
            !suggest(&config, &map, &component_deps, &crossing, &files, &[], &[])
                .text()
                .contains("pages")
        );
    }

    #[test]
    fn does_not_trust_a_missing_import_next_to_files_nothing_indexed() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  web: { in: src/web }
  core: { in: src/core }
  generated: { in: src/generated }
rules:
  web: { may_depend_on: [core, generated] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let component_deps = [component_dep("web", "core")];
        let uncovered = ["src/generated/index.ts".to_string()];
        let out = suggest(&config, &map, &component_deps, &[], &[], &uncovered, &[]).text();
        assert!(
            out.contains("  web -> generated (not indexed: 1 file(s) of generated)\n"),
            "{out}"
        );
        // It is not offered as a plain removal.
        assert!(!out.contains("  web -> generated\n"), "{out}");
        let clean = suggest(&config, &map, &component_deps, &[], &[], &[], &[]).text();
        assert!(clean.contains("  web -> generated\n"), "{clean}");
    }

    #[test]
    fn a_double_star_pattern_is_never_an_isolation_candidate() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  routes: { in: 'src/routes/**' }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let files = [
            "src/routes/a/x.ts".to_string(),
            "src/routes/b/y.ts".to_string(),
        ];
        let out = suggest(&config, &map, &[], &[], &files, &[], &[]).text();
        assert!(!out.contains("isolated"), "{out}");
    }

    #[test]
    fn isolation_candidates_with_unindexed_files_are_marked() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  pages: { in: 'src/pages/*' }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let files = [
            "src/pages/a/x.ts".to_string(),
            "src/pages/b/y.ts".to_string(),
        ];
        let uncovered = ["src/pages/c/z.ts".to_string()];
        let out = suggest(&config, &map, &[], &[], &files, &uncovered, &[]).text();
        assert!(out.contains("[unreliable: 1 file(s) not indexed]"), "{out}");
    }
}
