//! The constraints of the policy (GLOSSARY.md) that a change to it loosens or
//! tightens: every clause, beyond the decision on a pair of components, that
//! limits what the code may use, must use or must contain.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use super::Sides;
use crate::config::{Component, Config, Forbid, ForbiddenName, Listed, NoCycles, Rule};
use crate::diagram;
use crate::policy::{Policy, Provenance, Site};

/// Which way a change to a constraint goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// It permits what was refused, or stops requiring what was required.
    Loosened,
    Tightened,
}

/// What happened to an item of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Edit {
    Added,
    Removed,
}

impl Direction {
    /// The way a constraint goes that one side has and the other lacks:
    /// gaining it tightens, losing it loosens.
    fn gained(now: bool) -> Self {
        if now {
            Direction::Tightened
        } else {
            Direction::Loosened
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Direction::Loosened => "loosened",
            Direction::Tightened => "tightened",
        }
    }

    fn opposite(self) -> Self {
        match self {
            Direction::Loosened => Direction::Tightened,
            Direction::Tightened => Direction::Loosened,
        }
    }
}

/// One change to a constraint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConstraintChange {
    /// The component whose rule or entry holds it; null at the top level.
    pub component: Option<String>,
    pub key: &'static str,
    /// The name, module, glob, component or arrow added or removed, or the
    /// forbidden name that `allow` now lets `components` use or stops them
    /// using, or `allow_in` the files in `paths`; null for a value that changed
    /// as a whole.
    pub item: Option<String>,
    /// Whether the item was added or removed; absent for a value that changed
    /// as a whole, and for an `allow` or `allow_in` that changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<Edit>,
    /// The value at the merge-base, for a value that changed as a whole (null
    /// when unset).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<serde_json::Value>,
    /// For an `allow` that changed: the components that may now use `item`
    /// (loosened), or may no longer (tightened).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// For an `allow_in` that changed: the patterns whose files may now use
    /// `item` (loosened), or may no longer (tightened).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    pub change: Direction,
    /// What a violation of the constraint shows: the item's own reason, or
    /// else the reason of its rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes it: the head policy for what it adds or now
    /// holds, the merge-base policy for what it drops.
    pub provenance: Provenance,
}

/// Every change to a constraint, at the top level first, then for each
/// component both policies have, in the order of the head policy. Items are
/// told apart by name, so a list only reordered, or an item whose reason alone
/// changed, is no change; a component one policy lacks is in `components`.
pub fn changes(sides: &Sides) -> Vec<ConstraintChange> {
    let mut diff = Diff {
        sides,
        changes: Vec::new(),
    };
    diff.top_level();
    for name in sides.kept() {
        diff.rule(name);
        diff.component(name);
    }
    diff.changes
}

/// Where a constraint is written: at the top level, in the rule of a
/// component, or in its entry under `components`.
#[derive(Clone, Copy)]
enum At<'a> {
    Top,
    Rule(&'a str),
    Component(&'a str),
}

impl At<'_> {
    fn component(self) -> Option<String> {
        match self {
            At::Top => None,
            At::Rule(name) | At::Component(name) => Some(name.to_string()),
        }
    }

    /// The site of the item of the list of components `key` that names `name`,
    /// when `key` is such a list.
    fn listed_site(self, config: &Config, key: &str, name: &str) -> Option<Site> {
        match self {
            At::Top => Site::of_top_item(config, key, name),
            At::Rule(rule) => Site::of_rule_item(config, rule, key, name),
            At::Component(_) => None,
        }
    }

    /// The site of `key`, or of its item at `index`, in `config`.
    fn site(self, config: &Config, key: &str, index: Option<usize>) -> Site {
        let item = index.map(|i| format!("[{i}]")).unwrap_or_default();
        match self {
            At::Top => Site::in_key(key, format!("{key}{item}")),
            At::Rule(name) => Site::of_rule(config, name, &format!(".{key}{item}")),
            At::Component(name) => {
                Site::of_entry("components", name, format!("components.{name}.{key}{item}"))
            }
        }
    }
}

/// The items of a list as the policy writes them, each with its own reason,
/// and the reason a violation of an item without one shows.
struct Items<'a> {
    names: Vec<(&'a str, Option<&'a str>)>,
    reason: Option<&'a str>,
}

impl<'a> Items<'a> {
    fn listed(list: &'a [Listed], reason: Option<&'a str>) -> Self {
        Self::named(list.iter().map(|l| (l.name(), l.because())), reason)
    }

    /// The items of a `forbid_symbol` or `forbid_external` list; who each
    /// `allow` lets in is compared by [`Diff::allow`].
    fn forbidden(list: &'a [ForbiddenName], reason: Option<&'a str>) -> Self {
        Self::named(list.iter().map(|l| (l.name(), l.because())), reason)
    }

    fn named(
        names: impl Iterator<Item = (&'a str, Option<&'a str>)>,
        reason: Option<&'a str>,
    ) -> Self {
        Self {
            names: names.collect(),
            reason,
        }
    }

    fn plain(list: &'a [String], reason: Option<&'a str>) -> Self {
        Self {
            names: list.iter().map(|name| (name.as_str(), None)).collect(),
            reason,
        }
    }

    /// The list as a value, for a list set or unset as a whole.
    fn value(&self) -> Value {
        Value::from(self.names.iter().map(|(n, _)| *n).collect::<Vec<_>>())
    }

    /// The first item named `name`, with its index.
    fn find(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|(n, _)| *n == name)
    }
}

/// The changes found so far between the two policies.
struct Diff<'s> {
    sides: &'s Sides<'s>,
    changes: Vec<ConstraintChange>,
}

/// One change before it is placed: what it is, and where to look it up.
struct Found<'a> {
    item: Option<String>,
    before: Option<Value>,
    after: Option<Value>,
    change: Direction,
    because: Option<&'a str>,
    /// Whether the head policy writes it, rather than the merge-base one.
    in_head: bool,
    index: Option<usize>,
}

impl Diff<'_> {
    fn push(&mut self, at: At, key: &'static str, found: Found) {
        let policy = if found.in_head {
            self.sides.head
        } else {
            self.sides.base
        };
        // An item with values is one both sides have, changed within: only an
        // `allow` is written so; an item added or removed carries none.
        let whole = found.before.is_some() || found.after.is_some();
        let edit = (found.item.is_some() && !whole).then_some(if found.in_head {
            Edit::Added
        } else {
            Edit::Removed
        });
        // An item of a list of components is written where the policy names it,
        // maybe through a glob or a set; any other at its index.
        let config = &policy.config;
        let site = found
            .item
            .as_deref()
            .and_then(|item| at.listed_site(config, key, item))
            .unwrap_or_else(|| at.site(config, key, found.index));
        self.changes.push(ConstraintChange {
            component: at.component(),
            key,
            item: found.item,
            edit,
            before: found.before,
            after: found.after,
            components: Vec::new(),
            paths: Vec::new(),
            change: found.change,
            because: found.because.map(str::to_string),
            provenance: policy.provenance(&site),
        });
    }

    /// The items one list has and the other lacks: an item added goes
    /// `added`, one removed the other way.
    fn items(&mut self, at: At, key: &'static str, then: &Items, now: &Items, added: Direction) {
        self.items_that(at, key, then, now, added, |_, _| true);
    }

    /// As [`Diff::items`], listing only the items for which `listed`, given
    /// the item and whether it was added or removed, holds.
    fn items_that(
        &mut self,
        at: At,
        key: &'static str,
        then: &Items,
        now: &Items,
        added: Direction,
        listed: impl Fn(&str, Edit) -> bool,
    ) {
        for (side, from, other, change) in [
            (true, now, then, added),
            (false, then, now, added.opposite()),
        ] {
            for (index, (name, own)) in from.names.iter().enumerate() {
                if from.find(name) != Some(index)
                    || other.find(name).is_some()
                    || !listed(name, if side { Edit::Added } else { Edit::Removed })
                {
                    continue;
                }
                self.push(
                    at,
                    key,
                    Found {
                        item: Some(name.to_string()),
                        before: None,
                        after: None,
                        change,
                        because: own.or(from.reason),
                        in_head: side,
                        index: Some(index),
                    },
                );
            }
        }
    }

    /// The components that the `allow` of an item of the top-level list of
    /// `forbid` in the head policy, or of the merge-base item of the same name,
    /// lets use it now and not then (loosened), or then and not now
    /// (tightened): one entry each way. A component is judged on every list
    /// that forbids it, so one let in that its own rule used to forbid is
    /// listed here, and one whose own rule still forbids it is not. Then the
    /// same for the patterns of `allow_in`, told apart as written, of an item
    /// both policies have: a new item is tightened as a whole.
    fn allow(&mut self, forbid: Forbid) {
        let (base, head) = (&self.sides.base.config, &self.sides.head.config);
        let key = forbid.key();
        let kept: Vec<&str> = self.sides.kept().collect();
        for (index, item) in head.top_forbidden(forbid).iter().enumerate() {
            let name = item.name();
            let old = top_level_item(base, forbid, name);
            let (mut let_in, mut shut_out) = (Vec::new(), Vec::new());
            for &component in &kept {
                if !item.allows(component) && !old.is_some_and(|old| old.allows(component)) {
                    continue;
                }
                match (
                    forbids(base, forbid, component, name),
                    forbids(head, forbid, component, name),
                ) {
                    (true, false) => let_in.push(component.to_string()),
                    (false, true) => shut_out.push(component.to_string()),
                    _ => {}
                }
            }
            let field = match item.allow() {
                Some(_) => format!("{key}[{index}].allow"),
                None => format!("{key}[{index}]"),
            };
            for (components, change) in [
                (let_in, Direction::Loosened),
                (shut_out, Direction::Tightened),
            ] {
                if components.is_empty() {
                    continue;
                }
                self.changes.push(ConstraintChange {
                    component: None,
                    key,
                    item: Some(name.to_string()),
                    edit: None,
                    before: None,
                    after: None,
                    components,
                    paths: Vec::new(),
                    change,
                    because: item.because().map(str::to_string),
                    provenance: self
                        .sides
                        .head
                        .provenance(&Site::in_key(key, field.clone())),
                });
            }
            if let Some(old) = old {
                self.allow_in(key, index, item, old);
            }
        }
    }

    /// The patterns that the `allow_in` of the item at `index` of the top-level
    /// list `key` in the head policy has and the merge-base item `old` of the
    /// same name lacks (loosened), and the other way (tightened): one entry
    /// each way. The patterns alone tell: a pattern of `forbid_symbol` matches
    /// the files of no component that forbids the name on its own, or the
    /// policy fails.
    fn allow_in(
        &mut self,
        key: &'static str,
        index: usize,
        item: &ForbiddenName,
        old: &ForbiddenName,
    ) {
        let (now, then) = (
            item.allow_in().unwrap_or_default(),
            old.allow_in().unwrap_or_default(),
        );
        // The patterns of `from` that `other` lacks, once each, in their order.
        let lacking = |from: &[String], other: &[String]| -> Vec<String> {
            let mut lacking: Vec<String> = Vec::new();
            for pattern in from {
                if !other.contains(pattern) && !lacking.contains(pattern) {
                    lacking.push(pattern.clone());
                }
            }
            lacking
        };
        let field = match item.allow_in() {
            Some(_) => format!("{key}[{index}].allow_in"),
            None => format!("{key}[{index}]"),
        };
        for (paths, change) in [
            (lacking(now, then), Direction::Loosened),
            (lacking(then, now), Direction::Tightened),
        ] {
            if paths.is_empty() {
                continue;
            }
            self.changes.push(ConstraintChange {
                component: None,
                key,
                item: Some(item.name().to_string()),
                edit: None,
                before: None,
                after: None,
                components: Vec::new(),
                paths,
                change,
                because: item.because().map(str::to_string),
                provenance: self
                    .sides
                    .head
                    .provenance(&Site::in_key(key, field.clone())),
            });
        }
    }

    /// A list that allows, where unset means no limit: setting it tightens,
    /// unsetting it loosens, and between two lists an item added loosens.
    fn allowed(&mut self, at: At, key: &'static str, then: Option<Items>, now: Option<Items>) {
        match (then, now) {
            (None, None) => {}
            (Some(then), Some(now)) => self.items(at, key, &then, &now, Direction::Loosened),
            (then, now) => {
                let value =
                    |items: &Option<Items>| items.as_ref().map_or(Value::Null, Items::value);
                let reason = now.as_ref().or(then.as_ref()).and_then(|i| i.reason);
                self.push(
                    at,
                    key,
                    Found {
                        item: None,
                        before: Some(value(&then)),
                        after: Some(value(&now)),
                        change: Direction::gained(now.is_some()),
                        because: reason,
                        in_head: now.is_some(),
                        index: None,
                    },
                );
            }
        }
    }

    /// A flag whose turning on goes `on`.
    fn flag(
        &mut self,
        at: At,
        key: &'static str,
        (then, now): (bool, bool),
        on: Direction,
        because: (Option<&str>, Option<&str>),
    ) {
        if then == now {
            return;
        }
        self.push(
            at,
            key,
            Found {
                item: None,
                before: Some(Value::from(then)),
                after: Some(Value::from(now)),
                change: if now { on } else { on.opposite() },
                because: if now { because.1 } else { because.0 },
                in_head: now,
                index: None,
            },
        );
    }

    /// A limit: a higher one or none loosens, a lower one or a new one tightens.
    fn limit<T: PartialOrd + Copy + Into<Value>>(
        &mut self,
        at: At,
        key: &'static str,
        (then, now): (Option<T>, Option<T>),
        because: (Option<&str>, Option<&str>),
    ) {
        let change = match (then, now) {
            (None, Some(_)) | (Some(_), None) => Direction::gained(now.is_some()),
            (Some(a), Some(b)) if b > a => Direction::Loosened,
            (Some(a), Some(b)) if b < a => Direction::Tightened,
            _ => return,
        };
        let value = |v: Option<T>| v.map_or(Value::Null, Into::into);
        self.push(
            at,
            key,
            Found {
                item: None,
                before: Some(value(then)),
                after: Some(value(now)),
                change,
                because: if now.is_some() { because.1 } else { because.0 },
                in_head: now.is_some(),
                index: None,
            },
        );
    }

    fn top_level(&mut self) {
        let (base, head) = (&self.sides.base.config, &self.sides.head.config);
        // Every key, so that a new one cannot be left out.
        let Config {
            // The format, not a clause.
            version: _,
            // Compared per component below; one added or removed is in `components`.
            components: _,
            rules: _,
            // The decision on pairs of components: `permissions`.
            common: _,
            layers: _,
            // How lists of components are spelled: compared expanded, where they name
            // components.
            sets: _,
            expansion: _,
            // Which files are judged as the source of a dependency: the debt section.
            exclude: _,
            target_only: _,
            // The debt section compares its entries, not where they are kept.
            baseline: _,
            uncovered_ok: _,
            warn: _,
            // What is scanned: `rescanned` names them, and the delta shows their effect.
            reach: _,
            indexers: _,
            tests: _,
            packages: _,
            // The policy examples section.
            examples: _,
            forbid_symbol: _,
            forbid_external: _,
            no_cycles,
            strict,
            unowned,
            entry,
            diagram,
            manifests,
        } = head;
        let none = (None, None);
        for forbid in Forbid::ALL {
            self.items(
                At::Top,
                forbid.key(),
                &Items::forbidden(base.top_forbidden(forbid), None),
                &Items::forbidden(head.top_forbidden(forbid), None),
                Direction::Tightened,
            );
            self.allow(forbid);
        }
        self.no_cycles(&base.no_cycles, no_cycles);
        self.flag(
            At::Top,
            "strict",
            (base.strict, *strict),
            Direction::Tightened,
            none,
        );
        self.items(
            At::Top,
            "unowned",
            &Items::plain(&base.unowned, None),
            &Items::plain(unowned, None),
            Direction::Loosened,
        );
        // With an entry the `unused` check runs: it starts unset, as no list.
        let entries = |list| Some(Items::plain(list, None)).filter(|i| !i.names.is_empty());
        self.allowed(At::Top, "entry", entries(&base.entry), entries(entry));
        self.flag(
            At::Top,
            "manifests",
            (base.manifests, *manifests),
            Direction::Tightened,
            none,
        );
        self.diagram(base.diagram.as_ref(), diagram.as_ref());
    }

    /// The components both policies have that enter or leave `no_cycles`.
    fn no_cycles(&mut self, then: &NoCycles, now: &NoCycles) {
        let index = |cycles: &NoCycles, name: &str| match cycles {
            NoCycles::Among(list) => list.iter().position(|c| c == name),
            NoCycles::Off | NoCycles::All(_) => None,
        };
        let kept: Vec<&str> = self.sides.kept().collect();
        for name in kept {
            let (covered, covers) = (then.covers(name), now.covers(name));
            if covered == covers {
                continue;
            }
            self.push(
                At::Top,
                "no_cycles",
                Found {
                    item: Some(name.to_string()),
                    before: None,
                    after: None,
                    change: Direction::gained(covers),
                    because: None,
                    in_head: covers,
                    index: index(if covers { now } else { then }, name),
                },
            );
        }
    }

    /// A diagram set or unset as a whole, or the arrows, as pairs of
    /// components, that one diagram draws and the other does not. An arrow
    /// of a component the head policy drops goes with the component, which
    /// `components` lists; one to a component it adds is listed. A diagram
    /// that cannot be read is not guessed at: the review names it.
    fn diagram(&mut self, then: Option<&PathBuf>, now: Option<&PathBuf>) {
        let path =
            |p: Option<&PathBuf>| p.map_or(Value::Null, |p| Value::from(crate::relpath::slash(p)));
        if then.is_some() != now.is_some() {
            self.push(
                At::Top,
                "diagram",
                Found {
                    item: None,
                    before: Some(path(then)),
                    after: Some(path(now)),
                    change: Direction::gained(now.is_some()),
                    because: None,
                    in_head: now.is_some(),
                    index: None,
                },
            );
            return;
        }
        let (Ok(Some(then)), Ok(Some(now))) = (
            diagram::drawn(self.sides.base),
            diagram::drawn(self.sides.head),
        ) else {
            return;
        };
        for (policy, from, other, change, edit) in [
            (
                self.sides.head,
                &now,
                &then,
                Direction::Loosened,
                Edit::Added,
            ),
            (
                self.sides.base,
                &then,
                &now,
                Direction::Tightened,
                Edit::Removed,
            ),
        ] {
            let components = &self.sides.head.config.components;
            let kept = |(a, b): &&(String, String)| {
                components.contains_key(a) && components.contains_key(b)
            };
            for (a, b) in from.difference(other).filter(kept) {
                self.changes.push(ConstraintChange {
                    component: None,
                    key: "diagram",
                    item: Some(format!("{a} -> {b}")),
                    edit: Some(edit),
                    before: None,
                    after: None,
                    components: Vec::new(),
                    paths: Vec::new(),
                    change,
                    because: None,
                    provenance: drawn_in(policy),
                });
            }
        }
    }

    /// The rule of a component both policies have; none is a rule with
    /// nothing in it.
    fn rule(&mut self, name: &str) {
        let empty = Rule::default();
        let then = self.sides.base.config.rules.get(name).unwrap_or(&empty);
        let now = self.sides.head.config.rules.get(name).unwrap_or(&empty);
        let at = At::Rule(name);
        // Every key, so that a new one cannot be left out.
        let Rule {
            // The decision on pairs of components: `permissions`.
            may_depend_on: _,
            must_not_depend_on: _,
            must_not_reach: _,
            // The reason of each change below.
            because: _,
            forbid_symbol,
            forbid_external,
            may_use_external,
            forbid_file,
            must_depend_on,
            must_use_external,
            requires_file,
            requires_directive,
            allow_implementers,
            max_fan_out,
            max_instability,
        } = now;
        let reasons = (then.because.as_deref(), now.because.as_deref());
        // The lists that forbid or require: an item added tightens. A name the
        // top level forbids the component, or lets it use through `allow`, is
        // judged there, so an item that only moves between the two is none.
        let (base, head) = (&self.sides.base.config, &self.sides.head.config);
        for (forbid, before, after) in [
            (Forbid::Symbol, &then.forbid_symbol, forbid_symbol),
            (Forbid::External, &then.forbid_external, forbid_external),
        ] {
            self.items_that(
                at,
                forbid.key(),
                &Items::forbidden(before, reasons.0),
                &Items::forbidden(after, reasons.1),
                Direction::Tightened,
                |forbidden, edit| {
                    let at_top = |config| top_level_item(config, forbid, forbidden);
                    match edit {
                        Edit::Added => {
                            !forbids(base, forbid, name, forbidden)
                                && !at_top(head).is_some_and(|item| !item.allows(name))
                        }
                        Edit::Removed => {
                            !forbids(head, forbid, name, forbidden)
                                && !at_top(base).is_some_and(|item| !item.allows(name))
                                && !at_top(head).is_some_and(|item| item.allows(name))
                        }
                    }
                },
            );
        }
        for (key, before, after) in [
            ("forbid_file", &then.forbid_file, forbid_file),
            ("must_depend_on", &then.must_depend_on, must_depend_on),
            (
                "must_use_external",
                &then.must_use_external,
                must_use_external,
            ),
            ("requires_file", &then.requires_file, requires_file),
            (
                "requires_directive",
                &then.requires_directive,
                requires_directive,
            ),
        ] {
            self.items(
                at,
                key,
                &Items::listed(before, reasons.0),
                &Items::listed(after, reasons.1),
                Direction::Tightened,
            );
        }
        self.allowed(
            at,
            "may_use_external",
            then.may_use_external
                .as_deref()
                .map(|l| Items::listed(l, reasons.0)),
            may_use_external
                .as_deref()
                .map(|l| Items::listed(l, reasons.1)),
        );
        self.allowed(
            at,
            "allow_implementers",
            then.allow_implementers
                .as_deref()
                .map(|l| Items::plain(l, reasons.0)),
            allow_implementers
                .as_deref()
                .map(|l| Items::plain(l, reasons.1)),
        );
        self.limit(at, "max_fan_out", (then.max_fan_out, *max_fan_out), reasons);
        self.limit(
            at,
            "max_instability",
            (then.max_instability, *max_instability),
            reasons,
        );
    }

    /// The entry of a component both policies have.
    fn component(&mut self, name: &str) {
        let then = &self.sides.base.config.components[name];
        let now = &self.sides.head.config.components[name];
        let at = At::Component(name);
        // Every key, so that a new one cannot be left out.
        let Component {
            // Which files the component owns: `ownership`.
            patterns: _,
            except: _,
            priority: _,
            part: _,
            // Who may import which files: `surfaces`.
            public: _,
            allow_importers: _,
            surfaces: _,
            // The reason of each change below.
            because: _,
            // Who to ask, not a clause.
            owner: _,
            public_symbols,
            isolated,
            defines,
            optional,
        } = now;
        let reasons = (then.because.as_deref(), now.because.as_deref());
        self.allowed(
            at,
            "public_symbols",
            then.public_symbols
                .as_deref()
                .map(|l| Items::plain(l, reasons.0)),
            public_symbols
                .as_deref()
                .map(|l| Items::plain(l, reasons.1)),
        );
        // An import between instances shows the reason of the component, or
        // else that of its rule.
        self.flag(
            at,
            "isolated",
            (then.isolated, *isolated),
            Direction::Tightened,
            (
                reasons.0.or(self.sides.base.config.reason(name)),
                reasons.1.or(self.sides.head.config.reason(name)),
            ),
        );
        self.items(
            at,
            "defines",
            &Items::plain(&then.defines, reasons.0),
            &Items::plain(defines, reasons.1),
            Direction::Tightened,
        );
        self.flag(
            at,
            "optional",
            (then.optional, *optional),
            Direction::Loosened,
            (None, None),
        );
    }
}

impl ConstraintChange {
    /// The change in one line, as the text lists it:
    /// ``web.forbid_symbol: removed `tags.DeleteIn` (loosened)``.
    pub(super) fn line(&self) -> String {
        let key = match &self.component {
            Some(component) => format!("{component}.{}", self.key),
            None => self.key.to_string(),
        };
        let what = match (&self.item, self.edit) {
            (Some(item), Some(edit)) => {
                let verb = match edit {
                    Edit::Added => "added",
                    Edit::Removed => "removed",
                };
                let noun = if self.key == "diagram" { " arrow" } else { "" };
                format!("{verb}{noun} `{item}`")
            }
            // Who may use a forbidden name, or where.
            (Some(item), None) => {
                let (by, who) = if self.paths.is_empty() {
                    ("by", &self.components)
                } else {
                    ("in", &self.paths)
                };
                format!(
                    "`{item}` may {} be used {by} {}",
                    match self.change {
                        Direction::Loosened => "now",
                        Direction::Tightened => "no longer",
                    },
                    shown(Some(&Value::from(who.clone())))
                )
            }
            _ => format!(
                "{} (was {})",
                shown(self.after.as_ref()),
                shown(self.before.as_ref())
            ),
        };
        format!("{key}: {what} ({})", self.change.as_str())
    }
}

/// A value as the text shows it: `unset` for none, lists and paths quoted.
fn shown(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "unset".to_string(),
        Some(Value::Array(items)) if items.is_empty() => "none".to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| format!("`{}`", i.as_str().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(path)) => format!("`{path}`"),
        Some(other) => other.to_string(),
    }
}

/// Whether `config` forbids `component` the name or module spelled `name` in
/// the lists of `forbid`: its own rule lists it, or a top-level item does and
/// leaves the component out of `allow`. One spelled otherwise is another item,
/// even when it names the same, so a respelling stays listed both ways.
fn forbids(config: &Config, forbid: Forbid, component: &str, name: &str) -> bool {
    config.rules.get(component).is_some_and(|rule| {
        rule.own_forbidden(forbid)
            .iter()
            .any(|own| own.name() == name)
    }) || top_level_item(config, forbid, name).is_some_and(|item| !item.allows(component))
}

/// The item of the top-level list of `forbid` spelled `name`. Two items of one
/// spelling forbid alike: the policy refuses a second one when either has
/// `allow`.
fn top_level_item<'a>(config: &'a Config, forbid: Forbid, name: &str) -> Option<&'a ForbiddenName> {
    config
        .top_forbidden(forbid)
        .iter()
        .find(|item| item.name() == name)
}

/// Where an arrow is drawn: the diagram of `policy`, under `diagram`.
fn drawn_in(policy: &Policy) -> Provenance {
    Provenance {
        field: "diagram".to_string(),
        input: policy
            .diagram()
            .ok()
            .flatten()
            .map(|input| policy.input_name(input)),
        through: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{compare, compare_with};
    use super::*;

    const TWO: &str = "version: 1
components:
  web: { in: web }
  db: { in: db }
";

    /// Each change as `component.key: item (direction)`, short to compare.
    fn said(changes: &[ConstraintChange]) -> Vec<String> {
        changes
            .iter()
            .map(|c| {
                let key = match &c.component {
                    Some(component) => format!("{component}.{}", c.key),
                    None => c.key.to_string(),
                };
                let what = match (&c.item, &c.before, &c.after) {
                    (Some(item), _, _) => item.clone(),
                    (None, before, after) => format!(
                        "{} -> {}",
                        before.as_ref().unwrap(),
                        after.as_ref().unwrap()
                    ),
                };
                format!("{key}: {what} ({:?})", c.change)
            })
            .collect()
    }

    #[test]
    fn an_item_added_to_a_list_that_forbids_tightens_and_one_removed_loosens() {
        let base = format!(
            "{TWO}forbid_symbol: [{{ name: tags.DeleteIn, because: tags are kept forever }}]\n"
        );
        let head = format!(
            "{TWO}rules:\n  web: {{ forbid_external: [lodash], because: pages stay small }}\n"
        );
        let changes = compare(&base, &head, changes);
        assert_eq!(
            said(&changes),
            [
                "forbid_symbol: tags.DeleteIn (Loosened)",
                "web.forbid_external: lodash (Tightened)",
            ]
        );
        let (removed, added) = (&changes[0], &changes[1]);
        assert_eq!(removed.because.as_deref(), Some("tags are kept forever"));
        assert_eq!(removed.provenance.field, "forbid_symbol[0]");
        // A plain item gives the reason of its rule.
        assert_eq!(added.because.as_deref(), Some("pages stay small"));
        assert_eq!(added.provenance.field, "rules.web.forbid_external[0]");
        assert_eq!(added.provenance.input.as_deref(), Some("easement.yml"));
    }

    #[test]
    fn a_component_let_into_a_forbidden_name_loosens_and_one_left_out_tightens() {
        let three = TWO.replace("db: { in: db }", "db: { in: db }\n  jobs: { in: jobs }");
        let item = |allow: &str| {
            format!(
                "{three}forbid_symbol:\n  - tags.Rename\n  - {{ name: tags.DeleteIn{allow}, because: a use case }}\n"
            )
        };
        let (plain, web, db) = (item(""), item(", allow: [web]"), item(", allow: [db]"));
        let found = compare(&plain, &web, changes);
        assert_eq!(said(&found), ["forbid_symbol: tags.DeleteIn (Loosened)"]);
        let wider = &found[0];
        assert_eq!(wider.edit, None);
        assert_eq!(wider.components, ["web"]);
        assert_eq!(wider.because.as_deref(), Some("a use case"));
        assert_eq!(wider.provenance.field, "forbid_symbol[1].allow");
        assert_eq!(
            wider.line(),
            "forbid_symbol: `tags.DeleteIn` may now be used by `web` (loosened)"
        );
        // One component let in and another left out: both ways at once.
        let swapped = compare(&web, &db, changes);
        assert_eq!(
            said(&swapped),
            [
                "forbid_symbol: tags.DeleteIn (Loosened)",
                "forbid_symbol: tags.DeleteIn (Tightened)",
            ]
        );
        assert_eq!(
            (&swapped[0].components, &swapped[1].components),
            (&vec!["db".to_string()], &vec!["web".to_string()])
        );
        assert_eq!(
            swapped[1].line(),
            "forbid_symbol: `tags.DeleteIn` may no longer be used by `web` (tightened)"
        );
        let narrowed = compare(&db, &plain, changes);
        assert_eq!(
            said(&narrowed),
            ["forbid_symbol: tags.DeleteIn (Tightened)"]
        );
        // No `allow` left to point at: the item itself.
        assert_eq!(narrowed[0].provenance.field, "forbid_symbol[1]");
        // Only reordered: no change.
        let both = item(", allow: [web, db]");
        assert!(compare(&both, &item(", allow: [db, web]"), changes).is_empty());
    }

    #[test]
    fn a_path_let_into_a_forbidden_name_loosens_and_one_left_out_tightens() {
        let item = |grants: &str| {
            format!(
                "{TWO}forbid_symbol:\n  - tags.Rename\n  - {{ name: tags.DeleteIn{grants}, because: a use case }}\n"
            )
        };
        let plain = item("");
        let admin = item(", allow_in: [web/admin]");
        let found = compare(&plain, &admin, changes);
        assert_eq!(said(&found), ["forbid_symbol: tags.DeleteIn (Loosened)"]);
        let wider = &found[0];
        assert_eq!(wider.edit, None);
        assert!(wider.components.is_empty());
        assert_eq!(wider.paths, ["web/admin"]);
        assert_eq!(wider.because.as_deref(), Some("a use case"));
        assert_eq!(wider.provenance.field, "forbid_symbol[1].allow_in");
        assert_eq!(
            wider.line(),
            "forbid_symbol: `tags.DeleteIn` may now be used in `web/admin` (loosened)"
        );
        // One path let in and another left out, beside a component let in.
        let swapped = compare(
            &admin,
            &item(", allow: [web], allow_in: [db/jobs, db/sweep]"),
            changes,
        );
        let lines: Vec<String> = swapped.iter().map(ConstraintChange::line).collect();
        assert_eq!(
            lines,
            [
                "forbid_symbol: `tags.DeleteIn` may now be used by `web` (loosened)",
                "forbid_symbol: `tags.DeleteIn` may now be used in `db/jobs`, `db/sweep` (loosened)",
                "forbid_symbol: `tags.DeleteIn` may no longer be used in `web/admin` (tightened)",
            ]
        );
        let narrowed = compare(&admin, &plain, changes);
        assert_eq!(
            said(&narrowed),
            ["forbid_symbol: tags.DeleteIn (Tightened)"]
        );
        assert_eq!(narrowed[0].paths, ["web/admin"]);
        // No `allow_in` left to point at: the item itself.
        assert_eq!(narrowed[0].provenance.field, "forbid_symbol[1]");
        // Only reordered: no change; a new item is tightened as a whole.
        let both = item(", allow_in: [web/admin, db/jobs]");
        assert!(compare(&both, &item(", allow_in: [db/jobs, web/admin]"), changes).is_empty());
        let new = compare(
            &format!("{TWO}forbid_symbol: [tags.Rename]\n"),
            &admin,
            changes,
        );
        assert_eq!(said(&new), ["forbid_symbol: tags.DeleteIn (Tightened)"]);
        assert_eq!(new[0].edit, Some(Edit::Added));
    }

    const RUN: &str = "version: 1
components:
  app: { in: app }
  web: { in: web }
  wiring: { in: cmd/app }
";

    /// `os.Getenv` in the rules of `app` and `web`, spelled `own`.
    fn in_rules(own: &str) -> String {
        format!(
            "{RUN}rules:\n  app: {{ forbid_symbol: [{own}] }}\n  web: {{ forbid_symbol: [{own}] }}\n"
        )
    }

    /// `os.Getenv` forbidden at the top level, open to `allow`.
    fn at_top_level(name: &str, allow: &str) -> String {
        format!("{RUN}forbid_symbol:\n  - {{ name: {name}, allow: [{allow}] }}\n")
    }

    #[test]
    fn a_forbidden_name_moved_from_the_top_level_into_rules_loosens_only_for_the_rest() {
        let found = compare(
            &at_top_level("os.Getenv", "wiring"),
            &in_rules("os.Getenv"),
            changes,
        );
        // Files of no component may use it now; `app` and `web` still may not.
        assert_eq!(said(&found), ["forbid_symbol: os.Getenv (Loosened)"]);
    }

    #[test]
    fn an_item_a_top_level_item_already_forbids_is_no_change() {
        let top = format!("{RUN}forbid_symbol: [os.Getenv]\n");
        let both = format!("{top}rules:\n  web: {{ forbid_symbol: [os.Getenv] }}\n");
        assert!(compare(&top, &both, changes).is_empty());
        assert!(compare(&both, &top, changes).is_empty());
    }

    #[test]
    fn a_component_let_in_by_allow_as_its_own_item_goes_is_one_loosening() {
        let base = format!(
            "{}rules:\n  web: {{ forbid_symbol: [os.Getenv] }}\n",
            at_top_level("os.Getenv", "wiring")
        );
        let found = compare(&base, &at_top_level("os.Getenv", "wiring, web"), changes);
        assert_eq!(said(&found), ["forbid_symbol: os.Getenv (Loosened)"]);
        assert_eq!(found[0].components, ["web"]);
        // Shut out again and forbidden by its own rule: one tightening.
        let back = compare(&at_top_level("os.Getenv", "wiring, web"), &base, changes);
        assert_eq!(said(&back), ["forbid_symbol: os.Getenv (Tightened)"]);
        assert_eq!(back[0].components, ["web"]);
    }

    #[test]
    fn a_forbidden_name_respelled_as_it_moves_is_listed_both_ways() {
        let found = compare(
            &in_rules("os.Getenv"),
            &at_top_level("foo/os.Getenv", "wiring"),
            changes,
        );
        assert_eq!(
            said(&found),
            [
                "forbid_symbol: foo/os.Getenv (Tightened)",
                "app.forbid_symbol: os.Getenv (Loosened)",
                "web.forbid_symbol: os.Getenv (Loosened)",
            ]
        );
    }

    /// `github.com/samber/do` forbidden to every component but `wiring`, in
    /// each rule or by one top-level item.
    const DI_RULES: &str = "rules:\n  app: { forbid_external: [github.com/samber/do] }\n  web: { forbid_external: [github.com/samber/do] }\n";

    fn di_at_top_level(grants: &str) -> String {
        format!(
            "{RUN}forbid_external:\n  - {{ name: github.com/samber/do{grants}, because: no one sees the container }}\n"
        )
    }

    #[test]
    fn forbidden_modules_moved_from_the_rules_into_one_top_level_item_are_that_item_alone() {
        let found = compare(
            &format!("{RUN}{DI_RULES}"),
            &di_at_top_level(", allow: [wiring]"),
            changes,
        );
        // Files of no component may not use it now; `app` and `web` still may not.
        assert_eq!(
            said(&found),
            ["forbid_external: github.com/samber/do (Tightened)"]
        );
        assert_eq!(found[0].edit, Some(Edit::Added));
        assert_eq!(found[0].provenance.field, "forbid_external[0]");
        // And back: the item alone again.
        assert_eq!(
            said(&compare(
                &di_at_top_level(", allow: [wiring]"),
                &format!("{RUN}{DI_RULES}"),
                changes
            )),
            ["forbid_external: github.com/samber/do (Loosened)"]
        );
        // An item a top-level item already forbids is no change.
        let both = format!("{}{DI_RULES}", di_at_top_level(""));
        assert!(compare(&di_at_top_level(""), &both, changes).is_empty());
    }

    #[test]
    fn a_component_or_a_path_let_into_a_forbidden_module_loosens_and_one_left_out_tightens() {
        let wiring = di_at_top_level(", allow: [wiring]");
        let found = compare(&wiring, &di_at_top_level(", allow: [wiring, web]"), changes);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].components, ["web"]);
        assert_eq!(found[0].provenance.field, "forbid_external[0].allow");
        assert_eq!(
            found[0].line(),
            "forbid_external: `github.com/samber/do` may now be used by `web` (loosened)"
        );
        let paths = compare(
            &di_at_top_level(", allow: [wiring], allow_in: [app/boot.go]"),
            &di_at_top_level(", allow: [wiring], allow_in: [app/main.go]"),
            changes,
        );
        let lines: Vec<String> = paths.iter().map(ConstraintChange::line).collect();
        assert_eq!(
            lines,
            [
                "forbid_external: `github.com/samber/do` may now be used in `app/main.go` (loosened)",
                "forbid_external: `github.com/samber/do` may no longer be used in `app/boot.go` (tightened)",
            ]
        );
        assert_eq!(paths[0].provenance.field, "forbid_external[0].allow_in");
        assert_eq!(
            paths[0].because.as_deref(),
            Some("no one sees the container")
        );
        // A component let in as its own item goes: one loosening.
        let own = format!("{wiring}rules:\n  web: {{ forbid_external: [github.com/samber/do] }}\n");
        let found = compare(&own, &di_at_top_level(", allow: [wiring, web]"), changes);
        assert_eq!(
            said(&found),
            ["forbid_external: github.com/samber/do (Loosened)"]
        );
        assert_eq!(found[0].components, ["web"]);
    }

    #[test]
    fn a_list_that_allows_tightens_when_set_and_loosens_with_each_item_added() {
        let base = format!("{TWO}rules:\n  web: {{ max_fan_out: 3 }}\n");
        let set = format!("{TWO}rules:\n  web: {{ max_fan_out: 5, may_use_external: [react] }}\n");
        assert_eq!(
            said(&compare(&base, &set, changes)),
            [
                "web.may_use_external: null -> [\"react\"] (Tightened)",
                "web.max_fan_out: 3 -> 5 (Loosened)",
            ]
        );
        let wider = set.replace("[react]", "[react, lodash]");
        assert_eq!(
            said(&compare(&set, &wider, changes)),
            ["web.may_use_external: lodash (Loosened)"]
        );
        // Unset again, the rule entry dropped whole: no limit at all.
        assert_eq!(
            said(&compare(&wider, TWO, changes)),
            [
                "web.may_use_external: [\"react\",\"lodash\"] -> null (Loosened)",
                "web.max_fan_out: 5 -> null (Loosened)",
            ]
        );
    }

    #[test]
    fn flags_entries_and_public_symbols_go_the_way_each_key_means() {
        let base = TWO.replace("db: { in: db }", "db: { in: db/*, public_symbols: [Open] }");
        let head = TWO.replace(
            "db: { in: db }",
            "db: { in: db/*, optional: true, isolated: true }",
        ) + "strict: true\nentry: [cmd/**]\nmanifests: true\nunowned: [scripts/**]\n";
        assert_eq!(
            said(&compare(&base, &head, changes)),
            [
                "strict: false -> true (Tightened)",
                "unowned: scripts/** (Loosened)",
                "entry: null -> [\"cmd/**\"] (Tightened)",
                "manifests: false -> true (Tightened)",
                "db.public_symbols: [\"Open\"] -> null (Loosened)",
                "db.isolated: false -> true (Tightened)",
                "db.optional: false -> true (Loosened)",
            ]
        );
    }

    #[test]
    fn no_cycles_tightens_for_each_component_that_enters_it() {
        let base = format!("{TWO}no_cycles: [web]\n");
        let head = format!("{TWO}no_cycles: true\n");
        let changes = compare(&base, &head, changes);
        assert_eq!(said(&changes), ["no_cycles: db (Tightened)"]);
        assert_eq!(changes[0].provenance.field, "no_cycles");
        let back = compare(&head, &base, super::changes);
        assert_eq!(said(&back), ["no_cycles: db (Loosened)"]);
    }

    #[test]
    fn a_reordered_list_or_a_new_reason_alone_changes_no_constraint() {
        let base = format!(
            "{TWO}rules:\n  web: {{ forbid_external: [a, {{ name: b, because: old }}] }}\n"
        );
        let head = format!(
            "{TWO}rules:\n  web: {{ forbid_external: [{{ name: b, because: new }}, a], because: new }}\n"
        );
        assert_eq!(said(&compare(&base, &head, changes)), Vec::<String>::new());
    }

    const DRAWN: &str = "```mermaid\ngraph TD\n  web --> db\n```\n";
    const UNDRAWN: &str = "```mermaid\ngraph TD\n  web\n  db\n```\n";

    #[test]
    fn a_diagram_arrow_added_loosens_and_a_diagram_set_tightens() {
        let files = [
            ("old.md", UNDRAWN),
            ("new.md", DRAWN),
            ("bare.md", "no graph\n"),
        ];
        let old = format!("{TWO}diagram: old.md\n");
        let new = format!("{TWO}diagram: new.md\n");
        let changes = compare_with(&files, &old, &new, changes);
        assert_eq!(said(&changes), ["diagram: web -> db (Loosened)"]);
        assert_eq!(changes[0].provenance.input.as_deref(), Some("new.md"));
        assert_eq!(
            said(&compare_with(&files, TWO, &old, super::changes)),
            ["diagram: null -> \"old.md\" (Tightened)"]
        );

        // An arrow to a component the base policy lacks is listed too.
        let api = [("api.md", "```mermaid\ngraph TD\n  web --> api\n```\n")];
        let with_api = TWO.replace(
            "  db: { in: db }\n",
            "  db: { in: db }\n  api: { in: api }\n",
        );
        assert_eq!(
            said(&compare_with(
                &[files.as_slice(), &api].concat(),
                &old,
                &format!("{with_api}diagram: api.md\n"),
                super::changes
            )),
            ["diagram: web -> api (Loosened)"]
        );

        // The arrows of a component the head policy drops go with it: the
        // component is in `components`, not each of its arrows.
        assert_eq!(
            said(&compare_with(
                &[files.as_slice(), &api].concat(),
                &format!("{with_api}diagram: api.md\n"),
                &old,
                super::changes
            )),
            Vec::<String>::new()
        );

        // No arrow is guessed from a diagram that cannot be read.
        let bare = format!("{TWO}diagram: bare.md\n");
        assert_eq!(
            said(&compare_with(&files, &bare, &new, super::changes)),
            Vec::<String>::new()
        );
    }
}
