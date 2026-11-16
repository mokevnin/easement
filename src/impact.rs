use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::chains::{NO_COMPONENT, describe_chain, file_graph, reach, shortest_chain};
use crate::check::{Chain, Hop, Place, RuleId, Unjudged, Violation};
use crate::components::{AcceptedGap, ComponentMap};
use crate::config::Config;
use crate::decision;
use crate::explain::DependencyDecision;
use crate::lang::{Called, Dep, Mention, Unit, registry};
use crate::policy::Policy;
use crate::relpath;
use crate::report::{self, Coverage, EvidenceGap, GapKind};
use crate::resolved::PairDecision;
use crate::symbols;

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

/// A name of the target component that the source component writes: one unit of
/// the size of the edge.
#[derive(Serialize)]
pub struct WrittenName {
    /// The name as the symbol rules write it.
    pub name: String,
    /// The file of the target that defines it, past any re-export.
    pub file: String,
    /// Every line of the source that writes it.
    pub places: Vec<Place>,
    /// Whether the policy lets it in through the public surface of the target at
    /// every place: its public files and the named surfaces open to the source
    /// (`privacy`), and its `public_symbols`. Absent when the target declares neither.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
}

/// A module of the target that the source names without writing any name through
/// it, so the index gives its uses to no name.
#[derive(Serialize)]
pub struct BareModule {
    pub file: String,
    /// Every line of the source that names it.
    pub places: Vec<Place>,
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
    /// The names of `to` that the direct dependencies write, by name: their number
    /// is the size of the edge.
    pub names: Vec<WrittenName>,
    /// The modules of `to` that the direct dependencies name without writing a name.
    pub modules: Vec<BareModule>,
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
    // Only a language whose uses of names easement reads tells the names its
    // files write; the others are a gap.
    let (read, unread): (Vec<&Dep>, Vec<&Dep>) = at
        .values()
        .flatten()
        .copied()
        .partition(|dep| registry::of(&dep.from.key()).is_none_or(|language| language.symbols));
    let (names, modules) = written_names(config, graph.map, read.into_iter(), from, to);
    let mut evidence_gaps = graph.evidence_gaps.to_vec();
    evidence_gaps.extend(names_unread(&unread, &of, from, to));
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
        evidence_gaps,
        accepted_gaps: graph.accepted.to_vec(),
    };
    Ok(Why {
        from: from.to_string(),
        to: to.to_string(),
        decision: PairDecision::of(policy, graph.map, from, to),
        owners,
        depends: !dependencies.is_empty() || chain.is_some(),
        dependencies,
        names,
        modules,
        chain,
        coverage,
        cached: None,
    })
}

/// One evidence gap for each language among `deps` that depend on `to`: easement
/// reads no uses of names from it, so the names its files write are not counted.
fn names_unread<'a>(
    deps: &[&Dep],
    of: &dyn Fn(&str) -> Option<&'a str>,
    from: &str,
    to: &str,
) -> Vec<EvidenceGap> {
    let mut by_language: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for dep in deps {
        let file = dep.from.key();
        if !dep
            .targets()
            .iter()
            .any(|target| of(&target.key()) == Some(to))
        {
            continue;
        }
        if let Some(language) = registry::of(&file) {
            by_language.entry(language.name).or_default().insert(file);
        }
    }
    by_language
        .into_iter()
        .map(|(language, files)| {
            EvidenceGap::of(
                GapKind::Symbols,
                &Unjudged::new(format!(
                    "the names of {to} that {} {language} file(s) of {from} write are not counted: easement does not read uses of names from {language}, so only their dependencies are known",
                    files.len()
                ))
                .in_files(files),
                "read the names in those files by hand: their dependencies are listed with the others",
            )
        })
        .collect()
}

/// The names of `to` that `deps` write and the modules of `to` they name bare, read
/// from the lines each dependency knows refer to its target: a module and the name
/// it is the dependency for share them, so each place counts once.
///
/// A name is public when no place takes it in by a way the surfaces of `to` keep
/// closed to `from` (`privacy`: the dependency on its file, or a module of `to` its
/// line names) and `public_symbols` lists it, as those rules judge it. That is said
/// only of a `to` that declares a public surface or `public_symbols`.
fn written_names<'a>(
    config: &Config,
    map: &ComponentMap,
    deps: impl Iterator<Item = &'a Dep>,
    from: &str,
    to: &str,
) -> (Vec<WrittenName>, Vec<BareModule>) {
    let component = &config.components[to];
    let declared = component.public_surface().is_some() || component.public_symbols.is_some();
    // Each dependency on `to`, with every file of `to` it may mean.
    let on_to: Vec<(&Dep, Vec<String>)> = deps
        .map(|dep| {
            let targets = dep
                .targets()
                .iter()
                .map(Unit::key)
                .filter(|target| map.component_of(target) == Some(to))
                .collect();
            (dep, targets)
        })
        .filter(|(_, targets): &(_, Vec<String>)| !targets.is_empty())
        .collect();
    let closed: Vec<bool> = on_to
        .iter()
        .map(|(dep, _)| declared && decision::surface_closed(map, dep))
        .collect();
    // The lines that name a module of `to` by a way its surfaces keep closed.
    let closed_lines: HashSet<(String, usize)> = on_to
        .iter()
        .zip(&closed)
        .filter(|(_, closed)| **closed)
        .flat_map(|((dep, _), _)| {
            dep.mentions
                .iter()
                .filter(|mention| mention.symbol.is_none())
                .map(|mention| (dep.from.key(), mention.line))
        })
        .collect();
    #[derive(Default)]
    struct Found {
        places: BTreeSet<(String, usize)>,
        called: Option<Called>,
        closed: bool,
    }
    let mut names: BTreeMap<(String, String), Found> = BTreeMap::new();
    let mut modules: BTreeMap<String, BTreeSet<(String, usize)>> = BTreeMap::new();
    for ((dep, targets), closed) in on_to.iter().zip(&closed) {
        let file = dep.from.key();
        // The lines that refer to each target, the uses of an alias that stands for a
        // name of it, and the names of the other files of the module it is, which no
        // dependency of their own stands for.
        let aliased = dep.aliased.iter().map(|aliased| Mention {
            line: aliased.line,
            symbol: aliased.symbol.clone(),
            called: aliased.called.clone(),
            bare: aliased.symbol.is_none(),
        });
        let own: Vec<Mention> = dep.mentions.iter().cloned().chain(aliased).collect();
        let elsewhere = dep
            .elsewhere
            .iter()
            .filter(|name| map.component_of(&name.file) == Some(to))
            .map(|name| (&name.file, name.mention.clone()));
        let referring: Vec<(&String, Mention)> = targets
            .iter()
            .flat_map(|target| own.iter().map(move |mention| (target, mention.clone())))
            .chain(elsewhere)
            .collect();
        for (target, mention) in referring {
            let place = (file.clone(), mention.line);
            let Some(symbol) = &mention.symbol else {
                if mention.bare {
                    modules.entry(target.clone()).or_default().insert(place);
                }
                continue;
            };
            let name = mention
                .called
                .as_ref()
                .map_or_else(|| symbol.clone(), Called::to_string);
            let found = names.entry((name, target.clone())).or_default();
            found.closed |= *closed || closed_lines.contains(&place);
            found.called = found.called.take().or_else(|| mention.called.clone());
            found.places.insert(place);
        }
    }
    let places = |found: BTreeSet<(String, usize)>| -> Vec<Place> {
        found
            .into_iter()
            .map(|(file, line)| Place::new(file, line))
            .collect()
    };
    let names = names
        .into_iter()
        .map(|((name, file), found)| {
            // A name the index gives no call of is one `public_symbols` does not judge.
            let listed = |called: &Called| {
                from == to
                    || symbols::outside_public_symbols(config, to, &[called.name.as_str()])
                        .is_none()
            };
            let public =
                declared.then(|| !found.closed && found.called.as_ref().is_none_or(listed));
            WrittenName {
                name,
                file,
                places: places(found.places),
                public,
            }
        })
        .collect();
    let modules = modules
        .into_iter()
        .map(|(file, found)| BareModule {
            file,
            places: places(found),
        })
        .collect();
    (names, modules)
}

impl Why {
    /// The answer as text.
    pub fn text(&self) -> String {
        render_why(self)
    }
}

/// The size of the direct edge in text: the names `from` writes of `to`, each with
/// how many places write it, then the modules it names bare.
fn size_lines(why: &Why) -> String {
    let Why { from, to, .. } = why;
    let count = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    let uncounted = why
        .coverage
        .evidence_gaps
        .iter()
        .any(|gap| gap.kind == GapKind::Symbols);
    let mut out = format!(
        "{from} writes {} of {to}{}{}\n",
        count(why.names.len(), "name", "names"),
        match uncounted {
            true => " in the files easement reads names from (see the evidence gaps)",
            false => "",
        },
        if why.names.is_empty() { "" } else { ":" }
    );
    for name in &why.names {
        out.push_str(&format!(
            "  {} ({}){}\n",
            name.name,
            count(name.places.len(), "place", "places"),
            if name.public == Some(false) {
                " [not public]"
            } else {
                ""
            }
        ));
    }
    if !why.modules.is_empty() {
        out.push_str(&format!(
            "{from} names {} of {to} without writing a name of it:\n",
            count(why.modules.len(), "module", "modules")
        ));
        for module in &why.modules {
            out.push_str(&format!(
                "  {} ({})\n",
                module.file,
                count(module.places.len(), "place", "places")
            ));
        }
    }
    out
}

/// The text form of [`why`].
pub fn render_why(why: &Why) -> String {
    let Why { from, to, .. } = why;
    let decision = &why.decision;
    let through = crate::resolved::through(decision.provenance.as_ref());
    let said = match (decision.grant, decision.rejected_by) {
        (_, Some(clause)) if clause.prohibits() => {
            format!("is forbidden ({}{through})", clause.as_str())
        }
        (Some(grant), _) => format!("is allowed ({}{through})", grant.as_str()),
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
        out.push_str(&size_lines(why));
    }
    // Only the names of a language easement reads no uses from leave the
    // dependencies whole.
    let silent = match why
        .coverage
        .evidence_gaps
        .iter()
        .all(|gap| gap.kind.hides_dependencies())
    {
        true => "a dependency or a chain through this code is missing",
        false => "a dependency, a chain or a name written in this code is missing",
    };
    out.push_str(&report::gap_lines(
        &why.coverage.evidence_gaps,
        &why.coverage.accepted_gaps,
        silent,
        false,
        false,
    ));
    if let Some(note) = &why.cached {
        out.push_str(&format!("Cached: {note}\n"));
    }
    out
}
