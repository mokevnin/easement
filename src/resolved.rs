//! The policy as loaded, read back as data: what `easement policy` prints, and
//! what `explain` reads its answer for one file from. `extends` is merged, the
//! anchors, globs and sets are expanded, and each item carries the `provenance`
//! of where the policy writes it. Nothing here reads the code.

use serde::Serialize;

use crate::check::RuleId;
use crate::components::ComponentMap;
use crate::config::{
    AcceptedEntry, Component, Config, Forbid, ForbiddenName, Listed, NoCycles, Reach, Rule, TESTS,
};
use crate::decision::{self, ClauseId, Grant, Refusal};
use crate::policy::{InputKind, Policy, Provenance, Site};
use crate::symbols::{Granted, Item, bindings, own_symbol_spelling};

/// The decision on every dependency of one component on another, before
/// the paths of a dependency are known.
#[derive(Debug, Serialize)]
pub struct PairDecision {
    /// Whether the policy lets the dependency stand, the paths of each
    /// import aside (see `conditions`).
    pub allowed: bool,
    /// The grant that permits it, even when `allow_importers` then rejects it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<Grant>,
    /// The clause that rejects it: `must_not_depend_on`, `must_not_reach`,
    /// `not_granted` or `allow_importers`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_by: Option<ClauseId>,
    /// The rules a rejected dependency breaks: `dependency` for a direct
    /// dependency, and `reach` for a chain of them when `must_not_reach` rejects it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes the clause that rejects it, or else the grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    /// Why `layers` keep the two apart, when they do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layers: Option<String>,
    /// `allow_importers` of the target, when it does not list the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_importers: Option<Vec<String>>,
    /// What an allowed dependency leaves to the paths of each import.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
}

impl PairDecision {
    /// The decision on `from` depending on `to`.
    pub fn of(policy: &Policy, map: &ComponentMap, from: &str, to: &str) -> Self {
        let config = &policy.config;
        let verdict = decision::pair(config, map, from, to);
        let importers = verdict.importers.map(<[String]>::to_vec);
        let (rejected_by, site) = match (&verdict.grant, &importers) {
            (Err(refusal), _) => (
                Some(refusal.id()),
                decision::refusal_site(config, from, to, refusal),
            ),
            (Ok(_), Some(_)) => (
                Some(ClauseId::AllowImporters),
                Some(Site::of_entry(
                    "components",
                    to,
                    format!("components.{to}.allow_importers"),
                )),
            ),
            (Ok(grant), None) => (None, decision::grant_site(config, from, to, *grant)),
        };
        let allowed = !verdict.rejected();
        Self {
            allowed,
            grant: verdict.grant.as_ref().ok().copied(),
            rejected_by,
            rules: match (allowed, rejected_by) {
                (true, _) => Vec::new(),
                (false, Some(ClauseId::MustNotReach)) => vec![RuleId::Dependency, RuleId::Reach],
                (false, _) => vec![RuleId::Dependency],
            },
            because: verdict.because.map(str::to_string),
            provenance: site.map(|site| policy.provenance(&site)),
            layers: match &verdict.grant {
                Err(Refusal::NotGranted { layers }) => layers.clone(),
                _ => None,
            },
            allow_importers: importers,
            conditions: if allowed {
                verdict.conditions(to)
            } else {
                Vec::new()
            },
        }
    }

    /// Whether a prohibition rejects it.
    fn prohibited(&self) -> bool {
        self.rejected_by.is_some_and(ClauseId::prohibits)
    }
}

/// The decision on depending on one other component.
#[derive(Debug, Serialize)]
pub struct Toward {
    pub to: String,
    #[serde(flatten)]
    pub decision: PairDecision,
}

/// The decisions of one component on every other, sorted by how they end.
pub struct Dependencies {
    /// The components it may depend on, with the grant of each.
    pub grants: Vec<Toward>,
    /// The components a prohibition keeps it off.
    pub prohibitions: Vec<Toward>,
    /// The other components it may not depend on: nothing grants it, or
    /// their `allow_importers` does not list it.
    pub not_allowed: Vec<Toward>,
}

impl Dependencies {
    /// The decision of `name` on each other component.
    pub fn of(policy: &Policy, map: &ComponentMap, name: &str) -> Self {
        let mut found = Self {
            grants: Vec::new(),
            prohibitions: Vec::new(),
            not_allowed: Vec::new(),
        };
        for to in policy
            .config
            .components
            .keys()
            .filter(|c| c.as_str() != name)
        {
            let decision = PairDecision::of(policy, map, name, to);
            let list = if decision.allowed {
                &mut found.grants
            } else if decision.prohibited() {
                &mut found.prohibitions
            } else {
                &mut found.not_allowed
            };
            list.push(Toward {
                to: to.clone(),
                decision,
            });
        }
        found
    }
}

/// A named surface of a component.
#[derive(Debug, Serialize)]
pub struct SurfaceView {
    pub name: String,
    pub paths: Vec<String>,
    pub importers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub provenance: Provenance,
}

/// The named surfaces of the component `name`.
pub fn surfaces(policy: &Policy, name: &str) -> Vec<SurfaceView> {
    policy.config.components[name]
        .surfaces
        .iter()
        .map(|(surface, declared)| SurfaceView {
            name: surface.clone(),
            paths: strings(declared.patterns.as_vec()),
            importers: declared.importers.clone(),
            because: declared.because.clone(),
            provenance: policy.provenance(&Site::of_entry(
                "components",
                name,
                format!("components.{name}.surfaces.{surface}"),
            )),
        })
        .collect()
}

/// An item of a rule list, with the reason given for it.
#[derive(Debug, Serialize)]
pub struct Named {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the rule writes the item: given for the names and modules the rule
    /// forbids, whose lists keep the index they are written at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

fn named(items: &[Listed]) -> Vec<Named> {
    items
        .iter()
        .map(|l| Named {
            name: l.name().to_string(),
            because: l.because().map(str::to_string),
            provenance: None,
        })
        .collect()
}

/// The items the own list `forbid` holds in the rule of `component`, each with
/// where the rule writes it.
fn forbidden(policy: &Policy, component: &str, forbid: Forbid) -> Vec<Named> {
    let config = &policy.config;
    config
        .rules
        .get(component)
        .map_or(&[][..], |rule| rule.own_forbidden(forbid))
        .iter()
        .enumerate()
        .map(|(i, item)| Named {
            name: item.name().to_string(),
            because: item.because().map(str::to_string),
            provenance: Some(policy.provenance(&Site::of_rule(
                config,
                component,
                &format!(".{}[{i}]", forbid.key()),
            ))),
        })
        .collect()
}

/// What the rule of a component asks besides its dependencies on others.
#[derive(Debug, Serialize)]
pub struct RuleView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub may_use_external: Option<Vec<Named>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbid_external: Vec<Named>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub must_depend_on: Vec<Named>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub must_use_external: Vec<Named>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires_file: Vec<Named>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbid_file: Vec<Named>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires_directive: Vec<Named>,
    /// The components that may implement or extend its types besides itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_implementers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fan_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_instability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub provenance: Provenance,
}

/// Who may implement or extend the types of a component besides itself, as a
/// line of text: `allow_implementers: adapters, tests (and itself)`.
pub fn implementers_line(implementers: &[String]) -> String {
    match implementers {
        [] => "allow_implementers: none (only itself)".to_string(),
        listed => format!("allow_implementers: {} (and itself)", listed.join(", ")),
    }
}

impl RuleView {
    /// The rule of the component `name`, when it has one.
    pub fn of(policy: &Policy, name: &str) -> Option<Self> {
        let config = &policy.config;
        // Every list, so that a new one cannot be left out: those of components are
        // the decisions on the others, the forbidden ones are read with where each is
        // written, and `forbid_symbol` the policy lists apart.
        let Rule {
            may_depend_on: _,
            must_not_depend_on: _,
            must_not_reach: _,
            may_use_external,
            forbid_external: _,
            forbid_symbol: _,
            must_depend_on,
            must_use_external,
            requires_file,
            forbid_file,
            requires_directive,
            allow_implementers,
            max_fan_out,
            max_instability,
            because,
        } = config.rules.get(name)?;
        Some(Self {
            may_use_external: may_use_external.as_deref().map(named),
            forbid_external: forbidden(policy, name, Forbid::External),
            must_depend_on: named(must_depend_on),
            must_use_external: named(must_use_external),
            requires_file: named(requires_file),
            forbid_file: named(forbid_file),
            requires_directive: named(requires_directive),
            allow_implementers: allow_implementers.clone(),
            max_fan_out: *max_fan_out,
            max_instability: *max_instability,
            because: because.clone(),
            provenance: policy.provenance(&Site::of_rule(config, name, "")),
        })
    }

    /// Its lines in text, each a clause of the rule.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.provenance.through.is_empty() {
            lines.push(format!("rule: written under {}", self.provenance.field));
        }
        let names = |items: &[Named]| {
            items
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(external) = &self.may_use_external {
            let only = if external.is_empty() {
                "nothing".to_string()
            } else {
                names(external)
            };
            lines.push(format!("external modules: only {only}"));
        }
        for (label, items) in [
            ("forbidden external modules", &self.forbid_external),
            ("every directory must depend on", &self.must_depend_on),
            ("every directory must use external", &self.must_use_external),
            ("every directory must contain", &self.requires_file),
            ("forbidden files", &self.forbid_file),
            ("every directory must match", &self.requires_directive),
        ] {
            if !items.is_empty() {
                lines.push(format!("{label}: {}", names(items)));
            }
        }
        if let Some(implementers) = &self.allow_implementers {
            lines.push(implementers_line(implementers));
        }
        if let Some(max) = self.max_fan_out {
            lines.push(format!("max_fan_out: {max}"));
        }
        if let Some(max) = self.max_instability {
            lines.push(format!("max_instability: {max}"));
        }
        if let Some(because) = &self.because {
            lines.push(format!("because: {because}"));
        }
        lines
    }
}

/// ` through `api-*`, `@drivers``: the globs and sets that bring a clause to the
/// component it is about, as text follows the clause with them; empty when the
/// policy names the component.
pub fn through(provenance: Option<&Provenance>) -> String {
    through_names(provenance.map_or(&[], |p| p.through.as_slice()))
}

/// ` through `api-*`, `@drivers``, for the globs and sets `through`; empty for none.
pub fn through_names(through: &[String]) -> String {
    match through {
        [] => String::new(),
        through => format!(
            " through {}",
            through
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `: because`, or nothing without a reason.
fn reason(because: Option<&str>) -> String {
    because.map(|b| format!(": {b}")).unwrap_or_default()
}

/// ` (only web and src/cli may: why)`: who may use what an item of a top-level
/// list forbids, then its reason; nothing without either.
pub fn who_may_and_why(who: &[String], because: Option<&str>) -> String {
    let who: Vec<&str> = who.iter().map(String::as_str).collect();
    let only = (!who.is_empty()).then(|| format!("only {} may", crate::check::in_words(&who)));
    let said: Vec<&str> = only.as_deref().into_iter().chain(because).collect();
    if said.is_empty() {
        String::new()
    } else {
        format!(" ({})", said.join(": "))
    }
}

/// The line of a named surface: its paths and who it is open to.
pub fn surface_line(surface: &SurfaceView) -> String {
    format!(
        "surface {}: {} (open to {}{})",
        surface.name,
        surface.paths.join(", "),
        surface.importers.join(", "),
        reason(surface.because.as_deref())
    )
}

/// Each grant in text: `core (may_depend_on: why; only through …)`.
pub fn grant_items(grants: &[Toward]) -> Vec<String> {
    grants
        .iter()
        .map(|Toward { to, decision }| {
            let grant = decision.grant.map(Grant::as_str).unwrap_or_default();
            let through = through(decision.provenance.as_ref());
            let because = reason(decision.because.as_deref());
            let conditions: String = decision
                .conditions
                .iter()
                .map(|c| format!("; {c}"))
                .collect();
            format!("{to} ({grant}{through}{because}{conditions})")
        })
        .collect()
}

/// Each prohibition in text: `http (not even through others)`.
pub fn prohibition_items(prohibitions: &[Toward]) -> Vec<String> {
    prohibitions
        .iter()
        .map(|Toward { to, decision }| {
            let through = through(decision.provenance.as_ref());
            if decision.rejected_by == Some(ClauseId::MustNotReach) {
                format!("{to} (not even through others{through})")
            } else if through.is_empty() {
                to.clone()
            } else {
                format!("{to} ({})", through.trim_start())
            }
        })
        .collect()
}

/// The items of a list, or `nothing`.
pub fn listed_or_nothing(items: &[String]) -> String {
    if items.is_empty() {
        "nothing".to_string()
    } else {
        items.join(", ")
    }
}

fn strings(items: Vec<&str>) -> Vec<String> {
    items.into_iter().map(str::to_string).collect()
}

/// The symbol rules as they bind one file: what `explain` gives for it.
#[derive(Debug, Default, Serialize)]
pub struct FileSymbols {
    /// The names the file may not use: the items of the top-level `forbid_symbol`
    /// that let neither its component nor its path in, then those of the own
    /// `forbid_symbol` of its component. A name in both lists is given once.
    pub forbidden: Vec<ForbiddenSymbol>,
    /// The names of the top-level `forbid_symbol` it may use all the same.
    pub allowed: Vec<AllowedSymbol>,
    /// The names each component the file's component may depend on opens to it,
    /// for those that have `public_symbols`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub public: Vec<PublicSymbols>,
}

/// A name a file may not use.
#[derive(Debug, Serialize)]
pub struct ForbiddenSymbol {
    /// The name, as the item writes it.
    pub name: String,
    /// Who may use it, the components and then the paths, as the top-level item writes them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// The reason a violation gives: the item's, else that of the rule that lists it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes the item.
    pub provenance: Provenance,
}

/// A name of the top-level `forbid_symbol` that a file may use all the same.
#[derive(Debug, Serialize)]
pub struct AllowedSymbol {
    /// The name, as the item writes it.
    pub name: String,
    /// The component of the file that `allow` lets in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<String>,
    /// The `allow_in` pattern that matches the file, the first one the item writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_in: Option<String>,
    /// The reason the item gives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes what lets the file in: the item of `allow` (with the
    /// glob or set it comes through) or the `allow_in` pattern.
    pub provenance: Provenance,
}

/// The names a component opens to the others through `public_symbols`.
#[derive(Debug, Serialize)]
pub struct PublicSymbols {
    /// The component that defines the names.
    pub component: String,
    /// The only names of it other components may use.
    pub names: Vec<String>,
    /// The reason of the component, which a violation gives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes its `public_symbols`.
    pub provenance: Provenance,
}

impl FileSymbols {
    /// Whether no symbol rule binds the file.
    pub fn is_empty(&self) -> bool {
        self.forbidden.is_empty() && self.allowed.is_empty() && self.public.is_empty()
    }

    /// How the symbol rules bind `rel`, whose component may depend on those of
    /// `grants`, decided as the scan judges a use (`symbols::judge`): the top-level
    /// list before the component's own, `allow` before `allow_in`, and the
    /// component's own `forbid_symbol` over an `allow_in` pattern. Nothing binds a
    /// file whose uses the scan does not read.
    pub fn of(policy: &Policy, map: &ComponentMap, rel: &str, grants: &[Toward]) -> Self {
        let mut found = Self::default();
        if decision::unread(map, rel).is_some() {
            return found;
        }
        let config = &policy.config;
        let key = Forbid::Symbol.key();
        let owner = map.component_of(rel);
        let mut seen = std::collections::BTreeSet::new();
        for binding in bindings(config, map, owner, rel, |_| true) {
            let listed = binding.listed;
            let name = listed.name().to_string();
            let because = binding.because.map(str::to_string);
            let site = match (binding.item, binding.granted) {
                (Item::Top(item), Some(Granted::Allow(component))) => {
                    found.allowed.push(AllowedSymbol {
                        name,
                        allow: Some(component.to_string()),
                        allow_in: None,
                        because,
                        provenance: policy.provenance(
                            &Site::of_top_allow(config, key, item, component).unwrap_or_else(
                                || Site::in_key(key, format!("{key}[{item}].allow")),
                            ),
                        ),
                    });
                    continue;
                }
                (Item::Top(item), Some(Granted::AllowIn { at, pattern })) => {
                    // The component's own list still forbids it: it is given below.
                    if owner.is_none_or(|component| {
                        own_symbol_spelling(config, component, &name).is_none()
                    }) {
                        found.allowed.push(AllowedSymbol {
                            name,
                            allow: None,
                            allow_in: Some(pattern.to_string()),
                            because,
                            provenance: policy.provenance(&Site::in_key(
                                key,
                                format!("{key}[{item}].allow_in[{at}]"),
                            )),
                        });
                    }
                    continue;
                }
                // Only a top-level item lets anyone in.
                (Item::Own(_), Some(_)) => continue,
                (Item::Top(item), None) => Site::in_key(key, format!("{key}[{item}]")),
                (Item::Own(i), None) => {
                    let component = owner.unwrap_or_default();
                    Site::of_rule(config, component, &format!(".{key}[{i}]"))
                }
            };
            // A name in both lists is given once, as a violation is.
            if seen.insert(listed.name()) {
                found.forbidden.push(ForbiddenSymbol {
                    name,
                    only: listed.who_may(),
                    because,
                    provenance: policy.provenance(&site),
                });
            }
        }
        found.public = grants
            .iter()
            .filter_map(|Toward { to, .. }| {
                let component = &config.components[to];
                Some(PublicSymbols {
                    component: to.clone(),
                    names: component.public_symbols.clone()?,
                    because: component.because.clone(),
                    provenance: policy.provenance(&Site::of_entry(
                        "components",
                        to,
                        format!("components.{to}.public_symbols"),
                    )),
                })
            })
            .collect();
        found
    }

    /// Its lines in text.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        // Names that say the same, who may use them and why, share a line.
        for alike in self
            .forbidden
            .chunk_by(|a, b| (&a.only, &a.because) == (&b.only, &b.because))
        {
            let names: Vec<&str> = alike.iter().map(|f| f.name.as_str()).collect();
            let first = &alike[0];
            lines.push(format!(
                "may not use: {}{}",
                names.join(", "),
                who_may_and_why(&first.only, first.because.as_deref())
            ));
        }
        for allowed in &self.allowed {
            let granted = match (&allowed.allow, &allowed.allow_in) {
                (Some(component), _) => format!("allow `{component}`"),
                (None, Some(pattern)) => format!("allow_in `{pattern}`"),
                (None, None) => String::new(),
            };
            lines.push(format!(
                "may use, although forbid_symbol forbids it: {} ({granted}{}{})",
                allowed.name,
                through(Some(&allowed.provenance)),
                reason(allowed.because.as_deref())
            ));
        }
        for public in &self.public {
            let because = public
                .because
                .as_deref()
                .map(|b| format!(" ({b})"))
                .unwrap_or_default();
            lines.push(format!(
                "public symbols of {}: {}{because}",
                public.component,
                listed_or_nothing(&public.names)
            ));
        }
        lines
    }
}

/// The policy as loaded.
#[derive(Debug, Serialize)]
pub struct ResolvedPolicy {
    /// The configuration, then each file it extends, then the diagram.
    pub inputs: Vec<Input>,
    pub components: Vec<ComponentEntry>,
    pub common: Vec<CommonItem>,
    /// From the top layer to the bottom one.
    pub layers: Vec<LayerRow>,
    pub forbid_symbol: Vec<ForbiddenItem>,
    pub forbid_external: Vec<ForbiddenItem>,
    pub examples: Vec<ExampleEntry>,
    pub no_cycles: Cycles,
    pub strict: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unowned: Vec<String>,
    /// Whether `tests: auto` puts the test files into the component `tests`.
    pub tests: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entry: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub target_only: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub accepted_gaps: Vec<ResolvedAccepted>,
    pub reach: &'static str,
    pub manifests: bool,
    pub packages: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warn: Vec<String>,
}

/// A policy input: a file whose content is part of the policy.
#[derive(Debug, Serialize)]
pub struct Input {
    /// Relative to the root of the policy, or absolute outside it.
    pub path: String,
    /// `config`, `extended` or `diagram`.
    pub kind: InputKind,
}

/// A component, with its rule decided against every other component.
#[derive(Debug, Serialize)]
pub struct ComponentEntry {
    pub name: String,
    #[serde(rename = "in")]
    pub patterns: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
    /// The public surface, when the component restricts it: empty when it
    /// declares only named surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_symbols: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<SurfaceView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_importers: Option<Vec<String>>,
    pub isolated: bool,
    pub optional: bool,
    pub priority: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy defines the component.
    pub provenance: Provenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleEntry>,
    /// The components it may depend on, with the grant of each.
    pub grants: Vec<Toward>,
    /// The components a prohibition keeps it off.
    pub prohibitions: Vec<Toward>,
}

/// The rule of a component, with the names it must not use.
#[derive(Debug, Serialize)]
pub struct RuleEntry {
    #[serde(flatten)]
    pub view: RuleView,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbid_symbol: Vec<Named>,
}

/// A component of `common`.
#[derive(Debug, Serialize)]
pub struct CommonItem {
    pub name: String,
    pub provenance: Provenance,
}

/// A layer: one component, or sibling components.
#[derive(Debug, Serialize)]
pub struct LayerRow {
    pub components: Vec<String>,
    /// The layers above may not reach past it.
    pub closed: bool,
    pub provenance: Provenance,
}

/// An item of the top-level `forbid_symbol` or `forbid_external`.
#[derive(Debug, Serialize)]
pub struct ForbiddenItem {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// The components that may use it all the same.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// The paths whose files may use it all the same.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_in: Vec<String>,
    pub provenance: Provenance,
}

/// A policy example.
#[derive(Debug, Serialize)]
pub struct ExampleEntry {
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defined_in: Option<String>,
    pub expect: crate::config::Expect,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<crate::config::ExampleRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub provenance: Provenance,
    /// The example as reports name it ([`crate::examples::label`]).
    #[serde(skip)]
    pub label: String,
}

/// What `no_cycles` covers: every component, or those it lists.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Cycles {
    All(bool),
    Among(Vec<String>),
}

/// An entry of `accepted_gaps`: the kind of gap it accepts, what it names and why.
#[derive(Debug, Serialize)]
pub struct ResolvedAccepted {
    pub kind: &'static str,
    /// The globs of the files, for `uncovered`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub because: String,
}

impl ResolvedPolicy {
    /// The policy as `policy` loaded it, decided with the components of `map`.
    pub fn of(policy: &Policy, map: &ComponentMap) -> Self {
        // Every key, so that a new one is printed or left out on purpose.
        let Config {
            // Read by the loader, and expanded into every list.
            version: _,
            sets: _,
            expansion: _,
            // How the files are read and where the debt is kept, not what the policy allows.
            indexers: _,
            baseline: _,
            // A policy input: `inputs` lists the file, as a `diagram`.
            diagram: _,
            components,
            common,
            rules: _,
            forbid_symbol,
            forbid_external,
            exclude,
            target_only,
            no_cycles,
            reach,
            layers,
            strict,
            unowned,
            accepted_gaps,
            tests,
            entry,
            manifests,
            packages,
            warn,
            examples,
        } = &policy.config;
        let config = &policy.config;
        let forbidden_items = |key: &str, items: &[ForbiddenName]| -> Vec<ForbiddenItem> {
            items
                .iter()
                .enumerate()
                .map(|(i, item)| ForbiddenItem {
                    name: item.name().to_string(),
                    because: item.because().map(str::to_string),
                    allow: item.allow().map(<[String]>::to_vec).unwrap_or_default(),
                    allow_in: item.allow_in().map(<[String]>::to_vec).unwrap_or_default(),
                    provenance: policy.provenance(&Site::in_key(key, format!("{key}[{i}]"))),
                })
                .collect()
        };
        Self {
            inputs: policy
                .inputs
                .iter()
                .map(|input| Input {
                    path: policy.input_name(input),
                    kind: input.kind,
                })
                .collect(),
            components: components
                .iter()
                .map(|(name, component)| component_entry(policy, map, name, component))
                .collect(),
            common: common
                .iter()
                .map(|name| CommonItem {
                    name: name.clone(),
                    provenance: policy.provenance(
                        &Site::of_common(config, name).unwrap_or_else(|| Site::of_key("common")),
                    ),
                })
                .collect(),
            layers: layers
                .iter()
                .map(|layer| {
                    let names = layer.names();
                    LayerRow {
                        components: strings(names.clone()),
                        closed: layer.closed(),
                        provenance: policy.provenance(&Site::of_layer(config, &names)),
                    }
                })
                .collect(),
            forbid_symbol: forbidden_items("forbid_symbol", forbid_symbol),
            forbid_external: forbidden_items("forbid_external", forbid_external),
            examples: examples
                .iter()
                .enumerate()
                .map(|(i, example)| ExampleEntry {
                    from: example.from.clone(),
                    to: example.to.clone(),
                    external: example.external.clone(),
                    symbol: example.symbol.clone(),
                    defined_in: example.defined_in.clone(),
                    expect: example.expect,
                    rule: example.rule,
                    because: example.because.clone(),
                    provenance: policy.example_provenance(i),
                    label: crate::examples::label(example),
                })
                .collect(),
            no_cycles: match no_cycles {
                NoCycles::Off => Cycles::All(false),
                NoCycles::All(on) => Cycles::All(*on),
                NoCycles::Among(list) => Cycles::Among(list.clone()),
            },
            strict: *strict,
            unowned: unowned.clone(),
            tests: tests.is_some(),
            entry: entry.clone(),
            exclude: exclude.clone(),
            target_only: target_only.clone(),
            accepted_gaps: accepted_gaps.iter().map(accepted).collect(),
            reach: reach.as_str(),
            manifests: *manifests,
            packages: *packages,
            warn: warn.clone(),
        }
    }

    /// The policy in text, a line for each item.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut inputs = self.inputs.iter();
        if let Some(config) = inputs.next() {
            out.push_str(&format!("policy: {}\n", config.path));
        }
        for input in inputs {
            out.push_str(&format!("  {}: {}\n", input.kind.as_str(), input.path));
        }
        out.push_str("components:\n");
        for component in &self.components {
            // `tests: auto` writes the patterns and the priority of `tests`, not the policy.
            let made = self.tests && component.name == TESTS;
            component_text(component, made, &mut out);
        }
        if !self.common.is_empty() {
            let names: Vec<&str> = self.common.iter().map(|c| c.name.as_str()).collect();
            out.push_str(&format!("common: {}\n", names.join(", ")));
        }
        if !self.layers.is_empty() {
            let rows: Vec<String> = self
                .layers
                .iter()
                .map(|row| {
                    let names = row.components.join(" | ");
                    if row.closed {
                        format!("{names} (closed)")
                    } else {
                        names
                    }
                })
                .collect();
            out.push_str(&format!("layers: {}\n", rows.join(" > ")));
        }
        for (key, items) in [
            ("forbid_symbol", &self.forbid_symbol),
            ("forbid_external", &self.forbid_external),
        ] {
            for item in items {
                let who: Vec<String> = item.allow.iter().chain(&item.allow_in).cloned().collect();
                out.push_str(&format!(
                    "{key}: {}{}\n",
                    item.name,
                    who_may_and_why(&who, item.because.as_deref())
                ));
            }
        }
        if !self.examples.is_empty() {
            out.push_str("examples:\n");
            for example in &self.examples {
                let rule = example
                    .rule
                    .map(|r| format!(" [{}]", r.id().as_str()))
                    .unwrap_or_default();
                let input = example
                    .provenance
                    .input
                    .as_deref()
                    .map(|i| format!(" ({i})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  {}: {}{rule}{}{input}\n",
                    example.label,
                    example.expect.as_str(),
                    reason(example.because.as_deref())
                ));
            }
        }
        match &self.no_cycles {
            Cycles::All(false) => {}
            Cycles::All(true) => out.push_str("no_cycles: every component\n"),
            Cycles::Among(list) => out.push_str(&format!("no_cycles: {}\n", list.join(", "))),
        }
        let list = |items: &[String]| items.join(", ");
        let settings = [
            (self.strict, "strict: true".to_string()),
            (
                !self.unowned.is_empty(),
                format!("unowned: {}", list(&self.unowned)),
            ),
            (self.tests, "tests: auto".to_string()),
            (
                !self.entry.is_empty(),
                format!("entry: {}", list(&self.entry)),
            ),
            (
                !self.exclude.is_empty(),
                format!("exclude: {}", list(&self.exclude)),
            ),
            (
                !self.target_only.is_empty(),
                format!("target_only: {}", list(&self.target_only)),
            ),
            (
                self.reach != Reach::default().as_str(),
                format!("reach: {}", self.reach),
            ),
            (self.manifests, "manifests: true".to_string()),
            (self.packages, "packages: true".to_string()),
            (!self.warn.is_empty(), format!("warn: {}", list(&self.warn))),
        ];
        for (on, line) in settings {
            if on {
                out.push_str(&format!("{line}\n"));
            }
        }
        for accepted in &self.accepted_gaps {
            out.push_str(&format!(
                "accepted_gaps: {} {}{}\n",
                accepted.kind,
                accepted.files.join(", "),
                reason(Some(&accepted.because))
            ));
        }
        out
    }
}

fn accepted(entry: &AcceptedEntry) -> ResolvedAccepted {
    ResolvedAccepted {
        kind: entry.kind(),
        files: strings(entry.uncovered_patterns()),
        because: entry.because().to_string(),
    }
}

/// The entry of the component `name`.
fn component_entry(
    policy: &Policy,
    map: &ComponentMap,
    name: &str,
    component: &Component,
) -> ComponentEntry {
    let config = &policy.config;
    // Every key, so that a new one is printed or left out on purpose.
    let Component {
        patterns,
        public: _,
        because,
        except,
        priority,
        isolated,
        public_symbols,
        allow_importers,
        surfaces: _,
        optional,
        owner,
        defines,
        part,
    } = component;
    let Dependencies {
        grants,
        prohibitions,
        // The complement of the two, which `explain` gives for the component of one file.
        not_allowed: _,
    } = Dependencies::of(policy, map, name);
    ComponentEntry {
        name: name.to_string(),
        patterns: strings(patterns.as_vec()),
        except: except
            .as_ref()
            .map(|e| strings(e.as_vec()))
            .unwrap_or_default(),
        public: component.public_surface().map(strings),
        public_symbols: public_symbols.clone(),
        surfaces: surfaces(policy, name),
        allow_importers: allow_importers.clone(),
        isolated: *isolated,
        optional: *optional,
        priority: *priority,
        owner: owner.clone(),
        defines: defines.clone(),
        part: part.clone(),
        because: because.clone(),
        provenance: policy.provenance(&component_site(config, name)),
        rule: RuleView::of(policy, name).map(|view| RuleEntry {
            view,
            forbid_symbol: forbidden(policy, name, Forbid::Symbol),
        }),
        grants,
        prohibitions,
    }
}

/// Where the policy defines the component `name`: `tests: auto` makes `tests`.
fn component_site(config: &Config, name: &str) -> Site {
    if name == TESTS && config.tests.is_some() {
        Site::of_key("tests")
    } else {
        Site::of_entry("components", name, format!("components.{name}"))
    }
}

/// The lines of one component.
fn component_text(component: &ComponentEntry, made: bool, out: &mut String) {
    let except = if component.except.is_empty() || made {
        String::new()
    } else {
        format!(", except {}", component.except.join(", "))
    };
    let flags: Vec<&str> = [
        (component.optional && !made, "optional"),
        (component.isolated, "isolated"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    let flags = if flags.is_empty() {
        String::new()
    } else {
        format!(" ({})", flags.join(", "))
    };
    let patterns = if made {
        "the test files of every language (tests: auto)".to_string()
    } else {
        component.patterns.join(", ")
    };
    out.push_str(&format!(
        "  {}: {patterns}{except}{flags}\n",
        component.name
    ));
    let mut lines = Vec::new();
    if let Some(owner) = &component.owner {
        lines.push(format!("owner: {owner}"));
    }
    if component.priority != 0 && !made {
        lines.push(format!("priority: {}", component.priority));
    }
    if let Some(part) = &component.part {
        lines.push(format!("part: {part}"));
    }
    match &component.public {
        Some(public) if public.is_empty() => {
            lines.push("public surface: none (only the named surfaces are open)".to_string());
        }
        Some(public) => lines.push(format!("public surface: {}", public.join(", "))),
        None => {}
    }
    if let Some(symbols) = &component.public_symbols {
        lines.push(format!("public symbols: {}", symbols.join(", ")));
    }
    lines.extend(component.surfaces.iter().map(surface_line));
    if let Some(importers) = &component.allow_importers {
        lines.push(format!("open only to: {}", importers.join(", ")));
    }
    if !component.defines.is_empty() {
        lines.push(format!("defines: {}", component.defines.join(", ")));
    }
    if let Some(because) = &component.because {
        lines.push(format!("restricted because: {because}"));
    }
    lines.push(format!(
        "may depend on: {}",
        listed_or_nothing(&grant_items(&component.grants))
    ));
    if !component.prohibitions.is_empty() {
        lines.push(format!(
            "forbidden: {}",
            prohibition_items(&component.prohibitions).join(", ")
        ));
    }
    if let Some(rule) = &component.rule {
        if !rule.forbid_symbol.is_empty() {
            let names: Vec<&str> = rule.forbid_symbol.iter().map(|n| n.name.as_str()).collect();
            lines.push(format!("forbidden names: {}", names.join(", ")));
        }
        // The reason of the rule last, as `explain` gives it.
        lines.extend(rule.view.lines());
    }
    for line in lines {
        out.push_str(&format!("    {line}\n"));
    }
}
