//! Chains of dependencies through the graph: cycles between components, the
//! chains `must_not_reach` forbids, and the shortest of each as its witness.

use std::collections::{BTreeMap, BTreeSet};

use pathfinding::NodeRefs;
use pathfinding::directed::bfs::{bfs, bfs_loop, bfs_reach};
use pathfinding::directed::strongly_connected_components::strongly_connected_components;

use crate::check::{Chain, Edge, Hop, Place, RuleId, Violation};
use crate::components::ComponentMap;
use crate::config::{Config, Listed};
use crate::decision::{self, Refusal};
use crate::flow::{self, Flow};
use crate::lang::{Dep, registry};
use crate::policy::Policy;
use crate::relpath::{self, slash};

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
    let nodes: Vec<&str> = graph.keys().copied().collect();
    let mut groups: Vec<BTreeSet<&str>> =
        strongly_connected_components(&nodes, |v| graph[v].iter().copied())
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
            // The first import of each step of the cycle.
            let steps: Vec<&&Edge> = cycle
                .windows(2)
                .map(|step| {
                    covered
                        .iter()
                        .filter(|e| e.from == step[0] && e.to == step[1])
                        .min_by(|x, y| (&x.file, x.line).cmp(&(&y.file, y.line)))
                        .expect("cycle edge exists")
                })
                .collect();
            let first = steps[0];
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
            let mut evidence: Vec<Place> = covered
                .iter()
                .filter(|e| members.contains(e.from.as_str()) && members.contains(e.to.as_str()))
                .map(|e| Place::new(e.file.clone(), e.line))
                .collect();
            evidence.sort();
            Violation {
                file: first.file.clone(),
                line: first.line,
                message,
                // The key of a cycle is its members only, so moving the evidence does not churn the baseline.
                from: names.join(","),
                evidence,
                chain: Some(Chain::shortest(
                    steps
                        .iter()
                        .map(|e| Hop {
                            file: e.file.clone(),
                            line: e.line,
                            from_component: Some(e.from.clone()),
                            to_component: Some(e.to.clone()),
                        })
                        .collect(),
                )),
                ..Violation::of(RuleId::Cycle)
            }
        })
        .collect()
}

/// For each component and each one its `must_not_reach` names: the shortest
/// chain of imports between them through files of neither, reported at its
/// first import. Files of no component count as hops; an import straight
/// into the forbidden component is a `dependency` violation instead.
pub fn reaches(
    config: &Config,
    map: &ComponentMap,
    deps: &[Dep],
    files: &[String],
    flow: Option<&Flow>,
) -> Vec<Violation> {
    let pairs: Vec<(&str, &Listed)> = config
        .rules
        .iter()
        .flat_map(|(from, rule)| {
            rule.must_not_reach
                .iter()
                .map(move |to| (from.as_str(), to))
        })
        .collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let graph = match flow {
        Some(flow) => symbol_graph(deps, files, flow),
        None => file_graph(deps, files),
    };
    // A node is a file, or with `reach: symbols` a method of one.
    let component: BTreeMap<&str, &str> = graph
        .iter()
        .flat_map(|(f, out)| std::iter::once(f.as_str()).chain(out.iter().map(|(t, _)| t.as_str())))
        .filter_map(|f| map.component_of(flow::file_of(f)).map(|c| (f, c)))
        .collect();
    let mut found = Vec::new();
    for (from, to) in pairs {
        let to_name = to.name();
        let of = |file: &str| component.get(file).copied();
        let starts = graph
            .keys()
            .map(String::as_str)
            .filter(|f| of(f) == Some(from));
        let is_hop = |file: &str| !matches!(of(file), Some(c) if c == from || c == to_name);
        let Some((hops, last)) =
            shortest_chain(&graph, starts, is_hop, |file| of(file) == Some(to_name))
        else {
            continue;
        };
        let hops: Vec<Place> = hops
            .into_iter()
            .map(|(node, line)| Place::new(flow::file_of(&node), line))
            .collect();
        let last = flow::file_of(last);
        let (through, chain) = describe_chain(&hops, last, |file| map.component_of(file));
        let witness = Chain::of(&hops, last, |file| map.component_of(file));
        let Place { file, line, .. } = hops[0].clone();
        found.push(Violation {
            file,
            line,
            message: format!("{from} reaches {to_name} through {through}: {chain}"),
            because: to
                .because()
                .or_else(|| config.reason(from))
                .map(str::to_string),
            from: from.to_string(),
            to: to_name.to_string(),
            target: last.to_string(),
            evidence: hops,
            chain: Some(witness),
            ..Violation::of(RuleId::Reach)
        });
    }
    found
}

/// Names, on each violation one clause of the policy accounts for, where that
/// clause is written: the clause of the dependency decision behind a rejected
/// import, the prohibition a reach breaks, `no_cycles` for a cycle.
pub fn attribute(policy: &Policy, map: &ComponentMap, deps: &[Dep], violations: &mut [Violation]) {
    let config = &policy.config;
    let mut at: BTreeMap<(String, usize), Vec<&Dep>> = BTreeMap::new();
    for dep in deps {
        at.entry((slash(&dep.file), dep.line))
            .or_default()
            .push(dep);
    }
    for v in violations.iter_mut() {
        let site = match v.rule {
            RuleId::Reach if config.components.contains_key(&v.from) => {
                decision::refusal_site(config, &v.from, &v.to, &Refusal::Prohibited { reach: true })
            }
            RuleId::Cycle => Some(crate::policy::Site::of_key("no_cycles")),
            _ => at
                .get(&(v.file.clone(), v.line))
                .into_iter()
                .flatten()
                .filter_map(|dep| decision::decide(config, map, dep).ok())
                .find(|d| {
                    d.target == v.target
                        && d.grant.as_ref().err().is_some_and(|c| c.rule() == v.rule)
                })
                .and_then(|d| d.site(config)),
        };
        v.provenance = site.map(|site| policy.provenance(&site));
    }
}

/// What a file of no component is called in reports.
pub const NO_COMPONENT: &str = "no component";

/// A chain from [`shortest_chain`] for a reader: the components of its hops,
/// `util, no component`, and the chain itself, `a.kt:2 -> b.kt:3 -> c.kt`.
pub fn describe_chain<'a>(
    hops: &[Place],
    last: &str,
    of: impl Fn(&str) -> Option<&'a str>,
) -> (String, String) {
    let through: BTreeSet<&str> = hops[1..]
        .iter()
        .map(|hop| of(&hop.file).unwrap_or(NO_COMPONENT))
        .collect();
    let chain: Vec<String> = hops
        .iter()
        .map(ToString::to_string)
        .chain(std::iter::once(last.to_string()))
        .collect();
    (
        through.into_iter().collect::<Vec<_>>().join(", "),
        chain.join(" -> "),
    )
}

/// The files of a project by directory, the set of them, and the groups of
/// them that are built together (see [`registry::built_together`]).
struct Files<'a> {
    in_dir: BTreeMap<&'a str, Vec<&'a str>>,
    known: BTreeSet<&'a str>,
    together: BTreeMap<&'a str, std::rc::Rc<Vec<&'a str>>>,
}

impl<'a> Files<'a> {
    fn new(files: &'a [String]) -> Self {
        let mut in_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for file in files {
            in_dir.entry(relpath::parent(file)).or_default().push(file);
        }
        let known: BTreeSet<&str> = files.iter().map(String::as_str).collect();
        let mut together = BTreeMap::new();
        for group in registry::built_together(&known.iter().copied().collect::<Vec<_>>()) {
            let group = std::rc::Rc::new(group);
            for file in group.iter() {
                together.insert(*file, group.clone());
            }
        }
        Self {
            in_dir,
            known,
            together,
        }
    }

    /// The files an import of `path` names: the file with those built together
    /// with it (the other files of its Go package), or one of each file directly
    /// in a package directory.
    fn targets(&self, path: &str) -> &[&'a str] {
        match self.known.get(path) {
            Some(file) => match self.together.get(file) {
                Some(group) => group,
                None => std::slice::from_ref(file),
            },
            None => self.in_dir.get(path).map_or(&[], Vec::as_slice),
        }
    }
}

/// Every file -> the files it imports, with the line of the import. An
/// import of a package directory is one of each file directly in it.
pub fn file_graph<'a>(
    deps: impl IntoIterator<Item = &'a Dep>,
    files: &[String],
) -> BTreeMap<String, BTreeSet<(String, usize)>> {
    let files = Files::new(files);
    let mut graph: BTreeMap<String, BTreeSet<(String, usize)>> = BTreeMap::new();
    for dep in deps {
        // Every copy the import may mean, not only the one the adapter found
        // first: a path the import may take is not lost from the graph.
        let keys = dep.targets();
        if keys.is_empty() {
            continue;
        }
        let out = graph.entry(dep.from.key()).or_default();
        for key in &keys {
            for target in files.targets(&key.key()) {
                out.insert(((*target).to_string(), dep.line));
            }
        }
    }
    graph
}

/// The graph of `reach: symbols`: every node (see [`flow::Node::key`]) -> the nodes
/// its references name, with the line. An import that no reference of its file
/// accounts for reaches everything in what it names, from the definition that holds
/// its line.
fn symbol_graph(
    deps: &[Dep],
    files: &[String],
    flow: &Flow,
) -> BTreeMap<String, BTreeSet<(String, usize)>> {
    let files = Files::new(files);
    let mut graph: BTreeMap<String, BTreeSet<(String, usize)>> = BTreeMap::new();
    let mut covered: BTreeSet<(&str, usize, &str)> = BTreeSet::new();
    for edge in &flow.edges {
        let (from, to) = (edge.from.file.as_str(), edge.to.file.as_str());
        if !files.known.contains(from) || !files.known.contains(to) {
            continue;
        }
        covered.insert((from, edge.line, to));
        graph
            .entry(edge.from.key())
            .or_default()
            .insert((edge.to.key(), edge.line));
    }
    for dep in deps {
        let keys = dep.targets();
        if keys.is_empty() {
            continue;
        }
        let file = dep.from.key();
        // Each file the import names that no reference at its line reaches is reached
        // whole, as the other files of a Go package are: they run on import too.
        let targets: Vec<&str> = keys
            .iter()
            .flat_map(|unit| files.targets(&unit.key()))
            .copied()
            .filter(|target| !covered.contains(&(file.as_str(), dep.line, *target)))
            .collect();
        for from in flow.owners(&file, dep.line) {
            let out = graph.entry(from.key()).or_default();
            for target in &targets {
                for node in flow.nodes_of(target) {
                    out.insert((node.key(), dep.line));
                }
            }
        }
    }
    graph
}

/// Every file a chain of `successors` reaches from any of `roots`, the roots
/// included.
pub fn reach<'a, I: IntoIterator<Item = &'a str>>(
    roots: impl IntoIterator<Item = &'a str>,
    mut successors: impl FnMut(&'a str) -> I,
) -> BTreeSet<&'a str> {
    // `None` stands before every root, so one search starts from all of them.
    let roots: Vec<Option<&str>> = roots.into_iter().map(Some).collect();
    bfs_reach(None, |node: &Option<&'a str>| match node {
        None => roots.clone(),
        Some(file) => successors(file).into_iter().map(Some).collect(),
    })
    .flatten()
    .collect()
}

/// The shortest chain from any of `starts` through files `is_hop` accepts to
/// a file `is_end` accepts: each hop with the line of its import, and the
/// file it ends in. Every start is searched at once, so no start's longer
/// chain hides another's shorter one.
pub fn shortest_chain<'a>(
    graph: &'a BTreeMap<String, BTreeSet<(String, usize)>>,
    starts: impl Iterator<Item = &'a str>,
    is_hop: impl Fn(&str) -> bool,
    is_end: impl Fn(&str) -> bool,
) -> Option<(Vec<(String, usize)>, &'a str)> {
    /// A file the chain passes through, or the one it ends in.
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Step<'a> {
        Hop(&'a str),
        End(&'a str),
    }
    let starts: BTreeSet<&str> = starts.collect();
    let first: Vec<Step> = starts.iter().map(|file| Step::Hop(file)).collect();
    let path = bfs(
        first.iter().collect::<NodeRefs<_>>(),
        |step| {
            let Step::Hop(file) = *step else {
                return Vec::new();
            };
            graph
                .get(file)
                .into_iter()
                .flatten()
                .filter_map(|(next, _)| {
                    let next = next.as_str();
                    // From a start, an import of the end is direct, not a chain.
                    if is_end(next) && !starts.contains(file) {
                        Some(Step::End(next))
                    } else {
                        is_hop(next).then_some(Step::Hop(next))
                    }
                })
                .collect()
        },
        |step| matches!(step, Step::End(_)),
    )?;
    let files: Vec<&str> = path
        .iter()
        .map(|step| match *step {
            Step::Hop(file) | Step::End(file) => file,
        })
        .collect();
    let hops = files
        .windows(2)
        .map(|pair| {
            let line = graph[pair[0]]
                .iter()
                .find(|(to, _)| to == pair[1])
                .map(|(_, line)| *line)
                .expect("a step of the chain is an import");
            (pair[0].to_string(), line)
        })
        .collect();
    Some((hops, *files.last().expect("a chain ends in a file")))
}

/// Shortest path from `start` back to itself inside `members`, breadth-first:
/// `[start, …, start]`.
fn shortest_cycle<'a>(
    graph: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    members: &BTreeSet<&'a str>,
    start: &'a str,
) -> Vec<&'a str> {
    bfs_loop(&start, |v| {
        graph[v].iter().copied().filter(|w| members.contains(w))
    })
    .expect("a strongly connected group has a cycle through every member")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Target;

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

    /// `a/A.rb` calls `host` (line 2); `x/G.rb` defines `host` (lines 3-5) and `fetch` (7-9),
    /// and `fetch` calls `get` of `c/C.rb`. `host_calls` lists what `host` references.
    fn symbol_world(host_calls: &[&str]) -> (Config, ComponentMap, Vec<Dep>, Vec<String>, Flow) {
        use crate::lang::Scope;
        use crate::scip::{Document, Reference};
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  a: { in: a }
  c: { in: c }
rules:
  a: { must_not_reach: [c] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let dep =
            |from: &str, to: &str, line| Dep::module(from, line, "", Target::Internal(to.into()));
        let reference = |line, symbol: &str| Reference {
            line,
            columns: None,
            symbol: symbol.to_string(),
            spelled: None,
        };
        let scope = |symbol: &str, first, last| Scope {
            symbol: symbol.to_string(),
            first,
            last,
        };
        let host: Vec<Reference> = host_calls.iter().map(|s| reference(4, s)).collect();
        let documents = vec![
            Document {
                path: "a/A.rb".into(),
                references: vec![reference(2, "host().")],
                ..Document::default()
            },
            Document {
                path: "x/G.rb".into(),
                definitions: vec!["host().".into(), "fetch().".into()],
                scopes: vec![scope("host().", 3, 5), scope("fetch().", 7, 9)],
                references: host.into_iter().chain([reference(8, "get().")]).collect(),
                ..Document::default()
            },
            Document {
                path: "c/C.rb".into(),
                definitions: vec!["get().".into()],
                scopes: vec![scope("get().", 1, 1)],
                ..Document::default()
            },
        ];
        let deps = vec![dep("a/A.rb", "x/G.rb", 2), dep("x/G.rb", "c/C.rb", 8)];
        let files = ["a/A.rb", "x/G.rb", "c/C.rb"].map(String::from).to_vec();
        let flow = crate::scip::flow(&documents);
        (config, map, deps, files, flow)
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
        let hop = |from: &str, to: &str| Hop {
            file: format!("{from}/x"),
            line: 1,
            from_component: Some(from.into()),
            to_component: Some(to.into()),
        };
        // One witness: the shortest cycle, an import for each step.
        assert_eq!(
            found[0].chain,
            Some(Chain {
                witness: "shortest",
                hops: vec![hop("a", "b"), hop("b", "a")],
            })
        );
    }
    #[test]
    fn reach_reports_the_shortest_chain_from_any_file() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  a: { in: a }
  c: { in: c }
rules:
  a: { must_not_reach: [c] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let dep =
            |from: &str, to: &str, line| Dep::module(from, line, "", Target::Internal(to.into()));
        // `a/A1.kt` sorts first but its chain is the longer one.
        let deps = [
            dep("a/A1.kt", "x/L1.kt", 1),
            dep("x/L1.kt", "x/L2.kt", 2),
            dep("x/L2.kt", "c/C.kt", 3),
            dep("a/A2.kt", "x/S.kt", 4),
            dep("x/S.kt", "c/C.kt", 5),
        ];
        let files: Vec<String> = [
            "a/A1.kt", "a/A2.kt", "x/L1.kt", "x/L2.kt", "x/S.kt", "c/C.kt",
        ]
        .map(String::from)
        .to_vec();
        let found = reaches(&config, &map, &deps, &files, None);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].message,
            "a reaches c through no component: a/A2.kt:4 -> x/S.kt:5 -> c/C.kt"
        );
        assert_eq!(found[0].target, "c/C.kt");
    }
    #[test]
    fn reach_by_symbols_passes_a_method_that_reaches_nothing_forbidden() {
        let (config, map, deps, files, flow) = symbol_world(&[]);
        // By files, calling `host` reaches what `fetch` uses.
        assert_eq!(reaches(&config, &map, &deps, &files, None).len(), 1);
        assert!(reaches(&config, &map, &deps, &files, Some(&flow)).is_empty());
    }
    #[test]
    fn reach_by_symbols_keeps_a_chain_through_the_called_method() {
        // `host` calls `get` itself (line 4), so the chain stays, at the call of `host`.
        let (config, map, deps, files, flow) = symbol_world(&["get()."]);
        let found = reaches(&config, &map, &deps, &files, Some(&flow));
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].message,
            "a reaches c through no component: a/A.rb:2 -> x/G.rb:4 -> c/C.rb"
        );
    }
    #[test]
    fn reach_by_symbols_reaches_the_files_of_a_package_that_no_reference_names() {
        use crate::lang::Scope;
        use crate::scip::{Document, Reference};
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  a: { in: a }
  c: { in: c }
rules:
  a: { must_not_reach: [c] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let dep =
            |from: &str, to: &str, line| Dep::module(from, line, "", Target::Internal(to.into()));
        // `a/A.go` imports the package `x` at line 3 and calls `One` of `x/one.go`; `x/two.go`
        // holds code that runs on import and uses `c`.
        let documents = vec![
            Document {
                path: "a/A.go".into(),
                references: vec![Reference {
                    line: 3,
                    columns: None,
                    symbol: "One().".into(),
                    spelled: None,
                }],
                ..Document::default()
            },
            Document {
                path: "x/one.go".into(),
                definitions: vec!["One().".into()],
                scopes: vec![Scope {
                    symbol: "One().".into(),
                    first: 1,
                    last: 2,
                }],
                ..Document::default()
            },
            Document {
                path: "x/two.go".into(),
                ..Document::default()
            },
        ];
        let deps = [dep("a/A.go", "x", 3), dep("x/two.go", "c/C.go", 5)];
        let files = ["a/A.go", "x/one.go", "x/two.go", "c/C.go"]
            .map(String::from)
            .to_vec();
        let flow = crate::scip::flow(&documents);
        let found = reaches(&config, &map, &deps, &files, Some(&flow));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].message,
            "a reaches c through no component: a/A.go:3 -> x/two.go:5 -> c/C.go"
        );
    }
    #[test]
    fn of_cycles_as_short_the_first_in_name_order_is_shown() {
        let edges = [
            edge("a", "c"),
            edge("c", "a"),
            edge("a", "b"),
            edge("b", "a"),
        ];
        let found = cycles(&config("true"), &edges);
        assert_eq!(
            found[0].message,
            "component dependency cycle among 3 components: a, b, c (shortest: a -> b -> a)"
        );
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
    #[test]
    fn file_graph_keeps_an_edge_to_every_candidate() {
        let mut dep = Dep::module(
            "a/A.kt",
            1,
            "thing.Thing",
            Target::Internal("b/Thing.kt".into()),
        );
        dep.candidates = ["c/Thing.kt", "b/Thing.kt"].map(String::from).to_vec();
        let files = ["a/A.kt", "b/Thing.kt", "c/Thing.kt"]
            .map(String::from)
            .to_vec();
        let graph = file_graph(&[dep], &files);
        assert_eq!(
            graph["a/A.kt"],
            [("b/Thing.kt".to_string(), 1), ("c/Thing.kt".to_string(), 1)].into()
        );
    }
}
