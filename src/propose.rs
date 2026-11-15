//! `easement propose`: the dependency decision on dependencies whose code does
//! not exist yet, and what they would do together to the graph of components.
//! Each proposal becomes the dependency the code would have, judged by
//! [`decision::decide`] and overlaid on the project's dependencies, where the
//! rules that need the whole graph run as `check` runs them.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{Context, Result, bail};
use pathfinding::directed::bfs::bfs;
use serde::{Deserialize, Serialize};

use crate::chains;
use crate::check::{self, Edge, RuleId, Violation};
use crate::components::ComponentMap;
use crate::config::{Config, TESTS};
use crate::decision::{self, Clause, Outside};
use crate::flow::Flow;
use crate::lang::Dep;
use crate::lang::{Target, Unit, registry};
use crate::policy::Policy;
use crate::relpath;
use crate::report::{self, Coverage};
use crate::stdlib;

/// What a proposed dependency would import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum To {
    /// A path of the project, relative to the root.
    Path(String),
    /// An external module.
    External(String),
}

/// A proposed dependency: the file that would hold the import, relative to the
/// root, and what it would import. Neither needs to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub from: String,
    pub to: To,
}

/// The line of the first proposal's dependency, past any line a file has: evidence
/// at `FIRST_LINE + i` is proposal `i`'s, never an import of the code.
const FIRST_LINE: usize = usize::MAX / 2;

/// The proposal whose dependency sits at `line`, if one does.
fn proposal_at(line: usize) -> Option<usize> {
    line.checked_sub(FIRST_LINE)
}

impl Proposal {
    /// `from -> to`, as reports name the proposal.
    pub fn label(&self) -> String {
        match &self.to {
            To::Path(path) => format!("{} -> {path}", self.from),
            To::External(module) => format!("{} -> external {module}", self.from),
        }
    }

    /// Whether the proposal is the import of `to` from `from`.
    fn is(&self, from: &str, to: &str) -> bool {
        self.from == from && self.to == To::Path(to.to_string())
    }

    /// The dependency the code would have, as the `index`th proposal.
    fn dep(&self, index: usize) -> Dep {
        let (specifier, target) = match &self.to {
            To::Path(path) => (path.clone(), Target::Internal(Unit::from(path.as_str()))),
            To::External(module) => (module.clone(), Target::External(module.clone())),
        };
        Dep::module(&self.from, FIRST_LINE + index, specifier, target)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Planned {
    from: String,
    to: Option<String>,
    external: Option<String>,
}

/// The proposals of a plan, a JSON list of `{"from": ..., "to": ...}` and
/// `{"from": ..., "external": ...}`, with each path made relative to the root by `relative`.
pub fn plan(text: &str, relative: impl Fn(&str) -> String) -> Result<Vec<Proposal>> {
    let items: Vec<Planned> = serde_json::from_str(text).context(
        r#"a plan is a JSON list of {"from": <path>, "to": <path>} and {"from": <path>, "external": <module>}"#,
    )?;
    if items.is_empty() {
        bail!("the plan proposes nothing");
    }
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let to = match (item.to, item.external) {
                (Some(to), None) => To::Path(relative(&to)),
                (None, Some(module)) => To::External(module),
                _ => bail!(
                    "proposal {} of the plan needs exactly one of `to` and `external`",
                    i + 1
                ),
            };
            Ok(Proposal {
                from: relative(&item.from),
                to,
            })
        })
        .collect()
}

/// What the policy says of a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Allowed,
    Rejected,
    /// No component owns the source: a rejection (`orphan`) under `strict`,
    /// unless `unowned` lists it.
    Unowned,
    /// `exclude` hides the source, so no rule judges its dependencies.
    Excluded,
    /// `target_only` keeps the source as a target alone, so no rule judges its dependencies.
    #[serde(rename = "target_only")]
    TargetOnly,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Allowed => "allowed",
            Kind::Rejected => "rejected",
            Kind::Unowned => "unowned",
            Kind::Excluded => "excluded",
            Kind::TargetOnly => "target_only",
        }
    }
}

/// The dependency decision on one proposal.
#[derive(Debug, Serialize)]
pub struct Judged {
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external: Option<String>,
    /// The component of `from`; none when no component owns it.
    pub component: Option<String>,
    /// The component of the target: none for an external module, a target
    /// `exclude` hides, or one no component owns.
    pub target_component: Option<String>,
    pub decision: Kind,
    /// The grant that allows it, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<&'static str>,
    /// The rule that rejects it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleId>,
    /// What the rule says of it, as `check` says it of the import once written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The reason the policy gives for the grant or the rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// The public surface of the target component, when the target is outside it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub public_surface: Vec<String>,
    /// The named surfaces of the target component open to the proposing
    /// component, when the target is outside them all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub named_surfaces: Vec<NamedSurface>,
    /// Rejected by a rule in `warn`: reported, but it does not fail.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub warning: bool,
}

/// A named surface of the target open to the proposing component.
#[derive(Debug, Serialize)]
pub struct NamedSurface {
    pub name: String,
    pub paths: Vec<String>,
}

fn owned(paths: Vec<&str>) -> Vec<String> {
    paths.into_iter().map(str::to_string).collect()
}

impl Judged {
    /// The proposal with a decision of `kind` and nothing else said of it.
    fn undecided(proposal: &Proposal, decision: Kind) -> Self {
        let (to, external) = match &proposal.to {
            To::Path(path) => (Some(path.clone()), None),
            To::External(module) => (None, Some(module.clone())),
        };
        Judged {
            from: proposal.from.clone(),
            to,
            external,
            component: None,
            target_component: None,
            decision,
            grant: None,
            rule: None,
            message: None,
            because: None,
            public_surface: Vec::new(),
            named_surfaces: Vec::new(),
            warning: false,
        }
    }

    /// Whether the decision fails the run.
    pub fn fails(&self) -> bool {
        self.rule.is_some() && !self.warning
    }

    /// What the proposal imports, as reports name it.
    pub fn target(&self) -> String {
        match (&self.to, &self.external) {
            (Some(path), _) => path.clone(),
            (None, Some(module)) => format!("external {module}"),
            (None, None) => String::new(),
        }
    }

    /// The decision in a few words: `allowed by common (web -> storage)`,
    /// `rejected [privacy]`.
    pub fn verdict(&self) -> String {
        let pair = match (&self.component, &self.target_component) {
            (Some(from), Some(to)) => format!(" ({from} -> {to})"),
            (Some(from), None) => format!(" ({from})"),
            _ => String::new(),
        };
        match (self.decision, self.rule, self.grant) {
            (_, Some(rule), _) => format!("{} {}", self.decision.as_str(), tag(rule, self.warning)),
            (Kind::Allowed, None, Some(grant)) => format!("allowed by {grant}{pair}"),
            (Kind::Allowed, None, None) => format!("allowed{pair}: no rule restricts it"),
            (kind, None, _) => kind.as_str().to_string(),
        }
    }
}

/// The dependency decision on each proposal, in order.
pub fn judge(config: &Config, map: &ComponentMap, proposals: &[Proposal]) -> Vec<Judged> {
    proposals
        .iter()
        .enumerate()
        .map(|(i, proposal)| {
            let dep = proposal.dep(i);
            let decision = match decision::decide(config, map, &dep) {
                Ok(decision) => decision,
                Err(Outside::Excluded) => {
                    return Judged {
                        message: Some(format!(
                            "`exclude` hides {}, so no rule judges its dependencies",
                            proposal.from
                        )),
                        ..Judged::undecided(proposal, Kind::Excluded)
                    };
                }
                Err(Outside::TargetOnly) => {
                    return Judged {
                        message: Some(format!(
                            "`target_only` names {}, so no rule judges its dependencies",
                            proposal.from
                        )),
                        ..Judged::undecided(proposal, Kind::TargetOnly)
                    };
                }
                Err(Outside::Unowned) => {
                    let rejected = config.strict && !map.is_unowned(&proposal.from);
                    return Judged {
                        rule: rejected.then_some(RuleId::Orphan),
                        message: Some(if rejected {
                            format!("{} belongs to no component (strict mode)", proposal.from)
                        } else {
                            format!(
                                "{} belongs to no component, so no rule judges its dependencies",
                                proposal.from
                            )
                        }),
                        warning: rejected && config.warns(RuleId::Orphan),
                        ..Judged::undecided(proposal, Kind::Unowned)
                    };
                }
            };
            let violation = check::violation(&decision, &dep, &check::Reexport::default());
            let (public_surface, named_surfaces) = match (&decision.grant, decision.to) {
                (Err(Clause::OutsideSurface { .. }), Some(owner)) => {
                    let verdict = decision::pair(config, map, decision.from, owner);
                    (
                        owned(verdict.surface.unwrap_or_default()),
                        verdict
                            .open_named
                            .into_iter()
                            .map(|s| NamedSurface {
                                name: s.name.to_string(),
                                paths: owned(s.paths),
                            })
                            .collect(),
                    )
                }
                _ => (Vec::new(), Vec::new()),
            };
            Judged {
                // A file of no component is judged by the top-level `forbid_external` alone.
                component: Some(decision.from)
                    .filter(|from| !from.is_empty())
                    .map(str::to_string),
                target_component: decision.to.map(str::to_string),
                decision: if decision.grant.is_ok() {
                    Kind::Allowed
                } else {
                    Kind::Rejected
                },
                grant: decision
                    .grant
                    .as_ref()
                    .ok()
                    .copied()
                    .flatten()
                    .map(|g| g.as_str()),
                rule: violation.as_ref().map(|v| v.rule),
                warning: violation.as_ref().is_some_and(|v| config.warns(v.rule)),
                message: violation.map(|v| v.message),
                because: decision.because.map(str::to_string),
                public_surface,
                named_surfaces,
                ..Judged::undecided(proposal, Kind::Allowed)
            }
        })
        .collect()
}

/// The extensions of the sources of external proposals whose standard library
/// `$std` cannot tell, where the source's rules name `$std`: an evidence gap of
/// their decisions, as of `check`'s.
pub fn std_unknown(map: &ComponentMap, judged: &[Judged]) -> Vec<String> {
    let found: BTreeSet<String> = judged
        .iter()
        .filter(|j| j.external.is_some())
        .filter(|j| j.component.as_deref().is_some_and(|c| map.uses_std(c)))
        .filter(|j| !stdlib::knows(&j.from))
        .map(|j| relpath::extension(&j.from).to_string())
        .collect();
    found.into_iter().collect()
}

/// What a reader must know of how the proposals were judged: a source whose
/// language has test-only parts (see [`registry::Language::parts`]) is judged
/// as the code of the whole file, since only a path tells test code here.
pub fn notes(config: &Config, map: &ComponentMap, proposals: &[Proposal]) -> Vec<String> {
    if config.tests.is_none() {
        return Vec::new();
    }
    let sources: BTreeSet<&str> = proposals
        .iter()
        .map(|p| p.from.as_str())
        .filter(|from| registry::of(from).is_some_and(|l| l.parts.is_some()))
        .filter(|from| map.component_of(from) != Some(TESTS))
        .collect();
    sources
        .into_iter()
        .map(|from| {
            format!(
                "{from} is judged as code outside the tests: code its language builds only for tests cannot be proposed, only a path the test globs match"
            )
        })
        .collect()
}

/// One step of a witness: an edge between components, or for a reach chain, an
/// import between files.
#[derive(Debug, Serialize)]
pub struct Step {
    pub from: String,
    pub to: String,
    /// The import that makes the step, when the code has it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The proposal that makes the step, by its index in `proposals`, when the code does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<usize>,
}

impl Step {
    /// A step at `line` of `file`: a proposal's when the line is one.
    fn at(from: &str, to: &str, file: &str, line: usize) -> Self {
        let proposal = proposal_at(line);
        Step {
            from: from.to_string(),
            to: to.to_string(),
            file: proposal.is_none().then(|| file.to_string()),
            line: proposal.is_none().then_some(line),
            proposal,
        }
    }

    /// The step of components `from -> to`: its first import in the code, else
    /// the first proposal that makes it.
    fn of_edge(from: &str, to: &str, edges: &[Edge]) -> Self {
        let edge = edges
            .iter()
            .filter(|e| e.from == from && e.to == to)
            .min_by_key(|e| (proposal_at(e.line).is_some(), &e.file, e.line))
            .expect("a step of a witness is an edge");
        Step::at(from, to, &edge.file, edge.line)
    }
}

/// What the proposals together do to the graph that it does not do now.
#[derive(Debug, Serialize)]
pub struct Consequence {
    /// `cycle`, `reach`, `metric` or `diagram`.
    pub rule: RuleId,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// The components the rule names, as `check` reports them: the members of
    /// a cycle joined by commas, the component a limit or a reach is about.
    pub from: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    /// The limit crossed, for `metric`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// The steps that make it, with the proposals among them.
    pub witness: Vec<Step>,
    /// The proposals in the witness, by their index in `proposals`.
    pub proposals: Vec<usize>,
    /// Of a rule in `warn`: reported, but it does not fail.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub warning: bool,
}

/// The project's graph: what the rules that judge it read.
pub struct Graph<'a> {
    pub policy: &'a Policy,
    pub map: &'a ComponentMap,
    pub deps: &'a [Dep],
    /// Every file that is not excluded.
    pub files: &'a [String],
    pub flow: Option<&'a Flow>,
}

/// The violations of the rules that judge the whole graph, and its edges between components.
fn graph_rules(graph: &Graph) -> Result<(Vec<Violation>, Vec<Edge>)> {
    let config = &graph.policy.config;
    let (_, edges, _) = check::evaluate(config, graph.map, graph.deps);
    let mut found = chains::cycles(config, &edges);
    found.extend(chains::reaches(
        config,
        graph.map,
        graph.deps,
        graph.files,
        graph.flow,
    ));
    found.extend(check::metrics(config, &edges));
    found.extend(crate::diagram::check(graph.policy, &edges)?);
    Ok((found, edges))
}

/// The cycles, reach chains, crossed limits and missing diagram arrows that the
/// proposals together add to the graph: those whose violation `check` would
/// report once their code is written, and does not report now.
pub fn consequences(graph: &Graph, proposals: &[Proposal]) -> Result<Vec<Consequence>> {
    let (before, _) = graph_rules(graph)?;
    let mut deps = graph.deps.to_vec();
    // A source the scan does not read (`exclude`, `target_only`) is not read once written either.
    deps.extend(
        proposals
            .iter()
            .enumerate()
            .filter(|(_, p)| decision::unread(graph.map, &p.from).is_none())
            .map(|(i, p)| p.dep(i)),
    );
    // A path the proposals name that does not exist yet is a file of the graph
    // once written: a chain through it must not vanish.
    let mut files = graph.files.to_vec();
    let known: HashSet<&str> = graph.files.iter().map(String::as_str).collect();
    let mut new: BTreeSet<&str> = BTreeSet::new();
    for proposal in proposals {
        new.insert(&proposal.from);
        if let To::Path(path) = &proposal.to {
            new.insert(path);
        }
    }
    files.extend(
        new.into_iter()
            .filter(|f| !known.contains(f))
            .map(str::to_string),
    );
    let (after, edges) = graph_rules(&Graph {
        deps: &deps,
        files: &files,
        ..*graph
    })?;
    let old: BTreeSet<check::Key> = before.iter().map(Violation::key).collect();
    Ok(after
        .into_iter()
        .filter(|v| !old.contains(&v.key()))
        .map(|v| consequence(graph, v, &edges))
        .collect())
}

/// A new violation of the overlaid graph, told through the proposals in it.
/// Its own `file`, `line` and `message` may name a proposal's line, so they are
/// told again from the structure.
fn consequence(graph: &Graph, v: Violation, edges: &[Edge]) -> Consequence {
    let config = &graph.policy.config;
    let mut limit = None;
    let (message, witness) = match v.rule {
        RuleId::Cycle => {
            let members: BTreeSet<&str> = v.components().collect();
            let witness = cycle_witness(&members, edges);
            let path: Vec<&str> = witness
                .iter()
                .map(|s| s.from.as_str())
                .chain(witness.first().map(|s| s.from.as_str()))
                .collect();
            let path = path.join(" -> ");
            let message = if members.len() == witness.len() {
                format!("component dependency cycle: {path}")
            } else {
                format!(
                    "component dependency cycle among {} components: {} (shortest through a proposal: {path})",
                    members.len(),
                    v.from.replace(',', ", ")
                )
            };
            (message, witness)
        }
        RuleId::Reach => {
            let hops = &v.evidence;
            let witness: Vec<Step> = hops
                .iter()
                .enumerate()
                .map(|(i, hop)| {
                    let next = hops
                        .get(i + 1)
                        .map_or(v.target.as_str(), |next| next.file.as_str());
                    Step::at(&hop.file, next, &hop.file, hop.line)
                })
                .collect();
            let (through, _) =
                chains::describe_chain(hops, &v.target, |f| graph.map.component_of(f));
            (
                format!("{} reaches {} through {through}", v.from, v.to),
                witness,
            )
        }
        RuleId::Metric => {
            limit = Some(v.target.clone());
            let targets: BTreeSet<&str> = edges
                .iter()
                .filter(|e| e.from == v.from)
                .map(|e| e.to.as_str())
                .collect();
            let witness = targets
                .into_iter()
                .map(|to| Step::of_edge(&v.from, to, edges))
                .collect();
            (v.message.clone(), witness)
        }
        _ => (
            v.message.clone(),
            vec![Step::of_edge(&v.from, &v.to, edges)],
        ),
    };
    let mut proposals: BTreeSet<usize> = witness.iter().filter_map(|s| s.proposal).collect();
    // A group of components holds every cycle among them: each proposal that
    // makes an edge of it no import makes is in it, not only those of the witness.
    if v.rule == RuleId::Cycle {
        let members: BTreeSet<&str> = v.components().collect();
        for (from, to) in proposed_edges(&members, edges) {
            proposals.extend(
                edges
                    .iter()
                    .filter(|e| e.from == from && e.to == to)
                    .filter_map(|e| proposal_at(e.line)),
            );
        }
    }
    Consequence {
        rule: v.rule,
        message,
        because: v.because,
        warning: config.warns(v.rule),
        from: v.from,
        to: v.to,
        limit,
        witness,
        proposals: proposals.into_iter().collect(),
    }
}

/// The shortest cycle among `members` that takes an edge only the proposals
/// make, as its steps: the cycle the proposals close.
fn cycle_witness(members: &BTreeSet<&str>, edges: &[Edge]) -> Vec<Step> {
    let inside: Vec<&Edge> = edges
        .iter()
        .filter(|e| members.contains(e.from.as_str()) && members.contains(e.to.as_str()))
        .collect();
    let mut graph: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &inside {
        graph.entry(&e.from).or_default().insert(&e.to);
    }
    let cycle = proposed_edges(members, edges)
        .into_iter()
        .filter_map(|(a, b)| {
            let back = bfs(
                &b,
                |v| graph.get(v).into_iter().flatten().copied(),
                |v| *v == a,
            )?;
            Some(std::iter::once(a).chain(back).collect::<Vec<&str>>())
        })
        .min_by_key(Vec::len)
        .expect("a cycle the proposals add takes an edge only they make");
    cycle
        .windows(2)
        .map(|pair| Step::of_edge(pair[0], pair[1], edges))
        .collect()
}

/// The edges among `members` that only proposals make.
fn proposed_edges<'a>(members: &BTreeSet<&str>, edges: &'a [Edge]) -> BTreeSet<(&'a str, &'a str)> {
    let inside = edges
        .iter()
        .filter(|e| members.contains(e.from.as_str()) && members.contains(e.to.as_str()));
    let real: BTreeSet<(&str, &str)> = inside
        .clone()
        .filter(|e| proposal_at(e.line).is_none())
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    inside
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .filter(|pair| !real.contains(pair))
        .collect()
}

/// What `easement propose` says.
pub struct Report {
    pub proposals: Vec<Proposal>,
    pub judged: Vec<Judged>,
    /// None under `--no-graph`.
    pub consequences: Option<Vec<Consequence>>,
    pub notes: Vec<String>,
    pub coverage: Coverage,
    pub cached: Option<String>,
}

#[derive(Serialize)]
struct Json<'a> {
    passed: bool,
    proposals: &'a [Judged],
    /// Null when `--no-graph` left them unjudged.
    consequences: Option<&'a [Consequence]>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    notes: &'a [String],
    #[serde(flatten)]
    coverage: &'a Coverage,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached: Option<&'a str>,
}

impl Report {
    /// Whether every decision and every consequence passes.
    pub fn passed(&self) -> bool {
        !self.judged.iter().any(Judged::fails)
            && !self.consequences.iter().flatten().any(|c| !c.warning)
    }

    pub fn json(&self) -> String {
        serde_json::to_string_pretty(&Json {
            passed: self.passed(),
            proposals: &self.judged,
            consequences: self.consequences.as_deref(),
            notes: &self.notes,
            coverage: &self.coverage,
            cached: self.cached.as_deref(),
        })
        .expect("a report serializes")
    }

    pub fn text(&self) -> String {
        let mut out = String::from("Proposed dependencies:\n");
        for judged in &self.judged {
            out.push_str(&self.decision_lines(judged));
        }
        match &self.consequences {
            None => out.push_str(
                "\nGraph consequences not judged (`--no-graph`): cycles, reach chains, limits and diagram arrows the proposals cause are unknown.\n",
            ),
            Some(found) if found.is_empty() => out.push_str(
                "\nWhat they do to the graph: no new cycle, reach chain, crossed limit or missing diagram arrow.\n",
            ),
            Some(found) => {
                out.push_str("\nWhat they do to the graph:\n");
                for c in found {
                    out.push_str(&format!("  {} {}\n", c.message, tag(c.rule, c.warning)));
                    for step in &c.witness {
                        let at = match (step.proposal, &step.file, step.line) {
                            // A step of a reach chain is the proposal itself.
                            (Some(i), _, _) if self.proposals[i].is(&step.from, &step.to) => {
                                "proposed".to_string()
                            }
                            (Some(i), _, _) => format!("proposed: {}", self.proposals[i].label()),
                            (None, Some(file), Some(line)) => format!("{file}:{line}"),
                            _ => String::new(),
                        };
                        out.push_str(&format!("    {} -> {}: {at}\n", step.from, step.to));
                    }
                    if let Some(because) = &c.because {
                        out.push_str(&format!("    because: {because}\n"));
                    }
                }
            }
        }
        if !self.notes.is_empty() {
            out.push_str("\nNotes:\n");
            for note in &self.notes {
                out.push_str(&format!("  {note}\n"));
            }
        }
        let gaps = report::gap_lines(
            &self.coverage.evidence_gaps,
            &self.coverage.accepted_gaps,
            "a pass says nothing about this code",
            false,
            false,
        );
        if !gaps.is_empty() {
            out.push('\n');
            out.push_str(&gaps);
        }
        if let Some(cached) = &self.cached {
            out.push_str(&format!("\nCached: {cached}\n"));
        }
        let rejected = self.judged.iter().filter(|j| j.fails()).count();
        let failing = self
            .consequences
            .iter()
            .flatten()
            .filter(|c| !c.warning)
            .count();
        out.push_str(&format!(
            "\neasement: {} proposal(s), {rejected} rejected, {}{}\n",
            self.judged.len(),
            match &self.consequences {
                Some(_) => format!("{failing} failing graph consequence(s)"),
                None => "graph not judged".to_string(),
            },
            if self.coverage.confidence() == report::Confidence::Partial {
                " (partial)"
            } else {
                ""
            }
        ));
        out
    }

    /// The decision on one proposal, with what rejects it and why.
    fn decision_lines(&self, judged: &Judged) -> String {
        let mut out = format!(
            "  {} -> {}: {}\n",
            judged.from,
            judged.target(),
            judged.verdict()
        );
        if let Some(message) = &judged.message {
            out.push_str(&format!("    {message}\n"));
        }
        if !judged.public_surface.is_empty() {
            out.push_str(&format!(
                "    public surface of {}: {}\n",
                judged.target_component.as_deref().unwrap_or_default(),
                judged.public_surface.join(", ")
            ));
        }
        for surface in &judged.named_surfaces {
            out.push_str(&format!(
                "    {} surface of {}, open to {}: {}\n",
                surface.name,
                judged.target_component.as_deref().unwrap_or_default(),
                judged.component.as_deref().unwrap_or_default(),
                surface.paths.join(", ")
            ));
        }
        if let Some(because) = &judged.because {
            out.push_str(&format!("    because: {because}\n"));
        }
        out
    }
}

/// `[rule]`, or `[rule, warning]` for a rule in `warn`.
fn tag(rule: RuleId, warning: bool) -> String {
    if warning {
        format!("[{}, warning]", rule.as_str())
    } else {
        format!("[{}]", rule.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, file: &str, line: usize) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            file: file.into(),
            line,
        }
    }

    #[test]
    fn a_plan_lists_paths_and_external_modules() {
        let plan = plan(
            r#"[{"from": "./a.ts", "to": "b.ts"}, {"from": "a.ts", "external": "lodash"}]"#,
            |p| p.trim_start_matches("./").to_string(),
        )
        .unwrap();
        assert_eq!(
            plan,
            vec![
                Proposal {
                    from: "a.ts".into(),
                    to: To::Path("b.ts".into())
                },
                Proposal {
                    from: "a.ts".into(),
                    to: To::External("lodash".into())
                },
            ]
        );
        for wrong in [
            "{}",
            "[]",
            r#"[{"from": "a.ts"}]"#,
            r#"[{"from": "a", "to": "b", "x": 1}]"#,
        ] {
            assert!(super::plan(wrong, str::to_string).is_err(), "{wrong}");
        }
    }

    /// `a <-> b` exists; proposal 0 adds `c -> a`, proposals 1 and 2 add
    /// `a -> c` and `b -> c`. The witness is a shortest cycle through a
    /// proposal, at the import where the code has one; every proposal of the
    /// group is named.
    #[test]
    fn a_cycle_is_told_through_the_proposals_that_make_it() {
        let edges = vec![
            edge("a", "b", "a.ts", 3),
            edge("b", "a", "b.ts", 4),
            edge("c", "a", "c.ts", FIRST_LINE),
            edge("a", "c", "a.ts", FIRST_LINE + 1),
            edge("b", "c", "b.ts", FIRST_LINE + 2),
        ];
        let members: BTreeSet<&str> = ["a", "b", "c"].into();
        let witness = cycle_witness(&members, &edges);
        let told: Vec<(&str, &str, Option<usize>, Option<usize>)> = witness
            .iter()
            .map(|s| (s.from.as_str(), s.to.as_str(), s.line, s.proposal))
            .collect();
        assert_eq!(
            told,
            vec![("a", "c", None, Some(1)), ("c", "a", None, Some(0))]
        );
        assert_eq!(
            proposed_edges(&members, &edges),
            [("a", "c"), ("b", "c"), ("c", "a")].into()
        );
    }
}
