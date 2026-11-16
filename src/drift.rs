//! `check --diff <base> --plan <file>`: the imports a change added across
//! components, compared with the plan `propose --plan` judged before the code
//! existed. It reports what the plan did not name and what it named that the
//! code lacks; it never changes the verdict.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::components::ComponentMap;
use crate::decision;
use crate::lang::{Dep, Target};
use crate::propose::{Proposal, To};
use crate::relpath::slash;

/// How a change departs from its plan.
#[derive(Debug, Default, Serialize)]
pub struct Drift {
    /// Imports the change added across components that no entry of the plan names.
    pub unplanned: Vec<Unplanned>,
    /// Entries of the plan that no import of the code read has, as the plan writes them.
    pub unrealized: Vec<Proposal>,
}

/// An import the change added that the plan did not name.
#[derive(Debug, Serialize)]
pub struct Unplanned {
    /// The file that holds it, and its first line.
    pub from: String,
    pub line: usize,
    /// The path it imports, or the external module.
    #[serde(flatten)]
    pub to: To,
    /// The component of `from`, and of the target: none for a file of no
    /// component or an external module.
    pub component: Option<String>,
    pub target_component: Option<String>,
}

/// What the comparison reads: the plan, the imports of the code read at head,
/// and what tells a changed line, a file in scope and a file no indexer read.
pub struct Compared<'a, C, S>
where
    C: Fn(&str, usize) -> bool,
    S: Fn(&str) -> bool,
{
    pub plan: &'a [Proposal],
    pub deps: &'a [Dep],
    pub map: &'a ComponentMap,
    pub changed: C,
    pub in_scope: S,
    pub unindexed: &'a [String],
}

/// One import of the code: the file that holds it, its line, and what it
/// imports, the copy of a target it means as the rules judge it.
struct Import<'a> {
    file: String,
    line: usize,
    target: Target,
    dep: &'a Dep,
}

/// The imports of one file of one target, every line of them taken together.
struct Pair<'a> {
    target: &'a Target,
    from_component: Option<&'a str>,
    to_component: Option<&'a str>,
    line: usize,
    /// Whether every line of the pair is a changed one: the import is new, as
    /// `new_dependencies` tells a new edge between components.
    new: bool,
}

/// What the change did that the plan did not say, and what the plan said that
/// the code does not do.
pub fn compare<C, S>(compared: &Compared<'_, C, S>) -> Drift
where
    C: Fn(&str, usize) -> bool,
    S: Fn(&str) -> bool,
{
    let map = compared.map;
    let imports: Vec<Import> = compared
        .deps
        .iter()
        .map(|dep| Import {
            file: slash(&dep.file),
            line: dep.line,
            target: decision::target_meant(map, dep),
            dep,
        })
        .collect();
    let mut pairs: BTreeMap<(&str, String), Pair> = BTreeMap::new();
    for import in imports.iter().filter(|i| (compared.in_scope)(&i.file)) {
        let from_component = map.component_of_unit(&import.dep.from);
        let to_component = match &import.target {
            Target::Internal(unit) => map.component_of_unit(unit),
            Target::External(_) => None,
        };
        // A file of no component is a side of its own, and an external module
        // is in none.
        let crossing = from_component.is_none() || from_component != to_component;
        if !crossing {
            continue;
        }
        let pair = pairs
            .entry((&import.file, import.target.name()))
            .or_insert_with(|| Pair {
                target: &import.target,
                from_component,
                to_component,
                line: import.line,
                new: true,
            });
        pair.line = pair.line.min(import.line);
        pair.new &= (compared.changed)(&import.file, import.line);
    }
    let mut unplanned: Vec<Unplanned> = pairs
        .into_iter()
        .filter(|(_, pair)| pair.new)
        .filter(|((file, _), pair)| !compared.plan.iter().any(|p| p.covers(file, pair.target)))
        .map(|((from, name), pair)| Unplanned {
            from: from.to_string(),
            line: pair.line,
            to: match pair.target {
                Target::Internal(_) => To::Path(name),
                Target::External(_) => To::External(name),
            },
            component: pair.from_component.map(str::to_string),
            target_component: pair.to_component.map(str::to_string),
        })
        .collect();
    unplanned.sort_by(|a, b| (&a.from, a.line).cmp(&(&b.from, b.line)));
    let mut unrealized: Vec<Proposal> = Vec::new();
    for proposal in compared.plan {
        // What no indexer read, or what the checks do not read as a source, the
        // comparison cannot tell: the evidence gaps name the first.
        let unread = compared.unindexed.contains(&proposal.from)
            || decision::unread(map, &proposal.from).is_some();
        let realized = imports.iter().any(|i| proposal.covers(&i.file, &i.target));
        if (compared.in_scope)(&proposal.from)
            && !unread
            && !realized
            && !unrealized.contains(proposal)
        {
            unrealized.push(proposal.clone());
        }
    }
    Drift {
        unplanned,
        unrealized,
    }
}

impl Drift {
    /// The sections of the text report.
    pub fn text(&self) -> String {
        if self.unplanned.is_empty() && self.unrealized.is_empty() {
            return "The change matches the plan.\n".to_string();
        }
        let mut out = String::new();
        if !self.unplanned.is_empty() {
            out.push_str("Not in the plan:\n");
            for u in &self.unplanned {
                out.push_str(&format!("  {}:{} -> {}\n", u.from, u.line, u.to.label()));
            }
        }
        if !self.unrealized.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("Planned, not in the code:\n");
            for p in &self.unrealized {
                out.push_str(&format!("  {}\n", p.label()));
            }
        }
        out
    }

    /// The notes of the `github` report: each unplanned import at its place,
    /// as `(file, line, message)`, then the unrealized entries in one note.
    pub fn notes(&self) -> Vec<(Option<(&str, usize)>, String)> {
        let mut notes: Vec<_> = self
            .unplanned
            .iter()
            .map(|u| {
                (
                    Some((u.from.as_str(), u.line)),
                    format!("Not in the plan: {} -> {}", u.from, u.to.label()),
                )
            })
            .collect();
        if !self.unrealized.is_empty() {
            let labels: Vec<String> = self.unrealized.iter().map(Proposal::label).collect();
            notes.push((
                None,
                format!("Planned, not in the code: {}", labels.join(", ")),
            ));
        }
        notes
    }
}
