//! `easement review`: what a change to the policy permits, forbids and
//! accepts, told over the current code. It compares the policy at the
//! merge-base with the one in the working tree; it never judges the code at
//! the merge-base, and it gates nothing.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::check::{self, Key, Violation};
use crate::components::{Access, ComponentMap};
use crate::config::{Config, Example};
use crate::decision::{self, Grant, PairDecision, Refusal};
use crate::examples;
use crate::policy::Policy;
use crate::report::{self, AcceptedGap, EvidenceGap};

mod constraints;

pub use constraints::{ConstraintChange, Direction, changes as constraints};

/// What the review says of a change to the policy.
#[derive(Debug, Serialize)]
pub struct Review {
    /// The revision the change is compared with, as it was asked.
    pub base: String,
    /// The commit where HEAD left the base: its policy is the old one.
    pub merge_base: String,
    pub inputs: Inputs,
    /// The keys of the policy whose change alters what is scanned, when the
    /// current code was scanned a second time under the policy at the
    /// merge-base; null when one scan served both.
    pub rescanned: Option<Vec<&'static str>>,
    pub components: Components,
    /// Every ordered pair of components whose dependency decision changed.
    pub permissions: Vec<PermissionChange>,
    /// Every change to a surface or to the importers a component admits.
    pub surfaces: Vec<SurfaceChange>,
    /// Every change to a constraint (GLOSSARY.md), each loosened or tightened.
    pub constraints: Vec<ConstraintChange>,
    /// The policies whose diagram could not be read: its arrows are not
    /// compared, and the delta leaves the `diagram` rule out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread_diagrams: Vec<UnreadDiagram>,
    pub delta: Delta,
    pub ownership: Ownership,
    pub examples: Examples,
    pub debt: Debt,
    /// Code the scans could not read or judge: the delta and the file lists
    /// say nothing about it.
    pub evidence_gaps: Vec<EvidenceGap>,
    /// Files no indexer read that the head policy accepts in `accepted_gaps`,
    /// by entry: unread too, but by decision.
    pub accepted_gaps: Vec<AcceptedGap>,
    /// Set when indexes came from the cache: what their key leaves out and
    /// how to index again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<String>,
}

/// One of the two policies a review compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The policy at the merge-base.
    Base,
    /// The policy in the working tree.
    Head,
}

/// A policy whose diagram could not be read: its arrows are not compared, and
/// the delta leaves the `diagram` rule out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnreadDiagram {
    pub policy: Side,
    pub reason: String,
}

/// The policy inputs of both policies, relative to the root where under it.
#[derive(Debug, Default, Serialize)]
pub struct Inputs {
    pub base: Vec<String>,
    pub head: Vec<String>,
    /// The inputs whose content differs, or that one policy lacks.
    pub changed: Vec<String>,
    /// Inputs outside the repository: their content at the merge-base is
    /// unknown, so the review cannot see how they changed.
    pub past_unknown: Vec<String>,
}

/// Components one policy has and the other lacks. A rename is one of each.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Components {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl Components {
    /// The components the head policy adds and the ones it drops.
    pub fn of(sides: &Sides) -> Self {
        let names = |c: &Config| c.components.keys().cloned().collect::<Vec<_>>();
        let (added, removed) =
            added_and_removed(&names(&sides.base.config), &names(&sides.head.config));
        Self { added, removed }
    }
}

/// Files of the current code whose owner differs between the two policies.
#[derive(Debug, Default, Serialize)]
pub struct Ownership {
    /// Files whose component changed, to or from none.
    pub moved: Vec<Move>,
    /// Files the head policy excludes and the base policy did not.
    pub excluded: Vec<String>,
    /// Files the base policy excluded and the head policy does not.
    pub included: Vec<String>,
}

/// A file whose component differs between the two policies; none is no component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Move {
    pub file: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl Ownership {
    /// The ownership changes over `files`, the source files either scan found.
    pub fn of(sides: &Sides, files: &BTreeSet<&str>) -> Self {
        let mut ownership = Self::default();
        for &file in files {
            match (
                sides.base_map.is_excluded(file),
                sides.head_map.is_excluded(file),
            ) {
                (false, true) => ownership.excluded.push(file.to_string()),
                (true, false) => ownership.included.push(file.to_string()),
                (true, true) => {}
                (false, false) => {
                    let from = sides.base_map.component_of(file);
                    let to = sides.head_map.component_of(file);
                    if from != to {
                        ownership.moved.push(Move {
                            file: file.to_string(),
                            from: from.map(str::to_string),
                            to: to.map(str::to_string),
                        });
                    }
                }
            }
        }
        ownership
    }
}

/// The two policies being compared, each with its component map.
pub struct Sides<'a> {
    pub base: &'a Policy,
    pub head: &'a Policy,
    pub base_map: &'a ComponentMap,
    pub head_map: &'a ComponentMap,
}

impl<'a> Sides<'a> {
    /// The components both policies have, in the order of the head policy.
    fn kept(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.head
            .config
            .components
            .keys()
            .filter(|c| self.base.config.components.contains_key(*c))
            .map(String::as_str)
    }
}

/// One ordered pair of components whose dependency decision changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PermissionChange {
    pub from: String,
    pub to: String,
    pub change: Change,
    /// The grant that now permits the pair, or the clause that now rejects it.
    pub clause: String,
    /// The globs and sets, as the head policy writes them, that bring that grant
    /// or clause to the pair: a new component a glob covers is permitted through it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub through: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
}

/// Which way the decision on a pair flipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Permitted,
    Rejected,
}

/// The pairs of components whose decision differs between the two policies.
/// A pair with a component the base policy lacks counts as rejected there, so
/// a new component is listed with what it permits; a pair with a component
/// the head policy lacks is left out, since the component is gone.
pub fn permissions(sides: &Sides) -> Vec<PermissionChange> {
    let (base, head) = (&sides.base.config, &sides.head.config);
    let mut changes = Vec::new();
    for from in head.components.keys() {
        for to in head.components.keys().filter(|to| *to != from) {
            let now = decision::pair(head, sides.head_map, from, to);
            let then_permitted = base.components.contains_key(from)
                && base.components.contains_key(to)
                && !decision::pair(base, sides.base_map, from, to).rejected();
            if now.rejected() != then_permitted {
                continue;
            }
            let (site, change, clause, because) = match &now.grant {
                Ok(grant) if now.importers.is_none() => {
                    // A plain `may_depend_on` item gives the reason of its rule;
                    // the rule of the source says nothing of `common` or `layers`.
                    let because = match grant {
                        Grant::MayDependOn => now.because.or_else(|| head.reason(from)),
                        _ => now.because,
                    };
                    let site = decision::grant_site(head, from, to, *grant);
                    (site, Change::Permitted, grant.as_str().to_string(), because)
                }
                grant => {
                    let site = match grant {
                        Err(refusal) => decision::refusal_site(head, from, to, refusal),
                        Ok(_) => None,
                    };
                    let (clause, because) = rejection(head, &now, to);
                    (site, Change::Rejected, clause, because)
                }
            };
            changes.push(PermissionChange {
                from: from.clone(),
                to: to.clone(),
                change,
                clause,
                through: site.map(|s| s.through().to_vec()).unwrap_or_default(),
                because: because.map(str::to_string),
            });
        }
    }
    changes
}

/// The clause that rejects a pair, as `why` words it, and its reason.
fn rejection<'a>(
    config: &'a Config,
    verdict: &PairDecision<'a>,
    to: &str,
) -> (String, Option<&'a str>) {
    match &verdict.grant {
        Err(Refusal::Prohibited { reach: true }) => ("must_not_reach".to_string(), verdict.because),
        Err(Refusal::Prohibited { reach: false }) => {
            ("must_not_depend_on".to_string(), verdict.because)
        }
        Err(Refusal::NotGranted { layers: Some(why) }) => {
            (format!("not granted: {why}"), verdict.because)
        }
        Err(Refusal::NotGranted { layers: None }) => ("not granted".to_string(), verdict.because),
        Ok(_) => (
            match verdict.importers.unwrap_or_default() {
                [] => format!("allow_importers of {to} lists no component"),
                listed => format!("allow_importers of {to} lists only {}", listed.join(", ")),
            },
            config.components[to].because.as_deref(),
        ),
    }
}

/// A change to the `public` patterns or the `allow_importers` of a component
/// both policies have. Patterns are compared as written: no claim is made
/// about the paths two globs share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceChange {
    pub component: String,
    pub key: SurfaceKey,
    /// The value at the merge-base; null when unset: every file is public, or
    /// every component the rules allow may import it.
    pub before: Option<Vec<String>>,
    pub after: Option<Vec<String>>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Files of the component, as the head policy owns them, that the change
    /// opens: to every other component for `public`, to the importers added
    /// for `allow_importers`, to some component its surfaces kept out before
    /// for `surfaces`.
    pub opens: Vec<String>,
    /// Files of the component that the change closes: to every other
    /// component for `public`, to the importers removed for `allow_importers`,
    /// to some component its surfaces let in before for `surfaces`.
    pub closes: Vec<String>,
    /// For `allow_importers`, the glob or set, as written, that each importer of
    /// `added` or `removed` comes through: the head policy's for one added, the
    /// merge-base's for one removed. A new component a glob covers is let in so.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub through: BTreeMap<String, String>,
}

/// The key of a component that says who may import which of its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKey {
    Public,
    AllowImporters,
    Surfaces,
}

impl SurfaceKey {
    fn as_str(self) -> &'static str {
        match self {
            SurfaceKey::Public => "public",
            SurfaceKey::AllowImporters => "allow_importers",
            SurfaceKey::Surfaces => "surfaces",
        }
    }

    /// What the key means when it is not set.
    fn unset(self) -> &'static str {
        match self {
            SurfaceKey::Public => "unset: every file public",
            SurfaceKey::AllowImporters => "unset: any component the rules allow",
            SurfaceKey::Surfaces => "unset: no named surface",
        }
    }
}

/// The surface changes of every component both policies have, with the files
/// among `files` that a change to `public` opens or closes.
pub fn surfaces(sides: &Sides, files: &[String]) -> Vec<SurfaceChange> {
    let owned = |v: Option<Vec<&str>>| v.map(|v| v.into_iter().map(str::to_string).collect());
    let mut changes = Vec::new();
    for name in sides.kept() {
        let (then, now) = (
            &sides.base.config.components[name],
            &sides.head.config.components[name],
        );
        // The files of the component, as the head policy owns them, each with
        // whether its surface had it then and has it now.
        let surface: Vec<(&String, bool, bool)> = files
            .iter()
            .filter(|f| sides.head_map.component_of(f) == Some(name))
            .map(|f| {
                (
                    f,
                    sides.base_map.is_public(name, f),
                    sides.head_map.is_public(name, f),
                )
            })
            .collect();
        let change = |key, before: Option<Vec<String>>, after: Option<Vec<String>>| {
            (before != after).then(|| {
                let (added, removed) = added_and_removed(
                    before.as_deref().unwrap_or_default(),
                    after.as_deref().unwrap_or_default(),
                );
                SurfaceChange {
                    component: name.to_string(),
                    key,
                    before,
                    after,
                    added,
                    removed,
                    opens: Vec::new(),
                    closes: Vec::new(),
                    through: BTreeMap::new(),
                }
            })
        };
        let pick = |keep: fn(bool, bool) -> bool| -> Vec<String> {
            surface
                .iter()
                .filter(|(_, then, now)| keep(*then, *now))
                .map(|(f, _, _)| (*f).clone())
                .collect()
        };
        // A change to `public` as written, told as the public surface it
        // leaves: empty, not unset, beside named surfaces.
        if let Some(mut public) = (then.public.as_ref().map(|p| p.as_vec())
            != now.public.as_ref().map(|p| p.as_vec()))
        .then(|| {
            change(
                SurfaceKey::Public,
                owned(then.public_surface()),
                owned(now.public_surface()),
            )
        })
        .flatten()
        {
            public.opens = pick(|then, now| !then && now);
            public.closes = pick(|then, now| then && !now);
            changes.push(public);
        }
        // Who is let in, not how the list is spelled: a glob expands in the order of
        // the names, so the same importers in another order are no change.
        if let Some(mut importers) = (importers_of(then) != importers_of(now))
            .then(|| {
                change(
                    SurfaceKey::AllowImporters,
                    then.allow_importers.clone(),
                    now.allow_importers.clone(),
                )
            })
            .flatten()
        {
            // Importers admitted now reach the surface of today; importers
            // turned away lose the surface they had. No list admits every
            // component: a first list only turns some away, and dropping the
            // list only lets them back in.
            let both = importers.before.is_some() && importers.after.is_some();
            if importers.after.is_none() || (both && !importers.added.is_empty()) {
                importers.opens = pick(|_, now| now);
            }
            if importers.before.is_none() || (both && !importers.removed.is_empty()) {
                importers.closes = pick(|then, _| then);
            }
            let list = format!("components.{name}.allow_importers");
            for (config, items) in [
                (&sides.head.config, &importers.added),
                (&sides.base.config, &importers.removed),
            ] {
                for item in items {
                    if let Some(through) =
                        config.written(&list, item).and_then(|w| w.through.clone())
                    {
                        importers.through.insert(item.clone(), through);
                    }
                }
            }
            changes.push(importers);
        }
        if let Some(mut named) = change(
            SurfaceKey::Surfaces,
            named_surfaces(then),
            named_surfaces(now),
        ) {
            // Who may reach each file, as the surfaces say, then and now.
            let (mut opens, mut closes) = (Vec::new(), Vec::new());
            for file in surface.iter().map(|(f, _, _)| *f) {
                let then = sides.base_map.access(name, file);
                let now = sides.head_map.access(name, file);
                if wider(&now, &then) {
                    opens.push(file.clone());
                }
                if wider(&then, &now) {
                    closes.push(file.clone());
                }
            }
            (named.opens, named.closes) = (opens, closes);
            changes.push(named);
        }
    }
    changes
}

/// The importers `allow_importers` lets in, in no order; none when it is unset.
fn importers_of(component: &crate::config::Component) -> Option<BTreeSet<&str>> {
    component
        .allow_importers
        .as_ref()
        .map(|list| list.iter().map(String::as_str).collect())
}

/// The named surfaces of a component as a review lists them, one per name, its
/// importers in name order: `admin: src/billing/admin (open to jobs)`; none when
/// it has none.
fn named_surfaces(component: &crate::config::Component) -> Option<Vec<String>> {
    (!component.surfaces.is_empty()).then(|| {
        component
            .surfaces
            .iter()
            .map(|(name, s)| {
                // Who is let in, in name order: not how the list is spelled.
                let importers: BTreeSet<&str> = s.importers.iter().map(String::as_str).collect();
                format!(
                    "{name}: {} (open to {})",
                    s.patterns.as_vec().join(", "),
                    importers.into_iter().collect::<Vec<_>>().join(", ")
                )
            })
            .collect()
    })
}

/// Whether `this` lets in a component that `other` keeps out.
fn wider(this: &Access, other: &Access) -> bool {
    match (this, other) {
        (_, Access::Everyone) => false,
        (Access::Everyone, Access::Only(_)) => true,
        (Access::Only(this), Access::Only(other)) => !this.is_subset(other),
    }
}

/// The items of `after` missing from `before`, and those of `before` missing
/// from `after`, each in its own order.
fn added_and_removed(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
    (
        missing(after, before, String::eq),
        missing(before, after, String::eq),
    )
}

/// The items of `from` that `other` has none the same as.
fn missing<T: Clone>(from: &[T], other: &[T], same: impl Fn(&T, &T) -> bool) -> Vec<T> {
    from.iter()
        .filter(|item| !other.iter().any(|o| same(item, o)))
        .cloned()
        .collect()
}

/// The policy delta: the violations that fail under one policy and not the
/// other, over the same current code. The baseline at the merge-base is left
/// out; what the change legalizes in the baseline is in [`Debt`].
#[derive(Debug, Default, Serialize)]
pub struct Delta {
    /// Violations the head policy fails and the base policy did not, that
    /// `check` fails.
    pub introduced: Vec<Violation>,
    /// Violations the head policy fails and the base policy did not, that the
    /// baseline of the working tree legalizes: `check` does not fail them.
    pub legalized: Vec<Violation>,
    /// Violations the base policy failed and the head policy does not report.
    pub removed: Vec<Violation>,
    /// Violations the base policy failed that the head policy only warns
    /// about, its rule now in `warn`.
    pub warned: Vec<Violation>,
}

impl Delta {
    /// The delta between the violations each policy reports over the code;
    /// `baseline` holds the entries of the working tree's baseline. With
    /// `diagram_unread`, one policy could not read its diagram, so its rule
    /// is left out: what the other says of it would be told as a change.
    pub fn of(
        sides: &Sides,
        base_violations: Vec<Violation>,
        head_violations: Vec<Violation>,
        baseline: &BTreeSet<Key>,
        diagram_unread: bool,
    ) -> Self {
        let (base, head) = (&sides.base.config, &sides.head.config);
        let unjudged = |v: &Violation| diagram_unread && v.rule == check::RuleId::Diagram;
        let judged = |violations: Vec<Violation>| {
            let mut violations = check::dedupe(violations);
            violations.retain(|v| !unjudged(v));
            violations
        };
        let base_violations = judged(base_violations);
        let head_violations = judged(head_violations);
        let failing = |config: &Config, violations: &[Violation]| -> BTreeSet<Key> {
            violations
                .iter()
                .filter(|v| !config.warns(v.rule))
                .map(Violation::key)
                .collect()
        };
        let (then, now) = (
            failing(base, &base_violations),
            failing(head, &head_violations),
        );
        let reported: BTreeSet<Key> = head_violations.iter().map(Violation::key).collect();
        let mut delta = Self::default();
        for v in head_violations {
            let key = v.key();
            if head.warns(v.rule) || then.contains(&key) {
                continue;
            }
            // As `check` judges it, the baseline never accepts a rule about the config.
            if !v.rule.is_config_rule() && baseline.contains(&key) {
                delta.legalized.push(v);
            } else {
                delta.introduced.push(v);
            }
        }
        for v in base_violations {
            let key = v.key();
            if base.warns(v.rule) || now.contains(&key) {
                continue;
            }
            if reported.contains(&key) {
                delta.warned.push(v);
            } else {
                delta.removed.push(v);
            }
        }
        for list in [
            &mut delta.introduced,
            &mut delta.legalized,
            &mut delta.removed,
            &mut delta.warned,
        ] {
            list.sort_by(|a, b| (&a.file, a.line, a.rule).cmp(&(&b.file, b.line, b.rule)));
        }
        delta
    }
}

/// The policy examples the change adds, removes or changes. Removing one
/// breaks nothing, yet the policy no longer pins that decision.
#[derive(Debug, Default, Serialize)]
pub struct Examples {
    pub added: Vec<Example>,
    pub removed: Vec<Example>,
    /// Examples of the same dependency that now expect another decision.
    pub changed: Vec<ExampleChange>,
}

/// An example of one dependency, before and after the change.
#[derive(Debug, Serialize)]
pub struct ExampleChange {
    pub before: Example,
    pub after: Example,
}

impl Examples {
    /// What differs between the examples of the two policies: added in the
    /// order the head policy writes them, removed and changed in the order of
    /// the base policy.
    pub fn of(sides: &Sides) -> Self {
        let (base, head) = (&sides.base.config.examples, &sides.head.config.examples);
        let mut added = missing(head, base, same_expectation);
        let mut changed = Vec::new();
        // A dependency with one example on each side whose expectation moved
        // is one change; with several (`extends` joins the lists), which
        // became which is unknown, so they stay added and removed.
        let named_once = |list: &[Example], example: &Example| {
            list.iter().filter(|e| same_subject(e, example)).count() == 1
        };
        let removed = missing(base, head, same_expectation)
            .into_iter()
            .filter_map(|before| {
                let after = (named_once(base, &before) && named_once(head, &before))
                    .then(|| added.iter().position(|a| same_subject(a, &before)))
                    .flatten();
                match after {
                    Some(i) => {
                        changed.push(ExampleChange {
                            before,
                            after: added.remove(i),
                        });
                        None
                    }
                    None => Some(before),
                }
            })
            .collect();
        Self {
            added,
            removed,
            changed,
        }
    }

    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Whether both are examples of the same proposed dependency, or of the same
/// use of a name.
fn same_subject(a: &Example, b: &Example) -> bool {
    (&a.from, &a.to, &a.external, &a.symbol, &a.defined_in)
        == (&b.from, &b.to, &b.external, &b.symbol, &b.defined_in)
}

/// Whether both expect the same decision of the same dependency; the reason
/// they give does not count.
fn same_expectation(a: &Example, b: &Example) -> bool {
    same_subject(a, b) && (a.expect, a.rule) == (b.expect, b.rule)
}

/// The example and what it expects: `web/a.ts -> db/b.ts: expects rejected`.
fn example_line(example: &Example) -> String {
    format!(
        "{}: expects {}",
        examples::label(example),
        examples::expected(example)
    )
}

/// The example as the text lists it, what it expected before when it
/// changed, and its reason on a line of its own.
fn example_lines(example: &Example, before: Option<&Example>) -> String {
    let mut out = format!("    {}", example_line(example));
    if let Some(before) = before {
        out.push_str(&format!(" (was {})", examples::expected(before)));
    }
    out.push('\n');
    if let Some(because) = &example.because {
        out.push_str(&format!("      because: {because}\n"));
    }
    out
}

/// What the change accepts that no rule judges any more, and what it takes back.
#[derive(Debug, Default, Serialize)]
pub struct Debt {
    /// Violations the baseline legalizes now and did not before.
    pub baseline_added: Vec<Key>,
    /// Violations the baseline legalized before and does not now.
    pub baseline_removed: Vec<Key>,
    /// What entries of `accepted_gaps` accept now and did not before: evidence
    /// gaps that no longer count against the verdict, each glob of files apart.
    pub accepted_gaps_added: Vec<Accepted>,
    /// What they accepted before and do not now.
    pub accepted_gaps_removed: Vec<Accepted>,
    /// `exclude` globs added: paths no rule judges.
    pub exclude_added: Vec<String>,
    pub exclude_removed: Vec<String>,
    /// `target_only` globs added: files never judged as the source of a
    /// dependency.
    pub target_only_added: Vec<String>,
    pub target_only_removed: Vec<String>,
    /// Rules moved to `warn`, whose violations fail no more.
    pub warn_added: Vec<String>,
    /// Rules taken out of `warn`, whose violations fail again.
    pub warn_removed: Vec<String>,
}

/// One thing an entry of `accepted_gaps` accepts, with the reason the entry gives:
/// for `uncovered`, one glob of its files; for `unresolved_imports`, the imports of
/// one file written as one specifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Accepted {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specifier: Option<String>,
    pub because: String,
}

impl Debt {
    /// What differs between the two policies and their baseline entries.
    pub fn of(sides: &Sides, base_baseline: &BTreeSet<Key>, head_baseline: &BTreeSet<Key>) -> Self {
        let (base, head) = (&sides.base.config, &sides.head.config);
        let accepted = |config: &Config| -> Vec<Accepted> {
            config
                .accepted_gaps
                .iter()
                .flat_map(|entry| {
                    let accepted = |files: Option<&str>, import: Option<(&str, &str)>| Accepted {
                        kind: entry.kind(),
                        files: files.map(str::to_string),
                        file: import.map(|(file, _)| file.to_string()),
                        specifier: import.map(|(_, specifier)| specifier.to_string()),
                        because: entry.because().to_string(),
                    };
                    entry
                        .uncovered_patterns()
                        .into_iter()
                        .map(|pattern| accepted(Some(pattern), None))
                        .chain(entry.import().map(|import| accepted(None, Some(import))))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let (accepted_then, accepted_now) = (accepted(base), accepted(head));
        // A new reason is a new decision, so it counts too.
        let same = |a: &Accepted, b: &Accepted| a == b;
        let (exclude_added, exclude_removed) = added_and_removed(&base.exclude, &head.exclude);
        let (target_only_added, target_only_removed) =
            added_and_removed(&base.target_only, &head.target_only);
        let (warn_added, warn_removed) = added_and_removed(&base.warn, &head.warn);
        Self {
            baseline_added: head_baseline.difference(base_baseline).cloned().collect(),
            baseline_removed: base_baseline.difference(head_baseline).cloned().collect(),
            accepted_gaps_added: missing(&accepted_now, &accepted_then, same),
            accepted_gaps_removed: missing(&accepted_then, &accepted_now, same),
            exclude_added,
            exclude_removed,
            target_only_added,
            target_only_removed,
            warn_added,
            warn_removed,
        }
    }
}

/// Each item as the policy writes it, in backticks.
fn quoted_each(items: &[String]) -> Vec<String> {
    items.iter().map(|i| format!("`{i}`")).collect()
}

impl Accepted {
    /// What it accepts in backticks, with its kind and reason.
    fn line(&self) -> String {
        let what = match (&self.files, &self.file, &self.specifier) {
            (Some(files), _, _) => format!("`{files}`"),
            (None, Some(file), Some(specifier)) => format!("`{specifier}` in {file}"),
            _ => String::new(),
        };
        format!("{} {what} (because: {})", self.kind, self.because)
    }
}

/// Items as the policy writes them, each in backticks; `none` for no item.
fn quoted(items: &[String]) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    quoted_each(items).join(", ")
}

impl SurfaceChange {
    /// What changed, in one line, as a reader of the policy says it.
    fn summary(&self) -> String {
        let unset = self.key.unset();
        match (&self.before, &self.after) {
            (None, Some(after)) => format!("set to {} (was {unset})", quoted(after)),
            (Some(before), None) => format!("{unset} now (was {})", quoted(before)),
            _ => {
                let mut parts = Vec::new();
                // Each item, with the glob or set it comes through.
                let items = |items: &[String]| {
                    items
                        .iter()
                        .map(|item| match self.through.get(item) {
                            Some(through) => format!("`{item}` (through `{through}`)"),
                            None => format!("`{item}`"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                if !self.added.is_empty() {
                    parts.push(format!("added {}", items(&self.added)));
                }
                if !self.removed.is_empty() {
                    parts.push(format!("removed {}", items(&self.removed)));
                }
                if parts.is_empty() {
                    parts.push("reordered".to_string());
                }
                parts.join("; ")
            }
        }
    }
}

impl Review {
    /// The review as a reader takes it in: every section, each saying when
    /// nothing changed, then what it does not cover.
    pub fn text(&self) -> String {
        let mut out = format!(
            "Reviewing the change to the policy since {} (merge-base {}), over the current code\n",
            self.base,
            &self.merge_base[..self.merge_base.len().min(12)]
        );
        if self.inputs.changed.is_empty() {
            out.push_str("No policy input changed\n");
        } else {
            out.push_str(&format!(
                "Policy inputs changed: {}\n",
                self.inputs.changed.join(", ")
            ));
        }
        for input in &self.inputs.past_unknown {
            out.push_str(&format!(
                "Not seen: {input} is outside the repository, so its content at the merge-base is unknown and how it changed is not reviewed\n"
            ));
        }
        for unread in &self.unread_diagrams {
            let policy = match unread.policy {
                Side::Base => "at the merge-base",
                Side::Head => "in the working tree",
            };
            out.push_str(&format!(
                "Not compared: the diagram of the policy {policy} cannot be read ({}), so its arrows are not reviewed and the delta leaves out the `diagram` rule; make it readable and review again to compare them\n",
                unread.reason
            ));
        }
        if let Some(keys) = &self.rescanned {
            out.push_str(&format!(
                "Scanned twice: the change alters what is scanned ({}), so the current code was scanned again under the policy at the merge-base\n",
                quoted(&keys.iter().map(|k| k.to_string()).collect::<Vec<_>>())
            ));
        }
        if !self.components.added.is_empty() || !self.components.removed.is_empty() {
            out.push('\n');
        }
        if !self.components.added.is_empty() {
            out.push_str(&format!(
                "Components added: {}\n",
                self.components.added.join(", ")
            ));
        }
        if !self.components.removed.is_empty() {
            out.push_str(&format!(
                "Components removed: {}\n",
                self.components.removed.join(", ")
            ));
        }
        out.push_str("\nPermissions:\n");
        if self.permissions.is_empty() {
            out.push_str("  no pair of components changed\n");
        }
        for p in &self.permissions {
            let change = match p.change {
                Change::Permitted => "newly permitted",
                Change::Rejected => "newly rejected",
            };
            out.push_str(&format!(
                "  {} -> {} {change} ({}{})\n",
                p.from,
                p.to,
                p.clause,
                crate::resolved::through_names(&p.through)
            ));
            if let Some(because) = &p.because {
                out.push_str(&format!("    because: {because}\n"));
            }
        }
        out.push_str("\nSurfaces:\n");
        if self.surfaces.is_empty() {
            out.push_str("  no surface or importer list changed\n");
        }
        for s in &self.surfaces {
            out.push_str(&format!(
                "  {}.{}: {}\n",
                s.component,
                s.key.as_str(),
                s.summary()
            ));
            for (verb, files) in [("opens", &s.opens), ("closes", &s.closes)] {
                if !files.is_empty() {
                    out.push_str(&format!("    {verb} {} file(s):\n", files.len()));
                    out.push_str(&report::item_lines(files, "      "));
                }
            }
        }
        out.push_str("\nConstraints:\n");
        if self.constraints.is_empty() {
            out.push_str("  no constraint changed\n");
        }
        for c in &self.constraints {
            out.push_str(&format!("  {}\n", c.line()));
            if let Some(because) = &c.because {
                out.push_str(&format!("    because: {because}\n"));
            }
        }
        out.push_str("\nPolicy delta on the current code:\n");
        let delta = &self.delta;
        if delta.introduced.is_empty()
            && delta.legalized.is_empty()
            && delta.removed.is_empty()
            && delta.warned.is_empty()
        {
            out.push_str("  no violation introduced or removed\n");
        }
        for (heading, list) in [
            ("introduced", &delta.introduced),
            (
                "introduced but legalized in the baseline, so `check` does not fail them",
                &delta.legalized,
            ),
            ("removed", &delta.removed),
            ("now warnings, their rule in `warn`", &delta.warned),
        ] {
            if list.is_empty() {
                continue;
            }
            out.push_str(&format!("  {heading} ({}):\n", list.len()));
            for v in list {
                out.push_str(&format!(
                    "    {}:{}: {} [{}]\n",
                    v.file,
                    v.line,
                    v.message,
                    v.rule.as_str()
                ));
                if let Some(because) = &v.because {
                    out.push_str(&format!("      because: {because}\n"));
                }
            }
        }
        out.push_str("\nOwnership:\n");
        let ownership = &self.ownership;
        if ownership.moved.is_empty()
            && ownership.excluded.is_empty()
            && ownership.included.is_empty()
        {
            out.push_str("  no file changed component\n");
        }
        let mut moves: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
        fn name(c: &Option<String>) -> &str {
            c.as_deref().unwrap_or("no component")
        }
        for m in &ownership.moved {
            moves
                .entry((name(&m.from), name(&m.to)))
                .or_default()
                .push(m.file.clone());
        }
        for ((from, to), files) in &moves {
            out.push_str(&format!("  {from} -> {to} ({}):\n", files.len()));
            out.push_str(&report::item_lines(files, "    "));
        }
        for (heading, files) in [
            ("newly excluded", &ownership.excluded),
            ("no longer excluded", &ownership.included),
        ] {
            if !files.is_empty() {
                out.push_str(&format!("  {heading} ({}):\n", files.len()));
                out.push_str(&report::item_lines(files, "    "));
            }
        }
        out.push_str("\nPolicy examples:\n");
        let examples = &self.examples;
        if examples.is_empty() {
            out.push_str("  no example added, removed or changed\n");
        }
        for (heading, entries) in [
            (format!("added ({})", examples.added.len()), &examples.added),
            (
                format!(
                    "removed ({}), so the policy no longer pins their decision",
                    examples.removed.len()
                ),
                &examples.removed,
            ),
        ] {
            if entries.is_empty() {
                continue;
            }
            out.push_str(&format!("  {heading}:\n"));
            for example in entries {
                out.push_str(&example_lines(example, None));
            }
        }
        if !examples.changed.is_empty() {
            out.push_str(&format!("  changed ({}):\n", examples.changed.len()));
            for change in &examples.changed {
                out.push_str(&example_lines(&change.after, Some(&change.before)));
            }
        }
        out.push_str("\nDebt and trust:\n");
        let debt = &self.debt;
        let mut said = false;
        for (heading, keys) in [
            ("Baseline entries added", &debt.baseline_added),
            ("Baseline entries removed", &debt.baseline_removed),
        ] {
            if keys.is_empty() {
                continue;
            }
            said = true;
            out.push_str(&format!("  {heading} ({}):\n", keys.len()));
            for key in keys {
                out.push_str(&format!(
                    "    {} [{}]\n",
                    report::key_description(key),
                    key.rule.as_str()
                ));
            }
        }
        for (heading, items) in [
            (
                "accepted_gaps added",
                debt.accepted_gaps_added
                    .iter()
                    .map(Accepted::line)
                    .collect(),
            ),
            (
                "accepted_gaps removed",
                debt.accepted_gaps_removed
                    .iter()
                    .map(Accepted::line)
                    .collect(),
            ),
            ("exclude added", quoted_each(&debt.exclude_added)),
            ("exclude removed", quoted_each(&debt.exclude_removed)),
            ("target_only added", quoted_each(&debt.target_only_added)),
            (
                "target_only removed",
                quoted_each(&debt.target_only_removed),
            ),
            (
                "Moved to warn (their violations fail no more)",
                quoted_each(&debt.warn_added),
            ),
            (
                "Taken out of warn (their violations fail again)",
                quoted_each(&debt.warn_removed),
            ),
        ] {
            if items.is_empty() {
                continue;
            }
            said = true;
            out.push_str(&format!("  {heading}: {}\n", items.join(", ")));
        }
        if !said {
            out.push_str("  nothing accepted or taken back\n");
        }
        let gaps = report::gap_lines(
            &self.evidence_gaps,
            &self.accepted_gaps,
            "the delta and the file lists say nothing about this code",
            false,
            false,
        );
        if !gaps.is_empty() {
            out.push('\n');
            out.push_str(&gaps);
        }
        if let Some(note) = &self.cached {
            out.push_str(&format!("\n{note}\n"));
        }
        let loosened = self
            .constraints
            .iter()
            .filter(|c| c.change == Direction::Loosened)
            .count();
        out.push_str(&format!(
            "\neasement review: {} permission(s), {} surface(s), {} constraint(s) loosened, {} tightened, {} introduced, {} removed, {} file(s) moved, {} baseline entr(ies) added; it informs, `easement check` gates\n",
            self.permissions.len(),
            self.surfaces.len(),
            loosened,
            self.constraints.len() - loosened,
            self.delta.introduced.len(),
            self.delta.removed.len(),
            self.ownership.moved.len(),
            self.debt.baseline_added.len(),
        ));
        out
    }

    /// The review as data, for `--format json`.
    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a review is JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy written as `yaml`, with the other files it names, each a
    /// path and its content, in a directory of its own.
    pub(super) fn policy(yaml: &str, files: &[(&str, &str)]) -> Policy {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            std::fs::write(dir.path().join(path), content).unwrap();
        }
        let path = dir.path().join("easement.yml");
        std::fs::write(&path, yaml).unwrap();
        Policy::load(&path, None).unwrap()
    }

    /// Runs `each` on the two policies and their maps.
    pub(super) fn compare<T>(base: &str, head: &str, each: impl FnOnce(&Sides) -> T) -> T {
        compare_with(&[], base, head, each)
    }

    /// As [`compare`], with files both policies may name.
    pub(super) fn compare_with<T>(
        files: &[(&str, &str)],
        base: &str,
        head: &str,
        each: impl FnOnce(&Sides) -> T,
    ) -> T {
        let (base, head) = (policy(base, files), policy(head, files));
        let (base_map, head_map) = (
            ComponentMap::new(&base.config).unwrap(),
            ComponentMap::new(&head.config).unwrap(),
        );
        each(&Sides {
            base: &base,
            head: &head,
            base_map: &base_map,
            head_map: &head_map,
        })
    }

    const TWO: &str = "version: 1
components:
  web: { in: web }
  db: { in: db }
";

    fn pairs(changes: &[PermissionChange]) -> Vec<(String, Change, String)> {
        changes
            .iter()
            .map(|p| {
                (
                    format!("{} -> {}", p.from, p.to),
                    p.change,
                    p.clause.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_new_common_component_is_permitted_to_everyone_and_a_removed_one_is_only_named() {
        let head = TWO.replace("  db: { in: db }\n", "  log: { in: log }\ncommon: [log]\n");
        let (permissions, components) =
            compare(TWO, &head, |s| (permissions(s), Components::of(s)));
        assert_eq!(
            pairs(&permissions),
            [(
                "web -> log".to_string(),
                Change::Permitted,
                "common".to_string()
            )]
        );
        assert_eq!(
            components,
            Components {
                added: vec!["log".to_string()],
                removed: vec!["db".to_string()],
            }
        );
    }

    #[test]
    fn an_importer_list_that_leaves_a_permitted_component_out_rejects_it() {
        let base = format!("{TWO}common: [db]\n");
        let head = base.replace(
            "db: { in: db }",
            "db: { in: db, allow_importers: [], because: db is reached through jobs }",
        );
        compare(&base, &head, |s| {
            let permissions = permissions(s);
            assert_eq!(
                pairs(&permissions),
                [(
                    "web -> db".to_string(),
                    Change::Rejected,
                    "allow_importers of db lists no component".to_string()
                )]
            );
            assert_eq!(
                permissions[0].because.as_deref(),
                Some("db is reached through jobs")
            );
            // An empty list admits no one, unlike no list: both are told apart.
            let files = ["db/pool.ts", "web/page.ts"].map(str::to_string);
            let surfaces = surfaces(s, &files);
            assert_eq!(surfaces.len(), 1);
            assert_eq!(surfaces[0].before, None);
            assert_eq!(surfaces[0].after, Some(Vec::new()));
            assert!(surfaces[0].opens.is_empty());
            assert_eq!(surfaces[0].closes, ["db/pool.ts"]);
            assert_eq!(
                surfaces[0].summary(),
                "set to none (was unset: any component the rules allow)"
            );
        });
    }

    /// No list admits every component: a first list only turns some away, and
    /// dropping the list only lets them back in.
    #[test]
    fn an_importer_list_set_or_dropped_moves_the_surface_one_way() {
        let unset = format!("{TWO}common: [db]\n");
        let listed = unset.replace("db: { in: db }", "db: { in: db, allow_importers: [web] }");
        let files = ["db/pool.ts", "web/page.ts"].map(str::to_string);
        let set = compare(&unset, &listed, |s| surfaces(s, &files));
        assert_eq!(set.len(), 1);
        assert!(set[0].opens.is_empty(), "{:?}", set[0].opens);
        assert_eq!(set[0].closes, ["db/pool.ts"]);
        let dropped = compare(&listed, &unset, |s| surfaces(s, &files));
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].opens, ["db/pool.ts"]);
        assert!(dropped[0].closes.is_empty(), "{:?}", dropped[0].closes);
    }

    #[test]
    fn a_surface_set_for_the_first_time_closes_the_files_it_leaves_out() {
        let head = TWO.replace("db: { in: db }", "db: { in: db, public: db/api.ts }");
        let files = ["db/api.ts", "db/pool.ts", "web/page.ts"].map(str::to_string);
        let changes = compare(TWO, &head, |s| surfaces(s, &files));
        assert_eq!(changes.len(), 1);
        let public = &changes[0];
        assert_eq!(public.before, None);
        assert_eq!(public.added, ["db/api.ts"]);
        assert!(public.opens.is_empty());
        assert_eq!(public.closes, ["db/pool.ts"]);
        assert_eq!(
            public.summary(),
            "set to `db/api.ts` (was unset: every file public)"
        );
    }

    /// A component with only named surfaces has an empty public surface, so
    /// setting `public` adds to it rather than closing every other file.
    #[test]
    fn public_set_beside_named_surfaces_widens_an_empty_public_surface() {
        let files = ["db/api.ts", "db/admin/x.ts", "db/pool.ts"].map(str::to_string);
        let base = TWO.replace(
            "db: { in: db }",
            "db: { in: db, surfaces: { admin: { in: db/admin, importers: [web] } } }",
        );
        let head = base.replace("in: db,", "in: db, public: db/api.ts,");
        let changes = compare(&base, &head, |s| surfaces(s, &files));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before, Some(Vec::new()));
        assert_eq!(changes[0].summary(), "added `db/api.ts`");
        assert_eq!(changes[0].opens, ["db/api.ts"]);
        assert!(changes[0].closes.is_empty());
    }

    fn lines(examples: &[Example]) -> Vec<String> {
        examples.iter().map(example_line).collect()
    }

    #[test]
    fn an_example_that_flips_its_decision_is_one_change() {
        let base =
            format!("{TWO}examples:\n  - {{ from: web/a.ts, to: db/b.ts, expect: allowed }}\n");
        let head = base.replace(
            "expect: allowed }",
            "expect: rejected, because: db is private }",
        );
        let examples = compare(&base, &head, Examples::of);
        assert!(
            examples.added.is_empty() && examples.removed.is_empty(),
            "{examples:?}"
        );
        let [change] = examples.changed.as_slice() else {
            panic!("one change: {examples:?}");
        };
        assert_eq!(
            example_lines(&change.after, Some(&change.before)),
            "    web/a.ts -> db/b.ts: expects rejected (was allowed)\n      because: db is private\n"
        );
    }

    /// Examples of the uses of one name from one file are told apart by the file
    /// that defines it, and from an example of a dependency of that file.
    #[test]
    fn an_example_of_a_name_is_told_by_its_name_and_definition() {
        let base = format!(
            "{TWO}examples:\n  - {{ from: web/a.ts, to: db/b.ts, expect: rejected }}\n  - {{ from: web/a.ts, symbol: db/b.Pool, defined_in: db/b.ts, expect: rejected, rule: symbol }}\n"
        );
        let head = format!(
            "{base}  - {{ from: web/a.ts, symbol: db/b.Pool, expect: allowed }}\n  - {{ from: web/a.ts, symbol: db/b.Conn, expect: rejected }}\n"
        );
        let examples = compare(&base, &head, Examples::of);
        assert!(
            examples.removed.is_empty() && examples.changed.is_empty(),
            "{examples:?}"
        );
        assert_eq!(
            lines(&examples.added),
            [
                "web/a.ts uses db/b.Pool: expects allowed",
                "web/a.ts uses db/b.Conn: expects rejected"
            ]
        );
        let head = base.replace("expect: rejected, rule: symbol", "expect: allowed");
        let examples = compare(&base, &head, Examples::of);
        let [change] = examples.changed.as_slice() else {
            panic!("one change: {examples:?}");
        };
        assert_eq!(
            example_line(&change.after),
            "web/a.ts uses db/b.Pool defined in db/b.ts: expects allowed"
        );
    }

    #[test]
    fn an_example_whose_reason_alone_changed_is_not_a_change() {
        let base = format!(
            "{TWO}examples:\n  - {{ from: web/a.ts, to: db/b.ts, expect: rejected, because: old }}\n"
        );
        let head = base.replace("because: old", "because: new");
        let examples = compare(&base, &head, Examples::of);
        assert!(examples.is_empty(), "{examples:?}");
    }

    /// `extends` joins the lists, so one dependency can have two examples; which
    /// of them became the new one is unknown, so none is called changed.
    #[test]
    fn examples_of_a_dependency_named_twice_stay_added_and_removed() {
        let twice = format!(
            "{TWO}examples:\n  - {{ from: web/a.ts, to: db/b.ts, expect: rejected }}\n  - {{ from: web/a.ts, to: db/b.ts, expect: rejected, rule: privacy }}\n"
        );
        let once = twice.replace(
            "  - { from: web/a.ts, to: db/b.ts, expect: rejected, rule: privacy }\n",
            "",
        );
        let examples = compare(&twice, &once, Examples::of);
        assert!(examples.changed.is_empty(), "{examples:?}");
        assert!(examples.added.is_empty(), "{examples:?}");
        assert_eq!(
            lines(&examples.removed),
            ["web/a.ts -> db/b.ts: expects rejected [privacy]"]
        );

        let moved = twice.replace("rule: privacy", "rule: dependency");
        let examples = compare(&twice, &moved, Examples::of);
        assert!(examples.changed.is_empty(), "{examples:?}");
        assert_eq!(
            lines(&examples.added),
            ["web/a.ts -> db/b.ts: expects rejected [dependency]"]
        );
        assert_eq!(
            lines(&examples.removed),
            ["web/a.ts -> db/b.ts: expects rejected [privacy]"]
        );
    }

    /// A new reason is a new decision: the entry is listed as taken back with the
    /// old one and added with the new one.
    #[test]
    fn an_accepted_gap_with_a_new_reason_is_listed_both_ways() {
        let entry = |because: &str| {
            format!(
                "{TWO}accepted_gaps:\n  - {{ kind: uncovered, files: old/**, because: {because} }}\n"
            )
        };
        let debt = compare(&entry("rewritten soon"), &entry("kept for good"), |s| {
            Debt::of(s, &BTreeSet::new(), &BTreeSet::new())
        });
        let because = |items: &[Accepted]| -> Vec<String> {
            items.iter().map(|a| a.because.clone()).collect()
        };
        assert_eq!(because(&debt.accepted_gaps_added), ["kept for good"]);
        assert_eq!(because(&debt.accepted_gaps_removed), ["rewritten soon"]);
    }

    #[test]
    fn exclude_accepted_gaps_and_warn_entries_are_listed_both_ways() {
        let base = format!(
            "{TWO}exclude: [gen]\nwarn: [unused]\naccepted_gaps:\n  - {{ kind: uncovered, files: old/**, because: rewritten next quarter }}\n  - {{ kind: uncovered, files: tools/**, because: build helpers }}\n  - {{ kind: unresolved_imports, file: src/i18n.ts, specifier: './locales/*.json', because: by language }}\n"
        );
        let head = format!(
            "{TWO}exclude: [vendor]\nwarn: [dependency]\naccepted_gaps:\n  - {{ kind: uncovered, files: [legacy/**, old/**], because: rewritten next quarter }}\n"
        );
        let debt = compare(&base, &head, |s| {
            Debt::of(s, &BTreeSet::new(), &BTreeSet::new())
        });
        assert_eq!(debt.exclude_added, ["vendor"]);
        assert_eq!(debt.exclude_removed, ["gen"]);
        assert_eq!(debt.warn_added, ["dependency"]);
        assert_eq!(debt.warn_removed, ["unused"]);
        // Each glob apart: `old/**` was accepted already.
        assert_eq!(
            debt.accepted_gaps_added,
            [Accepted {
                kind: "uncovered",
                files: Some("legacy/**".to_string()),
                file: None,
                specifier: None,
                because: "rewritten next quarter".to_string(),
            }]
        );
        assert_eq!(
            debt.accepted_gaps_removed,
            [
                Accepted {
                    kind: "uncovered",
                    files: Some("tools/**".to_string()),
                    file: None,
                    specifier: None,
                    because: "build helpers".to_string(),
                },
                Accepted {
                    kind: "unresolved_imports",
                    files: None,
                    file: Some("src/i18n.ts".to_string()),
                    specifier: Some("./locales/*.json".to_string()),
                    because: "by language".to_string(),
                },
            ]
        );
    }
}
