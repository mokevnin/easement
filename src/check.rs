use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::components::ComponentMap;
use crate::config::Config;
use crate::lang::Target;
use crate::scan::{Dep, slash};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleId {
    /// An import between components that `may_depend_on` does not allow.
    Dependency,
    /// An import of a path outside the target component's public API.
    Privacy,
    /// A dependency cycle between components.
    Cycle,
    /// An inline suppression that matches no violation.
    UnusedSuppression,
    /// An inline suppression without a reason.
    SuppressionReason,
}

impl RuleId {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleId::Dependency => "dependency",
            RuleId::Privacy => "privacy",
            RuleId::Cycle => "cycle",
            RuleId::UnusedSuppression => "unused-suppression",
            RuleId::SuppressionReason => "suppression-reason",
        }
    }

    /// Rules that an inline comment can suppress.
    pub fn suppressible(s: &str) -> Option<Self> {
        [RuleId::Dependency, RuleId::Privacy]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}

fn default_rule() -> RuleId {
    RuleId::Dependency
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub rule: RuleId,
    pub file: String,
    pub line: usize,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub from: String,
    pub to: String,
    pub target: String,
    pub specifier: String,
    /// Every import behind the violation; diff mode keeps it if any of them changed.
    #[serde(skip)]
    pub evidence: Vec<(String, usize)>,
}

/// Identity of a violation for the baseline: line numbers are left out so that
/// unrelated edits do not invalidate it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    #[serde(default = "default_rule")]
    pub rule: RuleId,
    pub file: String,
    pub target: String,
    pub from: String,
    pub to: String,
}

impl Violation {
    pub fn key(&self) -> Key {
        Key {
            rule: self.rule,
            file: self.file.clone(),
            target: self.target.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub files: usize,
    pub internal_deps: usize,
    pub external_deps: usize,
    pub unresolved_deps: usize,
    pub unmapped_files: usize,
    pub suppressed: usize,
    /// `file:line: specifier` for imports that look internal but did not resolve.
    pub unresolved: Vec<String>,
}

/// An import between two different components.
#[derive(Debug, Clone)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub file: String,
    pub line: usize,
}

/// Checks every internal dependency against the dependency and privacy rules.
/// Every violating import is returned, one per line; see [`dedupe`]. The edges
/// between components feed [`cycles`].
pub fn evaluate(
    config: &Config,
    map: &ComponentMap,
    deps: &[Dep],
) -> (Vec<Violation>, Vec<Edge>, Stats) {
    let mut stats = Stats::default();
    let mut found = Vec::new();
    let mut edges = Vec::new();
    let mut unmapped = BTreeSet::new();
    for dep in deps {
        let target = match &dep.target {
            Target::Internal(path) => slash(path),
            Target::External(_) => {
                stats.external_deps += 1;
                continue;
            }
            Target::Unresolved => {
                stats.unresolved_deps += 1;
                stats.unresolved.push(format!(
                    "{}:{}: {}",
                    slash(&dep.file),
                    dep.line,
                    dep.specifier
                ));
                continue;
            }
        };
        stats.internal_deps += 1;
        let file = slash(&dep.file);
        let Some(from) = map.component_of(&file) else {
            unmapped.insert(file);
            continue;
        };
        let Some(to) = map.component_of(&target) else {
            continue;
        };
        if from == to {
            continue;
        }
        edges.push(Edge {
            from: from.to_string(),
            to: to.to_string(),
            file: file.clone(),
            line: dep.line,
        });
        let (rule, message, because) = if !config.allows(from, to) {
            (
                RuleId::Dependency,
                format!("{from} -> {to} is not allowed (imports {})", dep.specifier),
                config.reason(from),
            )
        } else if !map.is_public(to, &target) {
            (
                RuleId::Privacy,
                format!(
                    "{from} -> {to}: {target} is not in the public API of {to} (imports {})",
                    dep.specifier
                ),
                config.components[to].because.as_deref(),
            )
        } else {
            continue;
        };
        found.push(Violation {
            rule,
            evidence: vec![(file.clone(), dep.line)],
            file,
            line: dep.line,
            message,
            because: because.map(str::to_string),
            from: from.to_string(),
            to: to.to_string(),
            target,
            specifier: dep.specifier.clone(),
        });
    }
    stats.unmapped_files = unmapped.len();
    (found, edges, stats)
}

/// Dependency cycles between the components covered by `no_cycles`: one
/// violation per strongly connected group, with the shortest cycle through it.
pub fn cycles(config: &Config, edges: &[Edge]) -> Vec<Violation> {
    let covered: Vec<&Edge> = edges
        .iter()
        .filter(|e| config.no_cycles.covers(&e.from) && config.no_cycles.covers(&e.to))
        .collect();
    let mut graph: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &covered {
        graph.entry(&e.from).or_default().insert(&e.to);
        graph.entry(&e.to).or_default();
    }
    let mut groups: Vec<BTreeSet<&str>> = strongly_connected(&graph)
        .into_iter()
        .filter(|g| g.len() > 1)
        .map(|g| g.into_iter().collect())
        .collect();
    groups.sort();
    groups
        .into_iter()
        .map(|members| {
            let start = *members.first().expect("non-empty group");
            let cycle = shortest_cycle(&graph, &members, start);
            let first = covered
                .iter()
                .filter(|e| e.from == cycle[0] && e.to == cycle[1])
                .min_by(|x, y| (&x.file, x.line).cmp(&(&y.file, y.line)))
                .expect("cycle edge exists");
            let path = cycle.join(" -> ");
            let names: Vec<&str> = members.iter().copied().collect();
            let message = if members.len() == cycle.len() - 1 {
                format!("component dependency cycle: {path}")
            } else {
                format!(
                    "component dependency cycle among {} components: {} (shortest: {path})",
                    members.len(),
                    names.join(", ")
                )
            };
            let mut evidence: Vec<(String, usize)> = covered
                .iter()
                .filter(|e| members.contains(e.from.as_str()) && members.contains(e.to.as_str()))
                .map(|e| (e.file.clone(), e.line))
                .collect();
            evidence.sort();
            Violation {
                rule: RuleId::Cycle,
                file: first.file.clone(),
                line: first.line,
                message,
                because: None,
                // The key of a cycle is its members only, so moving the evidence does not churn the baseline.
                from: names.join(","),
                to: String::new(),
                target: String::new(),
                specifier: String::new(),
                evidence,
            }
        })
        .collect()
}

/// Tarjan's algorithm; component graphs are small, so recursion is fine.
fn strongly_connected<'a>(graph: &BTreeMap<&'a str, BTreeSet<&'a str>>) -> Vec<Vec<&'a str>> {
    #[derive(Default)]
    struct State<'a> {
        index: BTreeMap<&'a str, usize>,
        low: BTreeMap<&'a str, usize>,
        stack: Vec<&'a str>,
        on_stack: BTreeSet<&'a str>,
        groups: Vec<Vec<&'a str>>,
    }
    fn visit<'a>(v: &'a str, graph: &BTreeMap<&'a str, BTreeSet<&'a str>>, s: &mut State<'a>) {
        let i = s.index.len();
        s.index.insert(v, i);
        s.low.insert(v, i);
        s.stack.push(v);
        s.on_stack.insert(v);
        for &w in &graph[v] {
            if !s.index.contains_key(w) {
                visit(w, graph, s);
                let low = s.low[v].min(s.low[w]);
                s.low.insert(v, low);
            } else if s.on_stack.contains(w) {
                let low = s.low[v].min(s.index[w]);
                s.low.insert(v, low);
            }
        }
        if s.low[v] == s.index[v] {
            let mut group = Vec::new();
            while let Some(w) = s.stack.pop() {
                s.on_stack.remove(w);
                group.push(w);
                if w == v {
                    break;
                }
            }
            s.groups.push(group);
        }
    }
    let mut state = State::default();
    for &v in graph.keys() {
        if !state.index.contains_key(v) {
            visit(v, graph, &mut state);
        }
    }
    state.groups
}

/// Shortest path from `start` back to itself inside `members`, breadth-first:
/// `[start, …, start]`.
fn shortest_cycle<'a>(
    graph: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    members: &BTreeSet<&'a str>,
    start: &'a str,
) -> Vec<&'a str> {
    let mut prev: BTreeMap<&str, &str> = BTreeMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(v) = queue.pop_front() {
        for &w in &graph[v] {
            if !members.contains(w) {
                continue;
            }
            if w == start {
                let mut path = vec![v];
                let mut cur = v;
                while cur != start {
                    cur = prev[cur];
                    path.push(cur);
                }
                path.reverse();
                path.push(start);
                return path;
            }
            if w != start && !prev.contains_key(w) {
                prev.insert(w, v);
                queue.push_back(w);
            }
        }
    }
    unreachable!("a strongly connected group has a cycle through every member")
}

/// One violation per key, the earliest line, sorted by key.
pub fn dedupe(violations: Vec<Violation>) -> Vec<Violation> {
    let mut unique: BTreeMap<Key, Violation> = BTreeMap::new();
    for v in violations {
        match unique.get(&v.key()) {
            Some(seen) if seen.line <= v.line => {}
            _ => {
                unique.insert(v.key(), v);
            }
        }
    }
    unique.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            file: format!("{from}/x"),
            line: 1,
        }
    }

    fn config(no_cycles: &str) -> Config {
        serde_norway::from_str(&format!(
            "version: 1
components:
  a: {{ in: a }}
  b: {{ in: b }}
  c: {{ in: c }}
  d: {{ in: d }}
no_cycles: {no_cycles}
"
        ))
        .unwrap()
    }

    #[test]
    fn reports_each_cycle_group_once_with_shortest_path() {
        let edges = [
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "a"),
            edge("b", "a"),
            edge("c", "d"),
        ];
        let found = cycles(&config("true"), &edges);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].message,
            "component dependency cycle among 3 components: a, b, c (shortest: a -> b -> a)"
        );
        assert_eq!(found[0].from, "a,b,c");
        assert_eq!(found[0].evidence.len(), 4);
    }

    #[test]
    fn simple_cycle_and_scoping() {
        let edges = [edge("a", "b"), edge("b", "c"), edge("c", "a")];
        let found = cycles(&config("true"), &edges);
        assert_eq!(
            found[0].message,
            "component dependency cycle: a -> b -> c -> a"
        );
        assert!(cycles(&config("[a, b]"), &edges).is_empty());
        assert!(cycles(&config("false"), &edges).is_empty());
    }
}
