//! The `implementation` rule: which components may hold a type that implements or
//! extends a type of a component, by `allow_implementers` on the component's rule.
//! An implementation is a fact of its own, read from the relations an index
//! declares, never a dependency (ADR 0008). What the indexes cannot tell is an
//! evidence gap, asked of the registry, never of a language by name.

use std::collections::{BTreeMap, BTreeSet};

use crate::check::{Place, RuleId, Unjudged, Violation};
use crate::components::ComponentMap;
use crate::config::Config;
use crate::decision;
use crate::lang::registry::{self, Relations};
use crate::lang::{Dep, Implementation, Target, TypeDef, UnrelatedDeclaration};
use crate::relpath::slash;

/// Every type that implements or extends a type of a component whose
/// `allow_implementers` lists neither the type's component nor leaves it as the
/// component itself, at the type's definition. The definitions of every type on
/// the way to the supertype are its evidence: a change to any of them may make it.
pub fn violations(
    config: &Config,
    map: &ComponentMap,
    implementations: &[Implementation],
) -> Vec<Violation> {
    let mut found = Vec::new();
    for implementation in implementations {
        let (sub, supertype) = (&implementation.sub, &implementation.supertype);
        let Some(limit) = Limit::of(config, map, &supertype.file) else {
            continue;
        };
        // A type of no component is judged as a dependency from one is: not at all.
        let Some(home) = map.component_of(&sub.file) else {
            continue;
        };
        if limit.admits(map, &sub.file, &supertype.file) {
            continue;
        }
        let owner = limit.owner;
        let who = match limit.listed {
            [] => owner.to_string(),
            listed => listed.join(", "),
        };
        let evidence = std::iter::once(sub)
            .chain(&implementation.between)
            .chain([supertype])
            .map(|t| Place::new(t.file.clone(), t.line))
            .collect();
        found.push(Violation {
            file: sub.file.clone(),
            line: sub.line,
            message: format!(
                "{home} -> {owner}: {} implements {owner}' {}; {owner} may be implemented only in {who}",
                sub.name, supertype.name
            ),
            because: config.reason(owner).map(str::to_string),
            from: home.to_string(),
            to: owner.to_string(),
            target: format!("{} implements {}", sub.name, supertype.name),
            evidence,
            ..Violation::of(RuleId::Implementation)
        });
    }
    found
}

/// The `allow_implementers` of a component: who may implement its types.
struct Limit<'a> {
    owner: &'a str,
    /// The components that may besides the owner itself.
    listed: &'a [String],
}

impl<'a> Limit<'a> {
    /// The limit on the types of the component of `file`, when its rule sets one.
    fn of(config: &'a Config, map: &'a ComponentMap, file: &str) -> Option<Self> {
        let owner = map.component_of(file)?;
        let listed = config.rules.get(owner)?.allow_implementers.as_deref()?;
        Some(Self { owner, listed })
    }

    /// Whether a type in `file` may implement a type of the owner in `owned`: a file
    /// of no component is not judged, one of the owner only as the same-component
    /// grant takes it (the same instance, the same slice), and any file of a listed
    /// component may.
    fn admits(&self, map: &ComponentMap, file: &str, owned: &str) -> bool {
        match map.component_of(file) {
            None => true,
            Some(home) if home == self.owner => decision::same_component(map, file, owned),
            Some(home) => self.listed.iter().any(|c| c == home),
        }
    }
}

/// What the scan gives the gaps of the rule.
pub struct Scanned<'a> {
    /// The checked files.
    pub files: &'a [String],
    pub deps: &'a [Dep],
    /// The types checked files define.
    pub types: &'a [TypeDef],
    /// The declarations whose index gives no relations.
    pub unrelated: &'a [UnrelatedDeclaration],
}

/// Why `implementation` could not judge some types, for each component whose rule
/// sets `allow_implementers`, outside what may implement it: the files of a
/// language without relations that depend on it (implementing one of its types
/// writes its name there), every file of a language whose relations are structural
/// once it defines types of that language (implementing writes nothing there), and
/// the declarations whose index gives no relations that write one of its names.
/// Each reason is the registry's.
pub fn gaps(config: &Config, map: &ComponentMap, scanned: &Scanned) -> Vec<Unjudged> {
    let mut found = Vec::new();
    for owner in config.rules.keys() {
        let Some(listed) = config.rules[owner].allow_implementers.as_deref() else {
            continue;
        };
        let limit = Limit { owner, listed };
        // Files without relations that depend on the owner, by language.
        let mut depending: BTreeMap<&str, (&str, BTreeSet<String>)> = BTreeMap::new();
        for dep in scanned.deps {
            let file = slash(&dep.file);
            let Some((language, because)) =
                registry::of(&file).and_then(|language| match language.relations {
                    Relations::None { because } => Some((language.name, because)),
                    _ => None,
                })
            else {
                continue;
            };
            let Target::Internal(unit) = &dep.target else {
                continue;
            };
            let owned = unit.key();
            if map.component_of(&owned) == Some(owner) && !limit.admits(map, &file, &owned) {
                depending
                    .entry(language)
                    .or_insert((because, BTreeSet::new()))
                    .1
                    .insert(file);
            }
        }
        for (language, (because, files)) in depending {
            found.push(unjudged(
                files.len(),
                &format!("{language} files that depend on {owner} outside the components that may implement it"),
                because,
                &format!("whether a type in them implements a type of {owner}"),
            ).in_files(files));
        }
        // Every file of a structural language, once the owner defines types of it,
        // that some of those types may not be implemented from.
        let mut structural: BTreeMap<&str, (&str, Vec<&str>)> = BTreeMap::new();
        for owned in scanned
            .types
            .iter()
            .filter(|t| map.component_of(&t.file) == Some(owner))
        {
            if let Some(language) = registry::of(&owned.file)
                && let Relations::Structural { because } = language.relations
            {
                structural
                    .entry(language.name)
                    .or_insert((because, Vec::new()))
                    .1
                    .push(&owned.file);
            }
        }
        for (language, (because, owned)) in structural {
            let files: Vec<&String> = scanned
                .files
                .iter()
                .filter(|file| registry::of(file).is_some_and(|l| l.name == language))
                .filter(|file| owned.iter().any(|owned| !limit.admits(map, file, owned)))
                .collect();
            if !files.is_empty() {
                found.push(
                    unjudged(
                        files.len(),
                        &format!(
                            "{language} files outside the components that may implement {owner}"
                        ),
                        because,
                        &format!("whether a type in them implements a type of {owner}"),
                    )
                    .in_files(files.into_iter().cloned()),
                );
            }
        }
        // Declarations without relations that write a name of the owner.
        let mut declarations: BTreeMap<(&str, &str), Vec<&UnrelatedDeclaration>> = BTreeMap::new();
        for declaration in scanned.unrelated {
            let outside = declaration.writes.iter().any(|owned| {
                map.component_of(owned) == Some(owner)
                    && !limit.admits(map, &declaration.file, owned)
            });
            if outside && let Some(language) = registry::of(&declaration.file) {
                declarations
                    .entry((language.name, declaration.because))
                    .or_default()
                    .push(declaration);
            }
        }
        for ((language, because), sites) in declarations {
            found.push(
                unjudged(
                    sites.len(),
                    &format!("{language} declarations that write a name of {owner} outside the components that may implement it"),
                    because,
                    &format!("whether they implement or extend a type of {owner}"),
                )
                .in_files(sites.iter().map(|d| d.file.clone()))
                .at(sites.iter().map(|d| Place::new(d.file.clone(), d.line))),
            );
        }
    }
    found
}

/// The gap of `count` things `what` names that the rule could not judge, because of
/// `because`, so that `unchecked` was not checked.
fn unjudged(count: usize, what: &str, because: &str, unchecked: &str) -> Unjudged {
    Unjudged::new(format!(
        "`implementation` cannot judge {count} {what}: {because}, so {unchecked} was not checked"
    ))
}
