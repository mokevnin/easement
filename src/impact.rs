use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::chains::{NO_COMPONENT, describe_chain, file_graph, reach, shortest_chain};
use crate::check::{Chain, Hop, Place, RuleId, Unjudged, Violation};
use crate::components::{AcceptedGap, ComponentMap};
use crate::decision;
use crate::explain::{DependencyDecision, PairDecision};
use crate::lang::Dep;
use crate::policy::Policy;
use crate::relpath;
use crate::report::{self, Coverage, EvidenceGap, GapKind};

/// What depends on a set of files, directly or through other files.
#[derive(Serialize)]
pub struct Impact {
    pub changed: Vec<String>,
    /// Files the change deletes: what imported them is not in the code any more.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<String>,
    pub dependents: Vec<Dependent>,
    #[serde(flatten)]
    pub coverage: Coverage,
    /// Set when the indexes came from the cache: what their key leaves out and
    /// how to index again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<String>,
}

#[derive(Serialize)]
pub struct Dependent {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

/// The files of the project that `paths` name, a file or everything under a directory.
pub fn files_under(paths: &[String], files: &[String]) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    for path in paths {
        let path = path.trim_end_matches('/');
        let before = found.len();
        found.extend(files.iter().filter(|f| relpath::is_under(f, path)).cloned());
        if found.len() == before {
            bail!("`{path}` matches no file of the project (or `exclude` hides it)");
        }
    }
    Ok(found)
}

/// Why an answer may be short: source files no indexer read, whose imports are unknown.
/// Every file that imports one of `changed`, directly or through other files.
/// `evidence_gaps` are those of the run the graph comes from: dependents through
/// that code are missing.
pub fn impact(
    map: &ComponentMap,
    deps: &[Dep],
    files: &[String],
    changed: BTreeSet<String>,
    deleted: Vec<String>,
    mut evidence_gaps: Vec<EvidenceGap>,
    accepted: Vec<AcceptedGap>,
) -> Impact {
    let mut importers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (from, out) in file_graph(deps, files) {
        for (to, _) in out {
            importers.entry(to).or_default().insert(from.clone());
        }
    }
    let reached = reach(changed.iter().map(String::as_str), |file| {
        importers
            .get(file)
            .into_iter()
            .flatten()
            .map(String::as_str)
    });
    let dependents = reached
        .into_iter()
        .filter(|file| !changed.contains(*file))
        .map(|file| Dependent {
            file: file.to_string(),
            component: map.component_of(file).map(str::to_string),
        })
        .collect();
    if !deleted.is_empty() {
        evidence_gaps.push(EvidenceGap::of(
            GapKind::Deleted,
            &Unjudged::new(format!(
                "{} deleted file(s): what depended on them is not in the code any more; `easement check` and the build show the broken dependencies",
                deleted.len()
            ))
            .in_files(deleted.iter().cloned()),
            "run `easement check` and the build to find the dependencies on the deleted files",
        ));
    }
    let coverage = Coverage {
        narrowed: Vec::new(),
        evidence_gaps,
        accepted_gaps: accepted,
    };
    Impact {
        changed: changed.into_iter().collect(),
        deleted,
        dependents,
        coverage,
        cached: None,
    }
}

/// The text form of [`impact`]: the changed files, then their dependents by component.
pub fn render(impact: &Impact, map: &ComponentMap) -> String {
    let components = |files: &mut dyn Iterator<Item = &str>| -> String {
        let names: BTreeSet<&str> = files
            .map(|f| map.component_of(f).unwrap_or(NO_COMPONENT))
            .collect();
        if names.is_empty() {
            String::new()
        } else {
            format!(" in {}", names.into_iter().collect::<Vec<_>>().join(", "))
        }
    };
    let mut out = format!(
        "Changed: {} file(s){}\n",
        impact.changed.len(),
        components(&mut impact.changed.iter().map(String::as_str))
    );
    if !impact.deleted.is_empty() {
        out.push_str(&format!("Deleted: {}\n", impact.deleted.join(", ")));
    }
    if impact.dependents.is_empty() {
        out.push_str("Nothing depends on them.\n");
    } else {
        out.push_str(&format!(
            "Depend on them: {} file(s){}\n",
            impact.dependents.len(),
            components(&mut impact.dependents.iter().map(|d| d.file.as_str()))
        ));
        let mut by_component: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for d in &impact.dependents {
            by_component
                .entry(d.component.as_deref().unwrap_or(NO_COMPONENT))
                .or_default()
                .push(&d.file);
        }
        for (component, files) in by_component {
            out.push_str(&format!("  {component}: {}\n", files.join(", ")));
        }
    }
    out.push_str(&report::gap_lines(
        &impact.coverage.evidence_gaps,
        &impact.coverage.accepted_gaps,
        "what depends on the changes through this code is missing",
        false,
        false,
    ));
    if let Some(note) = &impact.cached {
        out.push_str(&format!("Cached: {note}\n"));
    }
    out
}

/// What `why` reads from one run of the check.
pub struct Graph<'a> {
    pub map: &'a ComponentMap,
    pub deps: &'a [Dep],
    pub files: &'a [String],
    pub violations: &'a [Violation],
    /// What the run the graph comes from could not read or judge.
    pub evidence_gaps: &'a [EvidenceGap],
    pub accepted: &'a [AcceptedGap],
}

/// One dependency the source component's code writes on the target component.
#[derive(Serialize)]
pub struct DirectDependency {
    pub file: String,
    pub line: usize,
    /// The files it reaches: every file of a package directory it names.
    pub targets: Vec<String>,
    /// The decision on each dependency written at that line.
    pub decisions: Vec<DependencyDecision>,
    /// The rules `check` reports it breaks.
    pub violates: Vec<RuleId>,
    /// For each alias written there that stands for a target: the hops from the line
    /// through the definition of every alias, and the target.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<AliasPath>,
}

/// How a line reaches a file through aliases: the line, then the definition of each
/// alias from the one written to the one that names `end`.
#[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct AliasPath {
    pub hops: Vec<Hop>,
    pub end: String,
}

/// The shortest chain of dependencies from one component to another when none is direct.
#[derive(Serialize)]
pub struct WhyChain {
    #[serde(flatten)]
    pub chain: Chain,
    /// The file it ends in.
    pub end: String,
    /// The rules `check` reports the chain breaks, such as `reach`.
    pub violates: Vec<RuleId>,
}

/// Why one component depends on another.
#[derive(Serialize)]
pub struct Why {
    pub from: String,
    pub to: String,
    pub decision: PairDecision,
    /// The owner of either component, by component.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub owners: BTreeMap<String, String>,
    /// Whether the code of `from` depends on `to`, directly or through others.
    pub depends: bool,
    pub dependencies: Vec<DirectDependency>,
    /// With no direct import: the shortest chain, one witness among those that may exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<WhyChain>,
    #[serde(flatten)]
    pub coverage: Coverage,
    /// Set when the indexes came from the cache: what their key leaves out and
    /// how to index again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<String>,
}

/// Why `from` depends on `to`: what the policy says, then each import behind
/// it with its decision and the violations it raises, or the shortest chain
/// of imports when there is no direct one.
pub fn why(policy: &Policy, graph: &Graph, from: &str, to: &str) -> Result<Why> {
    let config = &policy.config;
    for name in [from, to] {
        config.require_component(name)?;
    }
    let owners = [from, to]
        .into_iter()
        .filter_map(|name| {
            config.components[name]
                .owner
                .clone()
                .map(|owner| (name.to_string(), owner))
        })
        .collect();
    let of = |file: &str| graph.map.component_of(file);
    let files = file_graph(graph.deps, graph.files);
    // One import per line, however many files of a package it reaches.
    let mut direct: BTreeMap<(&str, usize), Vec<&str>> = BTreeMap::new();
    for (file, out) in files.iter().filter(|(f, _)| of(f) == Some(from)) {
        for (target, line) in out.iter().filter(|(t, _)| of(t) == Some(to)) {
            direct
                .entry((file.as_str(), *line))
                .or_default()
                .push(target);
        }
    }
    // The dependencies at each place a direct one is written, found in one pass.
    let mut at: HashMap<(String, usize), Vec<&Dep>> = HashMap::new();
    for dep in graph.deps {
        let key = (dep.from.key(), dep.line);
        if direct.contains_key(&(key.0.as_str(), key.1)) {
            at.entry(key).or_default().push(dep);
        }
    }
    let dependencies: Vec<DirectDependency> = direct
        .into_iter()
        .map(|((file, line), targets)| {
            let mut decisions: Vec<DependencyDecision> = at
                .get(&(file.to_string(), line))
                .into_iter()
                .flatten()
                .filter_map(|dep| decision::decide(config, graph.map, dep).ok())
                .filter(|d| d.to == Some(to))
                .map(|d| DependencyDecision::of(policy, &d))
                .collect();
            decisions.sort_by(|a, b| a.target.cmp(&b.target));
            decisions.dedup_by(|a, b| a.target == b.target);
            let violates: BTreeSet<RuleId> = graph
                .violations
                .iter()
                .filter(|v| v.file == file && v.line == line && v.from == from)
                .map(|v| v.rule)
                .collect();
            let aliases: BTreeSet<AliasPath> =
                at.get(&(file.to_string(), line))
                    .into_iter()
                    .flatten()
                    // Every copy the import may mean, not only the one the
                    // adapter found first.
                    .flat_map(|dep| dep.targets().into_iter().map(move |unit| (dep, unit.key())))
                    .filter(|(_, target)| of(target) == Some(to))
                    .flat_map(|(dep, target)| {
                        dep.aliased
                            .iter()
                            .filter(|aliased| aliased.line == line)
                            .map(move |aliased| {
                                let hops: Vec<Place> =
                                    std::iter::once(Place::new(file.to_string(), line))
                                        .chain(aliased.chain.iter().map(|alias| {
                                            Place::new(alias.file.clone(), alias.line)
                                        }))
                                        .collect();
                                AliasPath {
                                    hops: Chain::of(&hops, &target, of).hops,
                                    end: target.clone(),
                                }
                            })
                    })
                    .collect();
            DirectDependency {
                file: file.to_string(),
                line,
                targets: targets.into_iter().map(str::to_string).collect(),
                decisions,
                violates: violates.into_iter().collect(),
                aliases: aliases.into_iter().collect(),
            }
        })
        .collect();
    let chain = if dependencies.is_empty() {
        let starts = files
            .keys()
            .map(String::as_str)
            .filter(|f| of(f) == Some(from));
        let is_hop = |f: &str| !matches!(of(f), Some(c) if c == from || c == to);
        shortest_chain(&files, starts, is_hop, |f| of(f) == Some(to)).map(|(hops, last)| {
            let violates: BTreeSet<RuleId> = graph
                .violations
                .iter()
                .filter(|v| v.rule == RuleId::Reach && v.from == from && v.to == to)
                .map(|v| v.rule)
                .collect();
            let hops: Vec<Place> = hops
                .into_iter()
                .map(|(file, line)| Place::new(file, line))
                .collect();
            WhyChain {
                chain: Chain::of(&hops, last, of),
                end: last.to_string(),
                violates: violates.into_iter().collect(),
            }
        })
    } else {
        None
    };
    let coverage = Coverage {
        narrowed: Vec::new(),
        evidence_gaps: graph.evidence_gaps.to_vec(),
        accepted_gaps: graph.accepted.to_vec(),
    };
    Ok(Why {
        from: from.to_string(),
        to: to.to_string(),
        decision: PairDecision::of(policy, graph.map, from, to),
        owners,
        depends: !dependencies.is_empty() || chain.is_some(),
        dependencies,
        chain,
        coverage,
        cached: None,
    })
}

impl Why {
    /// The answer as text.
    pub fn text(&self) -> String {
        render_why(self)
    }
}

/// The text form of [`why`].
pub fn render_why(why: &Why) -> String {
    let Why { from, to, .. } = why;
    let decision = &why.decision;
    let said = match (decision.grant, decision.rejected_by) {
        (_, Some(clause)) if clause.prohibits() => format!("is forbidden ({})", clause.as_str()),
        (Some(grant), _) => format!("is allowed ({})", grant.as_str()),
        (None, _) => match &decision.layers {
            Some(why) => format!("is not allowed: {why}"),
            None => "is not allowed".to_string(),
        },
    };
    let because = decision
        .because
        .as_deref()
        .map(|b| format!(": {b}"))
        .unwrap_or_default();
    let mut out = format!("{from} -> {to} {said}{because}\n");
    if let Some(importers) = &decision.allow_importers {
        out.push_str(&format!(
            "  but {to} is open only to {}\n",
            importers.join(", ")
        ));
    }
    // What a grant leaves to the path of each import.
    for condition in &decision.conditions {
        out.push_str(&format!("  but {condition}\n"));
    }
    for name in [from, to] {
        if let Some(owner) = why.owners.get(name) {
            out.push_str(&format!("  owner of {name}: {owner}\n"));
        }
    }
    if let Some(WhyChain { chain, end, .. }) = &why.chain {
        let hops: Vec<Place> = chain
            .hops
            .iter()
            .map(|hop| Place::new(hop.file.clone(), hop.line))
            .collect();
        let (through, chain) = describe_chain(&hops, end, |file| {
            chain
                .hops
                .iter()
                .find(|hop| hop.file == file)
                .and_then(|hop| hop.from_component.as_deref())
        });
        out.push_str(&format!(
            "{from} depends on nothing of {to} directly; it reaches {to} through {through}:\n  {chain}\n"
        ));
    } else if why.dependencies.is_empty() {
        out.push_str(&format!("{from} does not depend on {to}\n"));
    } else {
        out.push_str(&format!("{} dependency(ies):\n", why.dependencies.len()));
        for import in &why.dependencies {
            out.push_str(&format!(
                "  {}:{} depends on {}",
                import.file,
                import.line,
                import.targets.join(", ")
            ));
            if !import.violates.is_empty() {
                let rules: Vec<&str> = import.violates.iter().map(|r| r.as_str()).collect();
                out.push_str(&format!(" [violates: {}]", rules.join(", ")));
            }
            out.push('\n');
            for AliasPath { hops, end } in &import.aliases {
                let places: Vec<Place> = hops
                    .iter()
                    .map(|hop| Place::new(hop.file.clone(), hop.line))
                    .collect();
                let (_, chain) = describe_chain(&places, end, |_| None);
                out.push_str(&format!("    through an alias: {chain}\n"));
            }
        }
    }
    out.push_str(&report::gap_lines(
        &why.coverage.evidence_gaps,
        &why.coverage.accepted_gaps,
        "a dependency or a chain through this code is missing",
        false,
        false,
    ));
    if let Some(note) = &why.cached {
        out.push_str(&format!("Cached: {note}\n"));
    }
    out
}
