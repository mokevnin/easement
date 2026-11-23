use anstyle::{AnsiColor, Style};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use serde::Serialize;
use serde_sarif::sarif::{self, ResultLevel};
use std::collections::BTreeMap;

use crate::baseline::{Severity, Verdict};
use crate::check::{
    self, CannotRun, HonouredSuppression, Key, Place, RecognisedFramework, RuleId, Stats, Unjudged,
    Violation,
};
use crate::config::AcceptedKind;
use crate::diff::{Changes, Diff, InFull};
use crate::drift::Drift;
use crate::policy::Provenance;
use crate::suggest::Spared;

#[derive(Serialize)]
struct JsonReport<'a> {
    passed: bool,
    violations: &'a [Violation],
    /// Violations of the rules in `warn`, which do not fail the check.
    warnings: &'a [Violation],
    /// Who to ask about a violation, by the component in its `from`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    owners: &'a BTreeMap<String, String>,
    stale: &'a [Key],
    /// The inline suppressions that excused what was judged, each with the
    /// violations it excused.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    suppressions: &'a [HonouredSuppression],
    legalized: usize,
    #[serde(flatten)]
    coverage: &'a Coverage,
    /// The frameworks `unused` found in packages: it took the files each loads by
    /// convention as entries.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    frameworks: &'a [RecognisedFramework],
    /// Set when indexes came from the cache: what their key leaves out and how
    /// to index again.
    #[serde(skip_serializing_if = "Option::is_none")]
    cached: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<DiffInfo<'a>>,
    /// Under `--plan`, how the change departs from the plan.
    #[serde(skip_serializing_if = "Option::is_none", rename = "plan")]
    drift: Option<&'a Drift>,
    /// Under `--suggest`, the unused `may_depend_on` entries it does not fail on.
    #[serde(flatten)]
    spared: &'a Spared,
    stats: &'a Stats,
}

#[derive(Serialize)]
struct DiffInfo<'a> {
    base: &'a str,
    merge_base: &'a str,
    /// The policy inputs that differ from the merge-base: what the change to
    /// the policy causes over the same code is judged too.
    policy_changed: &'a [String],
    /// What was judged in full instead of by the change, with why; every rule
    /// when `keys` is absent.
    in_full: &'a [InFull],
}

/// What an answer leaves out: the part it was asked to judge, the code it could
/// not read or judge, and the gaps the policy accepts. A cache hit is none of them.
#[derive(Debug, Default, Serialize)]
pub struct Coverage {
    /// What the run was asked to judge, when not the whole project.
    pub narrowed: Vec<Narrowing>,
    /// Code or facts the run could not read or judge: a pass says nothing about them.
    pub evidence_gaps: Vec<EvidenceGap>,
    /// The gaps the entries of `accepted_gaps` accept, by entry.
    pub accepted_gaps: Vec<AcceptedGap>,
}

/// An entry of `accepted_gaps`, with the gaps of the run it accepts: those of its
/// `kind`, which an evidence gap of that kind would hold otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptedGap {
    /// The kind of the evidence gap it accepts.
    pub kind: AcceptedKind,
    /// Why the policy accepts it.
    pub because: String,
    /// Where the policy writes the entry.
    pub provenance: Provenance,
    /// The line, counted from 1, of the entry in the input of its provenance.
    pub line: usize,
    /// The files it accepts, for `uncovered`; the files of the imports it accepts,
    /// for `unresolved_imports`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// How many files, as the evidence gap of its kind counts them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// The places of the imports it accepts, for `unresolved_imports`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<Place>,
}

impl AcceptedGap {
    /// The entry as every format names it: `accepted_gaps[0] at easement.yml:12`.
    pub fn entry(&self) -> String {
        match &self.provenance.input {
            Some(input) => format!("{} at {input}:{}", self.provenance.field, self.line),
            None => self.provenance.field.clone(),
        }
    }

    /// What it accepts, as a phrase: `files no indexer read`.
    fn what(&self) -> &'static str {
        match self.kind {
            AcceptedKind::Uncovered => "files no indexer read",
            AcceptedKind::UnresolvedImports => "imports whose target is computed at run time",
        }
    }

    /// Where what it accepts is, after a key: `files: a.go, b.go`, or the
    /// places of the imports.
    fn where_(&self) -> String {
        match self.kind {
            AcceptedKind::Uncovered => format!("files: {}", self.files.join(", ")),
            AcceptedKind::UnresolvedImports => {
                let places: Vec<String> = self.places.iter().map(ToString::to_string).collect();
                format!("places: {}", places.join(", "))
            }
        }
    }

    /// What it accepts and why, in one line, for a note.
    fn message(&self) -> String {
        format!(
            "{} accepts {}, because {}; their dependencies were not checked; {}",
            self.entry(),
            self.what(),
            self.because,
            self.where_()
        )
    }
}

/// A part of the project or of a change that a narrowed verdict judges alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Narrowing {
    pub kind: NarrowingKind,
    /// The base of `--diff`, the component of `--component`, the path of `--scope`.
    pub limited_to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NarrowingKind {
    Diff,
    Component,
    Scope,
    /// `propose --no-graph`: the decisions alone, not what they do to the graph.
    NoGraph,
}

/// Code or facts that a check could not read or judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceGap {
    pub kind: GapKind,
    /// The files the gap is in, when they are known one by one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The extensions of the files the gap is in, when it holds for a whole language.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// How many files, when they are known one by one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// The places in those files the gap is at, when they are known one by one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<Place>,
    pub message: String,
    /// How to close the gap.
    pub close: String,
    /// Why the files were not indexed, when their indexer cannot run on this platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cannot_run: Option<CannotRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// Source files of an indexed language that no indexer covered.
    Uncovered,
    /// Files whose types `defines` cannot see.
    NamingUnseen,
    /// Files whose standard library `$std` cannot tell.
    StdUnknown,
    /// Imports that `manifests` could not judge.
    Manifests,
    /// Test code that could not be told from the rest of its file.
    Parts,
    /// Manifests, or dependencies of one, that `packages` could not judge.
    Packages,
    /// A name declared in more than one place, so an import of it could not be
    /// told for certain which declaration it means. Every reader of the
    /// dependency sees every copy, so the graph keeps them all.
    Declarations,
    /// Imports a file writes that the indexer did not resolve, so their
    /// dependencies are not in the graph.
    UnresolvedImports,
    /// Files whose symbols carry a scheme no language's entry declares, so none
    /// of them was read and their dependencies are not in the graph.
    UnknownScheme,
    /// Uses of names that `forbid_symbol` could not judge.
    Symbols,
    /// Types that `implementation` could not judge: their index declares no
    /// implementations, or none for what they are.
    Implementation,
    /// Files that `unused` took as used without judging them.
    Unused,
    /// Files a change deletes: what imported them is not in the code any more.
    Deleted,
    /// Files whose names of another component `why` does not count: easement reads
    /// no uses of names from their language.
    Names,
    /// Files a change deleted lines from outside every definition of the head index:
    /// a definition they held is gone, and so are its callers from `impact --diff`.
    RemovedDefinitions,
    /// Changed files whose language's index gives no definition ranges: `impact
    /// --diff` takes each as touched whole.
    NoDefinitionRanges,
}

impl GapKind {
    /// Whether the gap leaves dependencies out of the graph, so that an answer
    /// about it (`impact`, `why`) may be short; the rest leave a rule unjudged.
    pub fn hides_dependencies(self) -> bool {
        match self {
            GapKind::Uncovered
            | GapKind::Parts
            | GapKind::Packages
            | GapKind::UnresolvedImports
            | GapKind::UnknownScheme
            | GapKind::Deleted => true,
            GapKind::NamingUnseen
            | GapKind::StdUnknown
            | GapKind::Manifests
            | GapKind::Declarations
            | GapKind::Symbols
            | GapKind::Implementation
            | GapKind::Unused
            | GapKind::Names
            | GapKind::RemovedDefinitions
            | GapKind::NoDefinitionRanges => false,
        }
    }
}

impl Narrowing {
    /// The flag that asked for it, as it is written: `--diff main`, `--no-graph`.
    fn flag(&self) -> String {
        match self.kind {
            NarrowingKind::Diff => format!("--diff {}", self.limited_to),
            NarrowingKind::Component => format!("--component {}", self.limited_to),
            NarrowingKind::Scope => format!("--scope {}", self.limited_to),
            NarrowingKind::NoGraph => "--no-graph".to_string(),
        }
    }
}

/// The parts that `--diff`, `--component` and `--scope` narrow a check to.
pub fn narrowed(
    changes: Option<&Changes>,
    components: &[String],
    paths: &[String],
) -> Vec<Narrowing> {
    let item = |kind, limited_to: &String| Narrowing {
        kind,
        limited_to: limited_to.clone(),
    };
    changes
        .map(|c| item(NarrowingKind::Diff, &c.base))
        .into_iter()
        .chain(components.iter().map(|c| item(NarrowingKind::Component, c)))
        .chain(paths.iter().map(|p| item(NarrowingKind::Scope, p)))
        .collect()
}

/// The narrowing by `--component` and `--scope`, as `component web` and
/// `path src/api`: what `stats.limited_to` holds.
pub fn limited_to(narrowed: &[Narrowing]) -> Vec<String> {
    narrowed
        .iter()
        .filter_map(|n| match n.kind {
            NarrowingKind::Diff | NarrowingKind::NoGraph => None,
            NarrowingKind::Component => Some(format!("component {}", n.limited_to)),
            NarrowingKind::Scope => Some(format!("path {}", n.limited_to)),
        })
        .collect()
}

impl EvidenceGap {
    /// The gap of what a rule could not judge, with how to close it.
    pub fn of(kind: GapKind, unjudged: &Unjudged, close: &str) -> Self {
        EvidenceGap {
            kind,
            files: unjudged.files.clone(),
            extensions: unjudged.extensions.clone(),
            count: (!unjudged.files.is_empty()).then_some(unjudged.files.len()),
            places: unjudged.places.clone(),
            message: unjudged.reason.clone(),
            close: unjudged.close.as_deref().unwrap_or(close).to_string(),
            cannot_run: None,
        }
    }
}

pub const CLOSE_UNCOVERED: &str = "add them to a project the indexer reads, `exclude` them, or accept them in `accepted_gaps` (`kind: uncovered`) with the reason";

/// What closes the gap of imports the indexer did not resolve, unless their cause
/// says otherwise.
pub const CLOSE_UNRESOLVED: &str =
    "build the code and fix what the compiler reports, then check again";

/// What closes the gap of files whose indexer cannot run on this platform.
pub const CLOSE_CANNOT_RUN: &str = "check them on Linux or macOS (or WSL), or `exclude` them; `accepted_gaps` never accepts them, since the policy describes the code, not the machine";

/// Code a check could not read or judge, in the order the reasons were always given.
pub fn evidence_gaps(stats: &Stats) -> Vec<EvidenceGap> {
    evidence_gaps_unread(stats, &stats.uncovered, &stats.imports_gaps)
}

/// The evidence gaps of a run that leave dependencies out of its graph, for an
/// answer about the graph (`impact`, `why`, `suggest`, `graph`). `unread` is every file no indexer
/// read, accepted or not, and every unresolved import counts, accepted or not:
/// accepting a gap settles the verdict of `check`, but nobody saw its dependencies.
pub fn graph_gaps(stats: &Stats, unread: &[String]) -> Vec<EvidenceGap> {
    evidence_gaps_unread(stats, unread, &stats.graph_imports_gaps)
        .into_iter()
        .filter(|gap| gap.kind.hides_dependencies())
        .collect()
}

/// The evidence gaps of a run, with `unread` as the files no indexer read and
/// `imports` as the lines of the imports it did not resolve.
fn evidence_gaps_unread(
    stats: &Stats,
    unread: &[String],
    imports: &[Unjudged],
) -> Vec<EvidenceGap> {
    // A file easement could not read is told apart from one no indexer covered,
    // and one whose indexer cannot run here from both, since what closes the gap differs.
    let mut uncovered: Vec<&String> = unread
        .iter()
        .filter(|f| !stats.unreadable.contains(f))
        .collect();
    let mut cannot_run = Vec::new();
    for unavailable in &stats.unavailable {
        let files: Vec<&String> = uncovered
            .iter()
            .copied()
            .filter(|f| unavailable.files.contains(f))
            .collect();
        if files.is_empty() {
            continue;
        }
        uncovered.retain(|f| !files.contains(f));
        let CannotRun {
            language,
            indexer,
            platform,
            reason,
        } = unavailable.cannot_run;
        let mut gap = EvidenceGap::of(
            GapKind::Uncovered,
            &Unjudged::new(format!(
                "{} {language} source files were not indexed, since {indexer} cannot run on {}: {reason}; the dependencies in or on them were not checked",
                files.len(),
                platform.name()
            ))
            .in_files(files.into_iter().cloned()),
            CLOSE_CANNOT_RUN,
        );
        gap.cannot_run = Some(unavailable.cannot_run.clone());
        cannot_run.push(gap);
    }
    let mut gaps = Vec::new();
    let mut add = |kind, unjudged: &[Unjudged], close: &str| {
        gaps.extend(unjudged.iter().map(|u| EvidenceGap::of(kind, u, close)));
    };
    add(
        GapKind::Unused,
        &stats.unused_gaps,
        "the files it took as used may be dead: review them by hand, or have an indexer read them",
    );
    add(
        GapKind::Manifests,
        &stats.manifests_gaps,
        "fix or add the manifest the message names, or review those dependencies by hand",
    );
    add(
        GapKind::Parts,
        &stats.parts_gaps,
        "fix what the message names, or review that code by hand",
    );
    add(
        GapKind::Packages,
        &stats.packages_gaps,
        "fix the manifest the message names, or review those dependencies by hand",
    );
    add(
        GapKind::Declarations,
        &stats.declarations_gaps,
        "give the name one home: keep it in one component, or rename it so that it is unique",
    );
    add(GapKind::UnresolvedImports, imports, CLOSE_UNRESOLVED);
    add(
        GapKind::UnknownScheme,
        &stats.schemes_gaps,
        "the indexer wrote a scheme easement does not read: check its version with `easement doctor`, run one this easement reads, or report the scheme to easement",
    );
    add(
        GapKind::Symbols,
        &stats.symbols_gaps,
        "do what the message says, or review those uses by hand",
    );
    add(
        GapKind::Implementation,
        &stats.implementations_gaps,
        "review those types by hand, or move them into a component `allow_implementers` lists",
    );
    if !stats.naming_unseen.is_empty() {
        add(
            GapKind::NamingUnseen,
            &[Unjudged::new(format!(
                "`defines` cannot see the types of .{} files, which are read without an index, so their names were not checked",
                stats.naming_unseen.join(", .")
            ))
            .in_extensions(stats.naming_unseen.iter().cloned())],
            "no indexer reads these files, so review the names of their types by hand",
        );
    }
    if !stats.std_unknown.is_empty() {
        add(
            GapKind::StdUnknown,
            &[Unjudged::new(format!(
                "`$std` cannot tell the standard library of .{} files, so their dependencies on it count as other modules; name those modules instead",
                stats.std_unknown.join(", .")
            ))
            .in_extensions(stats.std_unknown.iter().cloned())],
            "name the standard modules the rule means instead of `$std`",
        );
    }
    if !uncovered.is_empty() {
        add(
            GapKind::Uncovered,
            &[Unjudged::new(format!(
                "{} source files were not covered by an indexer, so the dependencies in or on them were not checked",
                uncovered.len()
            ))
            .in_files(uncovered.into_iter().cloned())],
            CLOSE_UNCOVERED,
        );
    }
    if !stats.unreadable.is_empty() {
        add(
            GapKind::Uncovered,
            &[Unjudged::new(format!(
                "{} source files could not be read, so what they import, and what they suppress, was not checked in full",
                stats.unreadable.len()
            ))
            .in_files(stats.unreadable.iter().cloned())],
            "make the files readable (their permissions, a broken link), or `exclude` them",
        );
    }
    gaps.extend(cannot_run);
    gaps
}

impl Coverage {
    /// What a check over `stats` leaves out, when it was narrowed to `narrowed`.
    pub fn of_check(
        stats: &Stats,
        narrowed: Vec<Narrowing>,
        accepted_gaps: Vec<AcceptedGap>,
    ) -> Self {
        Coverage {
            narrowed,
            evidence_gaps: evidence_gaps(stats),
            accepted_gaps,
        }
    }

    /// What the answer leaves out, in one clause for a summary line: the
    /// narrowing by the flags that asked for it, then how many evidence gaps
    /// (`narrowed: --no-graph; 1 evidence gap`); empty when it leaves out
    /// nothing. Accepted gaps are said apart, with their reasons.
    pub fn summary(&self) -> String {
        let mut reasons = Vec::new();
        if !self.narrowed.is_empty() {
            let flags: Vec<String> = self.narrowed.iter().map(Narrowing::flag).collect();
            reasons.push(format!("narrowed: {}", flags.join(", ")));
        }
        match self.evidence_gaps.len() {
            0 => {}
            1 => reasons.push("1 evidence gap".to_string()),
            n => reasons.push(format!("{n} evidence gaps")),
        }
        reasons.join("; ")
    }

    /// One sentence for `--diff`, and one for `--component` and `--scope` together.
    fn narrowed_reasons(&self) -> Vec<String> {
        let mut reasons: Vec<String> = self
            .narrowed
            .iter()
            .filter(|n| n.kind == NarrowingKind::Diff)
            .map(|n| {
                format!(
                    "only violations on lines changed since {}, or caused by the change to the policy, were judged, the rest of the project was not",
                    n.limited_to
                )
            })
            .collect();
        if self
            .narrowed
            .iter()
            .any(|n| n.kind == NarrowingKind::NoGraph)
        {
            reasons.push(
                "only the dependency decision of each proposal was judged, not the cycles, reach chains, limits and diagram arrows they cause (`--no-graph`)".to_string(),
            );
        }
        let limited = limited_to(&self.narrowed);
        if !limited.is_empty() {
            reasons.push(format!(
                "the report is limited to {}, the rest of the project was not judged",
                limited.join(", ")
            ));
        }
        reasons
    }
}

/// What a diff run judges, as the lines that open its text report.
fn diff_header(diff: &Diff) -> String {
    let Changes {
        base, merge_base, ..
    } = &diff.changes;
    let short = &merge_base[..merge_base.len().min(12)];
    let attribution = &diff.attribution;
    let mut out = if attribution.whole() {
        format!(
            "Checking the whole project, not only the lines changed since {base} (merge-base {short}):\n"
        )
    } else if attribution.policy_changed.is_empty() {
        format!("Checking lines changed since {base} (merge-base {short})\n")
    } else {
        format!(
            "Checking lines changed since {base} (merge-base {short}), and what the change to the policy causes\n"
        )
    };
    for part in &attribution.in_full {
        if part.keys.is_empty() {
            out.push_str(&format!("  {}\n", part.reason));
        } else {
            out.push_str(&format!("Judged in full: {}\n", part.reason));
        }
    }
    out
}

/// What a verdict over cached indexes rests on. Neither a narrowing nor a gap:
/// the key holds what decides an index, but not dependencies outside the project.
pub fn cache_note(stats: &Stats) -> Option<String> {
    cache_note_of(stats.from_cache)
}

/// What a result resting on `from_cache` cached indexes rests on, if any.
pub fn cache_note_of(from_cache: usize) -> Option<String> {
    (from_cache > 0).then(|| {
        format!(
            "{from_cache} index(es) came from the cache, keyed on the project files, the indexer and its environment but not on dependencies outside the project; if those changed without a lock file, run with `--no-cache` to index again"
        )
    })
}

/// What a run of the check says: its verdict, what it saw, what it leaves out
/// and, in diff mode, the change it judged.
#[derive(Clone, Copy)]
pub struct Report<'a> {
    pub verdict: &'a Verdict,
    pub stats: &'a Stats,
    pub coverage: &'a Coverage,
    pub diff: Option<&'a Diff>,
    /// `--require-evidence`: an evidence gap fails a run without violations.
    pub require_evidence: bool,
    /// Under `--suggest`, the unused `may_depend_on` entries it does not fail on.
    pub spared: &'a Spared,
    /// Under `--plan`, how the change departs from the plan.
    pub drift: Option<&'a Drift>,
}

/// How many files a text list names before it counts the rest.
const FILES_SHOWN: usize = 20;

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const RED: Style = AnsiColor::Red.on_default().bold();
const GREEN: Style = AnsiColor::Green.on_default().bold();
const YELLOW: Style = AnsiColor::Yellow.on_default();

/// `text` wrapped in the escape codes of `style`, or as it is when `color` is off.
fn paint(color: bool, style: Style, text: &str) -> String {
    if color {
        format!("{style}{text}{style:#}")
    } else {
        text.to_string()
    }
}

/// The first files or places of a list, one per line under `indent`, and how many more.
pub(crate) fn item_lines<T: std::fmt::Display>(items: &[T], indent: &str) -> String {
    check::name_first(
        items,
        FILES_SHOWN,
        "",
        |item| format!("{indent}{item}\n"),
        |more| format!("{indent}… and {more} more (all of them in `--format json`)\n"),
    )
}

/// The evidence gaps, each with how to close it and its files, then the
/// accepted gaps, each with its reason and files: every command says them so.
/// `silent` says what the report cannot vouch for (`a pass says nothing about this
/// code`). Under `--require-evidence` the gaps fail the run, and their heading says so.
pub fn gap_lines(
    evidence_gaps: &[EvidenceGap],
    accepted_gaps: &[AcceptedGap],
    silent: &str,
    required: bool,
    color: bool,
) -> String {
    let mut out = String::new();
    if !evidence_gaps.is_empty() {
        out.push_str(&if required {
            paint(
                color,
                RED,
                "Evidence required (`--require-evidence`), but the check could not read or judge this code:",
            )
        } else {
            paint(color, YELLOW, &format!("Evidence gaps ({silent}):"))
        });
        out.push('\n');
        for gap in evidence_gaps {
            out.push_str(&format!("  {}\n", paint(color, YELLOW, &gap.message)));
            out.push_str(&paint(color, DIM, &format!("    to close: {}", gap.close)));
            out.push('\n');
            // The places say the files too, each at its line.
            out.push_str(&if gap.places.is_empty() {
                item_lines(&gap.files, "    ")
            } else {
                item_lines(&gap.places, "    ")
            });
        }
    }
    if !accepted_gaps.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("Accepted gaps (`accepted_gaps`: their dependencies were not checked):\n");
        for gap in accepted_gaps {
            out.push_str(&format!(
                "  {} accepts {}{}\n",
                gap.entry(),
                gap.what(),
                paint(color, DIM, &format!(" because: {}", gap.because))
            ));
            // The places say the files too, each at its line.
            out.push_str(&match gap.kind {
                AcceptedKind::Uncovered => item_lines(&gap.files, "    "),
                AcceptedKind::UnresolvedImports => item_lines(&gap.places, "    "),
            });
        }
    }
    out
}

/// The frameworks `unused` found, one line per package.
fn framework_lines(frameworks: &[RecognisedFramework]) -> String {
    let mut by_package: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for framework in frameworks {
        by_package
            .entry(&framework.package)
            .or_default()
            .push(format!("{} (`{}`)", framework.name, framework.dependency));
    }
    let mut out = String::from(
        "Frameworks `unused` recognised, by package (it took the files each loads by convention as entries, or every file of a package an evidence gap names):\n",
    );
    for (package, names) in by_package {
        out.push_str(&format!("  {package}: {}\n", names.join(", ")));
    }
    out
}

/// `file:line: message [rule]`, and the reason under it when there is one.
pub(crate) fn violation_lines(
    v: &Violation,
    owner: Option<&str>,
    tag: Style,
    color: bool,
) -> String {
    let mut out = format!(
        "{}: {} {}\n",
        paint(color, BOLD, &format!("{}:{}", v.file, v.line)),
        v.message,
        paint(color, tag, &format!("[{}]", v.rule.as_str()))
    );
    if let Some(because) = &v.because {
        out.push_str(&paint(color, DIM, &format!("  because: {because}")));
        out.push('\n');
    }
    if let Some(owner) = owner {
        out.push_str(&paint(color, DIM, &format!("  owner: {owner}")));
        out.push('\n');
    }
    out
}

impl Report<'_> {
    /// Whether `--require-evidence` fails the run: it has an evidence gap.
    /// Violations and stale entries fail it first.
    pub fn evidence_required(self) -> bool {
        self.require_evidence && !self.coverage.evidence_gaps.is_empty()
    }

    /// Whether the run passes: no violations, no stale entries, and the
    /// evidence `--require-evidence` asks for.
    pub fn passed(self) -> bool {
        self.verdict.passed() && !self.evidence_required()
    }

    /// The report as data.
    pub fn json(self) -> String {
        let Report {
            verdict,
            stats,
            coverage,
            diff,
            require_evidence: _,
            spared,
            drift,
        } = self;
        let report = JsonReport {
            passed: self.passed(),
            violations: &verdict.new,
            warnings: &verdict.warnings,
            owners: &verdict.owners,
            stale: &verdict.stale,
            suppressions: &verdict.suppressions,
            legalized: verdict.legalized,
            coverage,
            frameworks: &stats.frameworks,
            cached: cache_note(stats),
            diff: diff.map(|d| DiffInfo {
                base: &d.changes.base,
                merge_base: &d.changes.merge_base,
                policy_changed: &d.attribution.policy_changed,
                in_full: &d.attribution.in_full,
            }),
            drift,
            spared,
            stats,
        };
        serde_json::to_string_pretty(&report).expect("report serializes")
    }

    /// The plain text report.
    pub fn text(self) -> String {
        self.render(false)
    }

    /// The text report with ANSI styles; the caller strips them where the output
    /// is not a terminal.
    pub fn colored(self) -> String {
        self.render(true)
    }

    fn render(self, color: bool) -> String {
        let Report {
            verdict,
            stats,
            coverage,
            diff,
            require_evidence: _,
            spared,
            drift,
        } = self;
        let mut out = String::new();
        if let Some(d) = diff {
            out.push_str(&diff_header(d));
            out.push('\n');
        }
        for v in &verdict.new {
            out.push_str(&violation_lines(
                v,
                verdict.owner_of(v).as_deref(),
                RED,
                color,
            ));
        }
        if !verdict.warnings.is_empty() {
            let mut body = paint(
                color,
                YELLOW,
                "Warnings (rules in `warn`; do not add more):",
            );
            body.push('\n');
            for v in &verdict.warnings {
                body.push_str(&violation_lines(
                    v,
                    verdict.owner_of(v).as_deref(),
                    YELLOW,
                    color,
                ));
            }
            section(&mut out, &body);
        }
        if !verdict.stale.is_empty() {
            let mut body = paint(
                color,
                YELLOW,
                "Stale baseline entries (fixed, run `easement baseline` to shrink it):",
            );
            body.push('\n');
            for k in &verdict.stale {
                let what = stale_description(k);
                body.push_str(&format!("  [{}] {what}\n", k.rule.as_str()));
            }
            section(&mut out, &body);
        }
        if !verdict.suppressions.is_empty() {
            // Judged in full, the list is the whole project's, not the change's.
            let title = if diff.is_some_and(|d| !d.attribution.whole()) {
                "Inline suppressions in this change:"
            } else {
                "Inline suppressions:"
            };
            section(
                &mut out,
                &suppression_lines(title, &verdict.suppressions, color),
            );
        }
        if !stats.new_dependencies.is_empty() {
            let mut body = String::from("New dependencies between components in this change:\n");
            for d in &stats.new_dependencies {
                body.push_str(&format!("  {d}\n"));
            }
            section(&mut out, &body);
        }
        if let Some(drift) = drift {
            section(&mut out, &drift.text());
        }
        if !stats.frameworks.is_empty() {
            section(&mut out, &framework_lines(&stats.frameworks));
        }
        let spared = spared.text();
        if !spared.is_empty() {
            section(&mut out, &spared);
        }
        let gaps = gap_lines(
            &coverage.evidence_gaps,
            &coverage.accepted_gaps,
            "a pass says nothing about this code",
            self.evidence_required(),
            color,
        );
        if !gaps.is_empty() {
            section(&mut out, &gaps);
        }
        if let Some(note) = cache_note(stats) {
            section(&mut out, &format!("{note}\n"));
        }
        section(&mut out, &self.summary(color));
        out.push('\n');
        out
    }

    /// The last line of the text report: the verdict and its counts.
    fn summary(self, color: bool) -> String {
        let Report { verdict, stats, .. } = self;
        let status = if self.passed() {
            paint(color, GREEN, "passed")
        } else {
            paint(color, RED, "failed")
        };
        let warnings = if verdict.warnings.is_empty() {
            String::new()
        } else {
            format!(", {} warning(s)", verdict.warnings.len())
        };
        let mut out = format!(
            "easement {status}: {} violation(s), {} stale, {} legalized{warnings}; {} files, {} internal deps",
            verdict.new.len(),
            verdict.stale.len(),
            verdict.legalized,
            stats.files,
            stats.internal_deps,
        );
        if !stats.limited_to.is_empty() {
            out.push_str(&format!(", limited to {}", stats.limited_to.join(", ")));
        }
        let suppressed = check::excused(&verdict.suppressions).count();
        if suppressed > 0 {
            out.push_str(&format!(", {suppressed} suppressed"));
        }
        if !stats.uncovered.is_empty() {
            out.push_str(&format!(
                ", {} files not covered by an indexer",
                stats.uncovered.len()
            ));
        }
        if stats.unmapped_files > 0 {
            out.push_str(&format!(
                ", {} files outside components",
                stats.unmapped_files
            ));
        }
        out
    }
}

/// Each suppression under `title` as `file:line [rules] reason`, with the
/// violations it excused under it.
fn suppression_lines(title: &str, suppressions: &[HonouredSuppression], color: bool) -> String {
    let mut out = format!("{title}\n");
    for s in suppressions {
        out.push_str(&format!(
            "  {} [{}] {}\n",
            paint(color, BOLD, &format!("{}:{}", s.file, s.line)),
            s.rules.join(", "),
            s.reason
        ));
        for v in &s.excused {
            for line in violation_lines(v, None, DIM, color).lines() {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    out
}

/// What a suppression excused, in one line.
fn suppression_message(s: &HonouredSuppression) -> String {
    let excused: Vec<&str> = s.excused.iter().map(|v| v.message.as_str()).collect();
    format!(
        "{} suppressed ({}): {}",
        s.rules.join(", "),
        s.reason,
        excused.join("; ")
    )
}

/// Appends a section of the text report, a blank line before it unless it is the first.
fn section(out: &mut String, body: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(body);
}

fn stale_description(k: &Key) -> String {
    match k.rule {
        RuleId::Diagram => format!("{} -> {} is now in the diagram or gone", k.from, k.to),
        _ => key_description(k),
    }
}

/// What the violation a baseline entry legalizes is, in one line.
pub fn key_description(k: &Key) -> String {
    match k.rule {
        RuleId::Cycle => format!("cycle among {}", k.from),
        RuleId::Metric => format!("{} exceeds {}", k.from, k.target),
        RuleId::Reach => format!("{} reaches {}", k.from, k.to),
        RuleId::Diagram => format!("{} -> {} is not in the diagram", k.from, k.to),
        RuleId::External => format!(
            "{}: {} -> external {}",
            k.file,
            crate::check::who(&k.from),
            k.target
        ),
        RuleId::Dependency | RuleId::Privacy => {
            format!("{}: {} -> {} ({})", k.file, k.from, k.to, k.target)
        }
        _ => format!("{}: {}", k.file, k.target),
    }
}

fn stale_message(k: &Key) -> String {
    format!(
        "stale baseline entry, fixed debt: {} (run `easement baseline` to shrink it)",
        stale_description(k)
    )
}

/// The kind of a note a run makes of itself beside its results. Every format
/// names it the same way: the GitHub annotation title (`easement evidence-gap`),
/// the SARIF notification descriptor, the prefix in JUnit's `<system-out>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NoteKind {
    Narrowed,
    EvidenceGap,
    AcceptedGap,
    UnusedAllowance,
    Cache,
}

impl NoteKind {
    fn id(self) -> &'static str {
        match self {
            NoteKind::Narrowed => "narrowed",
            NoteKind::EvidenceGap => "evidence-gap",
            NoteKind::AcceptedGap => "accepted-gap",
            NoteKind::UnusedAllowance => "unused-allowance",
            NoteKind::Cache => "cache",
        }
    }

    /// The title of its GitHub annotation, and the description of its SARIF descriptor.
    fn title(self) -> String {
        format!("easement {}", self.id())
    }
}

/// A note a run makes of itself: its kind, its level, its text and the files it names.
struct Note<'a> {
    kind: NoteKind,
    level: ResultLevel,
    text: String,
    files: &'a [String],
    /// Its line in JUnit's `<system-out>`, which opens with its kind.
    out: String,
}

impl<'a> Note<'a> {
    fn new(kind: NoteKind, level: ResultLevel, text: String, files: &'a [String]) -> Self {
        let out = format!("{}: {text}", kind.id().replace('-', " "));
        Note {
            kind,
            level,
            text,
            files,
            out,
        }
    }

    /// An entry of `accepted_gaps`: where it is written, its reason and its files.
    fn accepted(gap: &'a AcceptedGap) -> Self {
        Note {
            out: format!(
                "accepted gap ({}) because {}; {}",
                gap.entry(),
                gap.because,
                gap.where_()
            ),
            ..Note::new(
                NoteKind::AcceptedGap,
                ResultLevel::Note,
                gap.message(),
                &gap.files,
            )
        }
    }
}

impl Report<'_> {
    /// What a run says of itself beside its results, in the order the text gives
    /// it: the narrowing, the evidence gaps (errors under `--require-evidence`),
    /// the accepted gaps, the unused allowances and the cache.
    fn notes(&self) -> Vec<Note<'_>> {
        let mut notes: Vec<Note<'_>> = self
            .coverage
            .narrowed_reasons()
            .into_iter()
            .map(|reason| Note::new(NoteKind::Narrowed, ResultLevel::Note, reason, &[]))
            .collect();
        let level = gap_level(self.require_evidence);
        for gap in &self.coverage.evidence_gaps {
            let mut text = format!("{}; to close: {}", gap.message, gap.close);
            if !gap.files.is_empty() {
                text.push_str(&format!("; files: {}", gap.files.join(", ")));
            }
            notes.push(Note::new(NoteKind::EvidenceGap, level, text, &gap.files));
        }
        notes.extend(self.coverage.accepted_gaps.iter().map(Note::accepted));
        notes.extend(
            self.spared
                .notes()
                .into_iter()
                .map(|note| Note::new(NoteKind::UnusedAllowance, ResultLevel::Note, note, &[])),
        );
        if let Some(note) = cache_note(self.stats) {
            notes.push(Note::new(NoteKind::Cache, ResultLevel::Note, note, &[]));
        }
        notes
    }

    /// SARIF 2.1.0, the format GitHub Code Scanning ingests: the violations as
    /// results, and what the run leaves out as notifications of its invocation.
    pub fn sarif(&self) -> String {
        sarif(self.verdict, self.notes())
    }

    /// JUnit XML: one failed test case per violation, and under `--require-evidence`
    /// per evidence gap; what the run leaves out also in `<system-out>`, each
    /// note opening with its kind. An accepted gap is never a failed test case.
    pub fn junit(&self) -> String {
        let gaps: Vec<(String, String)> = if self.require_evidence {
            self.coverage
                .evidence_gaps
                .iter()
                .map(|gap| (gap.message.clone(), gap.close.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let out: Vec<String> = self.notes().into_iter().map(|note| note.out).collect();
        junit(self.verdict, &gaps, &out)
    }
}

/// SARIF 2.1.0 of a verdict, with what the run says of itself: each note a
/// notification whose descriptor is its kind, each kind declared once.
fn sarif(verdict: &Verdict, notes: Vec<Note<'_>>) -> String {
    let mut kinds: Vec<NoteKind> = notes.iter().map(|note| note.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let declared: Vec<_> = kinds
        .into_iter()
        .map(|kind| {
            sarif::ReportingDescriptor::builder()
                .id(kind.id())
                .short_description(
                    sarif::MultiformatMessageString::builder()
                        .text(kind.title())
                        .build(),
                )
                .build()
        })
        .collect();
    let notifications: Vec<_> = notes
        .into_iter()
        .map(|note| {
            sarif::Notification::builder()
                .level(serde_json::Value::from(note.level.to_string()))
                .message(sarif_message(note.text))
                .descriptor(
                    sarif::ReportingDescriptorReference::builder()
                        .id(note.kind.id())
                        .build(),
                )
                .locations(
                    note.files
                        .iter()
                        .map(|file| sarif_location(file, None))
                        .collect::<Vec<_>>(),
                )
                .build()
        })
        .collect();
    let mut ids: Vec<&str> = verdict
        .new
        .iter()
        .chain(&verdict.warnings)
        .chain(check::excused(&verdict.suppressions).map(|(_, v)| v))
        .map(|v| v.rule.as_str())
        .collect();
    if !verdict.stale.is_empty() {
        ids.push(STALE);
    }
    ids.sort_unstable();
    ids.dedup();
    let rules: Vec<_> = ids
        .iter()
        .map(|id| {
            sarif::ReportingDescriptor::builder()
                .id(*id)
                .short_description(
                    sarif::MultiformatMessageString::builder()
                        .text(format!("easement {id}"))
                        .build(),
                )
                .build()
        })
        .collect();
    let mut results: Vec<_> = verdict
        .reported()
        .map(|(severity, v)| {
            sarif::Result::builder()
                .rule_id(v.rule.as_str())
                .level(ResultLevel::from(severity))
                .message(sarif_message(message_with_reason(v)))
                .locations(vec![sarif_location(&v.file, Some(v.line))])
                .build()
        })
        .collect();
    // An excused violation is a result Code Scanning shows as suppressed.
    results.extend(check::excused(&verdict.suppressions).map(|(s, v)| {
        sarif::Result::builder()
            .rule_id(v.rule.as_str())
            .level(ResultLevel::from(verdict.excused_severity(v)))
            .message(sarif_message(message_with_reason(v)))
            .locations(vec![sarif_location(&v.file, Some(v.line))])
            .suppressions(vec![
                sarif::Suppression::builder()
                    .kind(serde_json::Value::from("inSource"))
                    .justification(s.reason.clone())
                    .location(sarif_location(&s.file, Some(s.line)))
                    .build(),
            ])
            .build()
    }));
    results.extend(verdict.stale.iter().map(|k| {
        sarif::Result::builder()
            .rule_id(STALE)
            .level(ResultLevel::Error)
            .message(sarif_message(stale_message(k)))
            .build()
    }));
    let mut driver = sarif::ToolComponent::builder()
        .name("easement")
        .version(env!("CARGO_PKG_VERSION"))
        .information_uri(env!("CARGO_PKG_HOMEPAGE"))
        .rules(rules)
        .build();
    // A run that says nothing of itself declares no kind.
    driver.notifications = (!declared.is_empty()).then_some(declared);
    let invocation = sarif::Invocation::builder()
        .execution_successful(true)
        .tool_execution_notifications(notifications)
        .build();
    let run = sarif::Run::builder()
        .tool(sarif::Tool::builder().driver(driver).build())
        .results(results)
        .invocations(vec![invocation])
        .build();
    let report = sarif::Sarif::builder()
        .schema("https://json.schemastore.org/sarif-2.1.0.json")
        .version("2.1.0")
        .runs(vec![run])
        .build();
    serde_json::to_string_pretty(&report).expect("report serializes")
}

fn sarif_message(text: String) -> sarif::Message {
    sarif::Message::builder().text(text).build()
}

/// Where a result or a notification is: the file, and the line when there is one.
fn sarif_location(file: &str, line: Option<usize>) -> sarif::Location {
    let artifact = sarif::ArtifactLocation::builder().uri(file).build();
    let physical = match line {
        Some(line) => sarif::PhysicalLocation::builder()
            .artifact_location(artifact)
            .region(
                sarif::Region::builder()
                    .start_line(i64::try_from(line).expect("a line fits in i64"))
                    .build(),
            )
            .build(),
        None => sarif::PhysicalLocation::builder()
            .artifact_location(artifact)
            .build(),
    };
    sarif::Location::builder()
        .physical_location(physical)
        .build()
}

impl From<Severity> for ResultLevel {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Error => ResultLevel::Error,
            Severity::Warning => ResultLevel::Warning,
        }
    }
}

/// The level of an evidence gap: an error when the run requires evidence.
fn gap_level(require_evidence: bool) -> ResultLevel {
    if require_evidence {
        ResultLevel::Error
    } else {
        ResultLevel::Warning
    }
}

/// The GitHub workflow command that annotates at `level`.
fn workflow_command(level: ResultLevel) -> &'static str {
    match level {
        ResultLevel::Error => "error",
        ResultLevel::Warning => "warning",
        ResultLevel::Note | ResultLevel::None => "notice",
    }
}

/// The message of a violation, with its reason in parentheses when there is one.
fn message_with_reason(v: &Violation) -> String {
    match &v.because {
        Some(because) => format!("{} (because: {because})", v.message),
        None => v.message.clone(),
    }
}

fn escape_command(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn escape_property(text: &str) -> String {
    escape_command(text).replace(':', "%3A").replace(',', "%2C")
}

impl Report<'_> {
    /// GitHub Actions workflow commands: annotations on the changed files.
    pub fn github(self) -> String {
        let Report {
            verdict,
            stats,
            coverage,
            diff,
            require_evidence: _,
            spared,
            drift,
        } = self;
        let mut out = String::new();
        for (severity, v) in verdict.reported() {
            let mut text = message_with_reason(v);
            if let Some(owner) = verdict.owner_of(v) {
                text.push_str(&format!(" (owner: {owner})"));
            }
            out.push_str(&format!(
                "::{} file={},line={},title={}::{}\n",
                workflow_command(severity.into()),
                escape_property(&v.file),
                v.line,
                escape_property(&format!("easement {}", v.rule.as_str())),
                escape_command(&text)
            ));
        }
        for k in &verdict.stale {
            out.push_str(&format!(
                "::error title={}::{}\n",
                escape_property(&format!("easement {STALE}")),
                escape_command(&stale_message(k))
            ));
        }
        for s in &verdict.suppressions {
            out.push_str(&format!(
                "::notice file={},line={},title={}::{}\n",
                escape_property(&s.file),
                s.line,
                escape_property("easement suppression"),
                escape_command(&suppression_message(s))
            ));
        }
        for note in spared.notes() {
            out.push_str(&format!(
                "::notice title={}::{}\n",
                escape_property(&NoteKind::UnusedAllowance.title()),
                escape_command(&note)
            ));
        }
        if !stats.new_dependencies.is_empty() {
            out.push_str(&format!(
                "::notice title={}::{}\n",
                escape_property("easement graph"),
                escape_command(&format!(
                    "New component dependencies in this change: {}",
                    stats.new_dependencies.join(", ")
                ))
            ));
        }
        for (place, note) in drift.iter().flat_map(|d| d.notes()) {
            let place = place.map_or(String::new(), |(file, line)| {
                format!("file={},line={line},", escape_property(file))
            });
            out.push_str(&format!(
                "::notice {place}title={}::{}\n",
                escape_property("easement plan"),
                escape_command(&note)
            ));
        }
        let narrowed = coverage
            .narrowed_reasons()
            .into_iter()
            .map(|reason| (NoteKind::Narrowed, ResultLevel::Note, reason));
        for part in diff.iter().flat_map(|d| &d.attribution.in_full) {
            out.push_str(&format!(
                "::notice title={}::{}\n",
                escape_property("easement diff"),
                escape_command(&format!("Judged in full: {}", part.reason))
            ));
        }
        let level = gap_level(self.evidence_required());
        let gaps = coverage
            .evidence_gaps
            .iter()
            .map(|gap| (NoteKind::EvidenceGap, level, gap.message.clone()));
        // One notice per entry of `accepted_gaps`, not one per file it accepts.
        let accepted = coverage
            .accepted_gaps
            .iter()
            .map(|gap| (NoteKind::AcceptedGap, ResultLevel::Note, gap.message()));
        for (kind, level, reason) in narrowed.chain(gaps).chain(accepted) {
            out.push_str(&format!(
                "::{} title={}::{}\n",
                workflow_command(level),
                escape_property(&kind.title()),
                escape_command(&reason)
            ));
        }
        if let Some(note) = cache_note(stats) {
            out.push_str(&format!(
                "::notice title={}::{}\n",
                escape_property(&NoteKind::Cache.title()),
                escape_command(&note)
            ));
        }
        out.push_str(&self.summary(false));
        out.push('\n');
        out
    }
}

/// A test case of the JUnit report: a pass, or a failure of a kind.
struct Case {
    name: String,
    classname: String,
    failure: Option<Failure>,
}

struct Failure {
    kind: String,
    message: String,
    body: String,
}

/// A failed test case of `kind`: its message, and the text of the failure.
fn failed(kind: &str, name: String, message: String, body: String) -> Case {
    Case {
        name,
        classname: format!("easement.{kind}"),
        failure: Some(Failure {
            kind: kind.to_string(),
            message,
            body,
        }),
    }
}

/// JUnit XML of a verdict: one failed test case per violation and per gap in
/// `gaps` (message, how to close it), with `out` as what the run says of itself.
fn junit(verdict: &Verdict, gaps: &[(String, String)], out_lines: &[String]) -> String {
    let mut cases: Vec<Case> = Vec::new();
    for v in &verdict.new {
        let mut body = v.message.clone();
        if let Some(because) = &v.because {
            body.push_str(&format!("\nbecause: {because}"));
        }
        cases.push(failed(
            v.rule.as_str(),
            format!("{}:{}", v.file, v.line),
            v.message.clone(),
            body,
        ));
    }
    for k in &verdict.stale {
        let message = stale_message(k);
        cases.push(failed(
            STALE,
            stale_description(k),
            message.clone(),
            message,
        ));
    }
    for (message, close) in gaps {
        cases.push(failed(
            "evidence",
            message.clone(),
            message.clone(),
            format!("{message}\nto close: {close}"),
        ));
    }
    if cases.is_empty() {
        cases.push(Case {
            name: "architecture".to_string(),
            classname: "easement".to_string(),
            failure: None,
        });
    }
    let out = (!out_lines.is_empty()).then(|| out_lines.join("\n"));
    let mut xml = Writer::new_with_indent(Vec::new(), b' ', 4);
    write_junit(&mut xml, &cases, out.as_deref()).expect("XML is written to memory");
    String::from_utf8(xml.into_inner()).expect("XML is written from strings")
}

/// The suite of `cases` in one report, then `out` as its standard output.
fn write_junit(
    xml: &mut Writer<Vec<u8>>,
    cases: &[Case],
    out: Option<&str>,
) -> std::io::Result<()> {
    let tests = cases.len().to_string();
    let failures = cases
        .iter()
        .filter(|case| case.failure.is_some())
        .count()
        .to_string();
    xml.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    let mut suites = BytesStart::new("testsuites");
    suites.extend_attributes([
        ("name", "easement"),
        ("tests", &tests),
        ("skipped", "0"),
        ("failures", &failures),
        ("errors", "0"),
    ]);
    xml.write_event(Event::Start(suites))?;
    let mut suite = BytesStart::new("testsuite");
    suite.extend_attributes([
        ("name", "easement"),
        ("tests", &tests),
        ("skipped", "0"),
        ("errors", "0"),
        ("failures", &failures),
    ]);
    xml.write_event(Event::Start(suite))?;
    for case in cases {
        let mut tag = BytesStart::new("testcase");
        tag.extend_attributes([
            ("name", xml_text(&case.name).as_str()),
            ("classname", xml_text(&case.classname).as_str()),
        ]);
        let Some(failure) = &case.failure else {
            xml.write_event(Event::Empty(tag))?;
            continue;
        };
        xml.write_event(Event::Start(tag))?;
        let mut tag = BytesStart::new("failure");
        tag.extend_attributes([
            ("message", xml_text(&failure.message).as_str()),
            ("type", xml_text(&failure.kind).as_str()),
        ]);
        xml.write_event(Event::Start(tag))?;
        xml.write_event(Event::Text(BytesText::new(&xml_text(&failure.body))))?;
        xml.write_event(Event::End(BytesEnd::new("failure")))?;
        xml.write_event(Event::End(BytesEnd::new("testcase")))?;
    }
    if let Some(out) = out {
        xml.write_event(Event::Start(BytesStart::new("system-out")))?;
        xml.write_event(Event::Text(BytesText::new(&xml_text(out))))?;
        xml.write_event(Event::End(BytesEnd::new("system-out")))?;
    }
    xml.write_event(Event::End(BytesEnd::new("testsuite")))?;
    xml.write_event(Event::End(BytesEnd::new("testsuites")))?;
    // The report ends with a newline.
    xml.write_indent()
}

/// `text` as XML can hold it: control characters but the newline are dropped.
fn xml_text(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

const STALE: &str = "stale-baseline";

#[cfg(test)]
mod tests {
    use super::*;

    /// A run without `--suggest`, which spares no allowance.
    static NO_SPARED: Spared = Spared {
        unreliable: Vec::new(),
        held: Vec::new(),
    };

    #[test]
    fn an_external_module_of_a_file_of_no_component_is_described_as_unowned_files() {
        let key = |from: &str| Key {
            rule: RuleId::External,
            file: "scripts/seed.ts".to_string(),
            target: "dayjs".to_string(),
            from: from.to_string(),
            to: String::new(),
        };
        assert_eq!(
            key_description(&key("")),
            "scripts/seed.ts: unowned files -> external dayjs"
        );
        assert_eq!(
            key_description(&key("web")),
            "scripts/seed.ts: web -> external dayjs"
        );
    }

    #[test]
    fn a_graph_answer_passes_the_gaps_that_hide_dependencies() {
        let stats = Stats {
            uncovered: vec!["a.go".to_string()],
            parts_gaps: vec![Unjudged::new(
                "a `mod` file is not in the project".to_string(),
            )],
            symbols_gaps: vec![Unjudged::new("`forbid_symbol` over kotlin".to_string())],
            std_unknown: vec!["rb".to_string()],
            ..Stats::default()
        };
        let unread = ["a.go".to_string(), "accepted.go".to_string()];
        let gaps = graph_gaps(&stats, &unread);
        let kinds: Vec<GapKind> = gaps.iter().map(|gap| gap.kind).collect();
        assert_eq!(kinds, [GapKind::Parts, GapKind::Uncovered]);
        assert_eq!(gaps[1].files, unread);
        assert_eq!(evidence_gaps(&stats).len(), 4);
    }

    /// A gap that knows its places lists them as `` file:line (`specifier`) ``
    /// instead of its files, cut where the list of files is.
    #[test]
    fn a_gap_with_places_lists_them_cut_like_files() {
        let places = (1..=FILES_SHOWN + 2).map(|line| Place {
            file: "a/a.go".to_string(),
            line,
            specifier: Some("example.com/b".to_string()),
        });
        let unjudged = Unjudged::new("the indexer did not resolve".to_string())
            .in_files(["a/a.go"])
            .at(places);
        let gap = EvidenceGap::of(GapKind::UnresolvedImports, &unjudged, "build it");
        let said = gap_lines(&[gap], &[], "unchecked", false, false);
        let listed: Vec<&str> = said.lines().skip(3).collect();
        assert_eq!(listed.len(), FILES_SHOWN + 1, "{said}");
        assert_eq!(listed[0], "    a/a.go:1 (`example.com/b`)");
        assert_eq!(
            listed[FILES_SHOWN - 1],
            format!("    a/a.go:{FILES_SHOWN} (`example.com/b`)")
        );
        assert_eq!(
            listed[FILES_SHOWN],
            "    … and 2 more (all of them in `--format json`)"
        );
        assert!(!said.contains("    a/a.go\n"), "{said}");
    }

    use crate::baseline::Verdict;

    fn verdict() -> Verdict {
        Verdict {
            new: vec![Violation {
                file: "src/a.ts".into(),
                line: 3,
                message: "a -> b is not allowed <x> 100%".into(),
                because: Some("layers".into()),
                from: "a".into(),
                to: "b".into(),
                target: "src/b.ts".into(),
                specifier: "../b".into(),
                ..Violation::of(RuleId::Dependency)
            }],
            warnings: vec![],
            owners: BTreeMap::new(),
            stale: vec![Key {
                rule: RuleId::Metric,
                file: "f".into(),
                target: "max_fan_out".into(),
                from: "core".into(),
                to: String::new(),
            }],
            legalized: 0,
            suppressions: vec![],
            warned: Default::default(),
        }
    }

    #[test]
    fn sarif_has_located_results() {
        let report: serde_json::Value =
            serde_json::from_str(&sarif(&verdict(), Vec::new())).unwrap();
        assert_eq!(report["version"], "2.1.0");
        let results = report["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["ruleId"], "dependency");
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            3
        );
        assert_eq!(results[1]["ruleId"], "stale-baseline");
        assert_eq!(
            report["runs"][0]["tool"]["driver"]["rules"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // Nothing said of the run, no kind of note declared.
        assert!(
            report["runs"][0]["tool"]["driver"]
                .get("notifications")
                .is_none()
        );
    }

    #[test]
    fn github_annotations_are_escaped() {
        let out = Report {
            verdict: &verdict(),
            stats: &Stats::default(),
            coverage: &Coverage::default(),
            diff: None,
            drift: None,
            require_evidence: false,
            spared: &NO_SPARED,
        }
        .github();
        assert!(out.starts_with(
            "::error file=src/a.ts,line=3,title=easement dependency::a -> b is not allowed <x> 100%25 (because: layers)\n"
        ));
        assert!(out.contains("::error title=easement stale-baseline::"));
        assert!(out.contains("easement failed"));
    }

    #[test]
    fn junit_counts_failures_and_escapes() {
        let xml = junit(&verdict(), &[], &[]);
        assert!(xml.contains(
            "<testsuite name=\"easement\" tests=\"2\" skipped=\"0\" errors=\"0\" failures=\"2\">"
        ));
        assert!(xml.contains("&lt;x&gt;"));
        let clean = junit(
            &Verdict {
                new: vec![],
                warnings: vec![],
                owners: BTreeMap::new(),
                stale: vec![],
                legalized: 0,
                suppressions: vec![],
                warned: Default::default(),
            },
            &[],
            &[],
        );
        assert!(clean.contains("tests=\"1\" skipped=\"0\" errors=\"0\" failures=\"0\""));
        assert!(clean.contains("<testcase name=\"architecture\" classname=\"easement\"/>"));
    }

    /// Control characters but the newline are dropped, as XML cannot hold most of
    /// them, and the rest is escaped in attributes and in text alike; in an
    /// attribute the newline is a reference, which a reader keeps a newline.
    #[test]
    fn junit_escapes_what_xml_reserves_and_drops_what_it_cannot_hold() {
        let awkward = "a & b < c > d \"e\" 'f'\tg\nh\u{1}i";
        let xml = junit(
            &Verdict {
                new: vec![Violation {
                    file: "src/a.ts".into(),
                    line: 3,
                    message: awkward.into(),
                    because: Some(awkward.into()),
                    ..Violation::of(RuleId::Dependency)
                }],
                warnings: vec![],
                owners: BTreeMap::new(),
                stale: vec![],
                legalized: 0,
                suppressions: vec![],
                warned: Default::default(),
            },
            &[],
            &[awkward.to_string()],
        );
        assert_eq!(
            xml,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<testsuites name=\"easement\" tests=\"1\" skipped=\"0\" failures=\"1\" errors=\"0\">
    <testsuite name=\"easement\" tests=\"1\" skipped=\"0\" errors=\"0\" failures=\"1\">
        <testcase name=\"src/a.ts:3\" classname=\"easement.dependency\">
            <failure message=\"a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos;g&#10;hi\" type=\"dependency\">a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos;g
hi
because: a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos;g
hi</failure>
        </testcase>
        <system-out>a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos;g
hi</system-out>
    </testsuite>
</testsuites>
"
        );
    }

    #[test]
    fn a_cache_hit_is_named_with_how_to_index_again() {
        let stats = Stats {
            indexed: 1,
            from_cache: 2,
            ..Stats::default()
        };
        let report: serde_json::Value = serde_json::from_str(
            &Report {
                verdict: &verdict(),
                stats: &stats,
                coverage: &Coverage::default(),
                diff: None,
                drift: None,
                require_evidence: false,
                spared: &NO_SPARED,
            }
            .json(),
        )
        .unwrap();
        let cached = report["cached"].as_str().unwrap();
        assert!(
            cached.contains("2 index(es)") && cached.contains("--no-cache"),
            "{cached}"
        );
        let out = Report {
            verdict: &verdict(),
            stats: &stats,
            coverage: &Coverage::default(),
            diff: None,
            drift: None,
            require_evidence: false,
            spared: &NO_SPARED,
        }
        .text();
        assert!(out.contains(cached), "{out}");

        let fresh: serde_json::Value = serde_json::from_str(
            &Report {
                verdict: &verdict(),
                stats: &Stats::default(),
                coverage: &Coverage::default(),
                diff: None,
                drift: None,
                require_evidence: false,
                spared: &NO_SPARED,
            }
            .json(),
        )
        .unwrap();
        assert!(fresh.get("cached").is_none());
        assert!(
            !Report {
                verdict: &verdict(),
                stats: &Stats::default(),
                coverage: &Coverage::default(),
                diff: None,
                drift: None,
                require_evidence: false,
                spared: &NO_SPARED,
            }
            .text()
            .contains("--no-cache")
        );
    }

    /// The `index`th entry of `accepted_gaps`, at `line` of easement.yml, accepting
    /// `files` as generated.
    fn accepted_gap(index: usize, line: usize, files: &[&str]) -> AcceptedGap {
        AcceptedGap {
            kind: AcceptedKind::Uncovered,
            because: "generated".into(),
            provenance: Provenance {
                field: format!("accepted_gaps[{index}]"),
                input: Some("easement.yml".into()),
                through: Vec::new(),
            },
            line,
            files: files.iter().map(|f| f.to_string()).collect(),
            count: Some(files.len()),
            places: Vec::new(),
        }
    }

    #[test]
    fn an_accepted_gap_leaves_the_verdict_full_and_is_still_reported() {
        let accepted = vec![accepted_gap(0, 12, &["gen/b.go", "tools/a.go"])];
        let coverage = Coverage {
            accepted_gaps: accepted,
            ..Coverage::default()
        };
        assert!(coverage.narrowed_reasons().is_empty());
        assert!(coverage.summary().is_empty());
        let (verdict, stats) = (verdict(), Stats::default());
        let report = |require_evidence| Report {
            verdict: &verdict,
            stats: &stats,
            coverage: &coverage,
            diff: None,
            drift: None,
            require_evidence,
            spared: &NO_SPARED,
        };
        let out = report(false).text();
        assert!(
            out.contains(
                "  accepted_gaps[0] at easement.yml:12 accepts files no indexer read because: generated\n    gen/b.go\n    tools/a.go\n"
            ),
            "{out}"
        );
        // One notice per entry, with where it is written, its reason and its files.
        let github = report(true).github();
        assert_eq!(
            github
                .lines()
                .filter(|l| l.contains("title=easement accepted-gap"))
                .collect::<Vec<_>>(),
            [
                "::notice title=easement accepted-gap::accepted_gaps[0] at easement.yml:12 accepts files no indexer read, because generated; their dependencies were not checked; files: gen/b.go, tools/a.go"
            ],
            "{github}"
        );
    }

    /// An entry that accepts imports is named alike, with the places it accepts.
    #[test]
    fn an_accepted_import_is_named_with_its_places_in_every_format() {
        let mut gap = accepted_gap(0, 5, &["src/i18n.ts"]);
        gap.kind = AcceptedKind::UnresolvedImports;
        gap.places = vec![Place {
            file: "src/i18n.ts".into(),
            line: 1,
            specifier: Some("./locales/*.json".into()),
        }];
        let coverage = Coverage {
            accepted_gaps: vec![gap],
            ..Coverage::default()
        };
        let (verdict, stats) = (clean_verdict(), Stats::default());
        let report = Report {
            verdict: &verdict,
            stats: &stats,
            coverage: &coverage,
            diff: None,
            drift: None,
            require_evidence: true,
            spared: &NO_SPARED,
        };
        let said = "accepted_gaps[0] at easement.yml:5 accepts imports whose target is computed at run time, because generated; their dependencies were not checked; places: src/i18n.ts:1 (`./locales/*.json`)";
        assert!(report.github().contains(said), "{}", report.github());
        let sarif: serde_json::Value = serde_json::from_str(&report.sarif()).unwrap();
        let notes = &sarif["runs"][0]["invocations"][0]["toolExecutionNotifications"];
        assert_eq!(notes[0]["message"]["text"], said, "{notes}");
        let xml = report.junit();
        assert!(
            xml.contains("accepted gap (accepted_gaps[0] at easement.yml:5) because generated; places: src/i18n.ts:1 (`./locales/*.json`)"),
            "{xml}"
        );
        assert!(xml.contains("failures=\"0\""), "{xml}");
        let text = report.text();
        assert!(
            text.contains("  accepted_gaps[0] at easement.yml:5 accepts imports whose target is computed at run time because: generated\n    src/i18n.ts:1 (`./locales/*.json`)\n"),
            "{text}"
        );
    }

    /// An accepted gap is a note in SARIF and in JUnit's `<system-out>`, never a
    /// failed test case, with `--require-evidence` or not.
    #[test]
    fn sarif_and_junit_say_an_accepted_gap_as_a_note() {
        let coverage = Coverage {
            accepted_gaps: vec![accepted_gap(1, 7, &["tools/a.go"])],
            ..Coverage::default()
        };
        let clean = clean_verdict();
        for require_evidence in [false, true] {
            let report = Report {
                verdict: &clean,
                stats: &Stats::default(),
                coverage: &coverage,
                diff: None,
                drift: None,
                require_evidence,
                spared: &NO_SPARED,
            };
            let sarif: serde_json::Value = serde_json::from_str(&report.sarif()).unwrap();
            let run = &sarif["runs"][0];
            let said = &run["invocations"][0]["toolExecutionNotifications"];
            assert_eq!(said.as_array().unwrap().len(), 1, "{said}");
            assert_eq!(said[0]["level"], "note");
            assert_eq!(said[0]["descriptor"]["id"], "accepted-gap");
            assert_eq!(
                said[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
                "tools/a.go"
            );
            assert_eq!(
                run["tool"]["driver"]["notifications"],
                serde_json::json!([{
                    "id": "accepted-gap",
                    "shortDescription": { "text": "easement accepted-gap" },
                }])
            );
            let xml = report.junit();
            assert!(xml.contains("failures=\"0\""), "{xml}");
            assert!(
                xml.contains(
                    "<system-out>accepted gap (accepted_gaps[1] at easement.yml:7) because generated; files: tools/a.go</system-out>"
                ),
                "{xml}"
            );
        }
    }

    /// `propose` sums up what its answer leaves out by kind, never as `partial`.
    #[test]
    fn the_summary_names_the_narrowing_and_counts_the_evidence_gaps() {
        let gap = |message: &str| {
            EvidenceGap::of(GapKind::StdUnknown, &Unjudged::new(message.into()), "close")
        };
        let no_graph = Narrowing {
            kind: NarrowingKind::NoGraph,
            limited_to: "dependency decisions".into(),
        };
        let narrowed = Coverage {
            narrowed: vec![no_graph.clone()],
            ..Coverage::default()
        };
        assert_eq!(narrowed.summary(), "narrowed: --no-graph");
        let gaps = Coverage {
            evidence_gaps: vec![gap("a"), gap("b")],
            ..Coverage::default()
        };
        assert_eq!(gaps.summary(), "2 evidence gaps");
        let both = Coverage {
            narrowed: vec![
                no_graph,
                Narrowing {
                    kind: NarrowingKind::Diff,
                    limited_to: "main".into(),
                },
                Narrowing {
                    kind: NarrowingKind::Component,
                    limited_to: "web".into(),
                },
                Narrowing {
                    kind: NarrowingKind::Scope,
                    limited_to: "src/api".into(),
                },
            ],
            evidence_gaps: vec![gap("a")],
            ..Coverage::default()
        };
        assert_eq!(
            both.summary(),
            "narrowed: --no-graph, --diff main, --component web, --scope src/api; 1 evidence gap"
        );
    }

    fn clean_verdict() -> Verdict {
        Verdict {
            new: vec![],
            warnings: vec![],
            owners: BTreeMap::new(),
            stale: vec![],
            legalized: 0,
            suppressions: vec![],
            warned: Default::default(),
        }
    }

    #[test]
    fn narrowing_by_component_and_scope_is_one_reason_after_the_diff() {
        let coverage = Coverage {
            narrowed: vec![
                Narrowing {
                    kind: NarrowingKind::Diff,
                    limited_to: "main".into(),
                },
                Narrowing {
                    kind: NarrowingKind::Component,
                    limited_to: "db".into(),
                },
                Narrowing {
                    kind: NarrowingKind::Scope,
                    limited_to: "internal".into(),
                },
            ],
            ..Coverage::default()
        };
        assert_eq!(
            coverage.narrowed_reasons(),
            [
                "only violations on lines changed since main, or caused by the change to the policy, were judged, the rest of the project was not",
                "the report is limited to component db, path internal, the rest of the project was not judged",
            ]
        );
    }

    #[test]
    fn require_evidence_turns_the_gaps_of_a_clean_run_into_errors() {
        let clean = Verdict {
            new: vec![],
            warnings: vec![],
            owners: BTreeMap::new(),
            stale: vec![],
            legalized: 0,
            suppressions: vec![],
            warned: Default::default(),
        };
        let stats = Stats {
            uncovered: vec!["tools/gen.go".into()],
            ..Stats::default()
        };
        let coverage = Coverage::of_check(&stats, vec![], vec![]);
        let report = |require_evidence| Report {
            verdict: &clean,
            stats: &stats,
            coverage: &coverage,
            diff: None,
            drift: None,
            require_evidence,
            spared: &NO_SPARED,
        };
        assert!(report(false).passed());
        let github = report(false).github();
        assert!(
            github.contains("::warning title=easement evidence-gap::"),
            "{github}"
        );
        assert!(github.contains("easement passed"), "{github}");

        assert!(!report(true).passed());
        let github = report(true).github();
        assert!(
            github.contains("::error title=easement evidence-gap::"),
            "{github}"
        );
        assert!(github.contains("easement failed"), "{github}");
        // Nothing to require of a run that reads everything.
        let whole = Coverage::default();
        assert!(
            Report {
                coverage: &whole,
                ..report(true)
            }
            .passed()
        );
    }

    /// SARIF and JUnit say what a run leaves out too: a clean result over code no
    /// indexer read is not a clean bill.
    #[test]
    fn sarif_and_junit_say_the_gaps_the_narrowing_and_the_cache() {
        let stats = Stats {
            from_cache: 1,
            uncovered: vec!["tools/gen.go".into()],
            ..Stats::default()
        };
        let coverage = Coverage {
            narrowed: super::narrowed(None, &["web".into()], &[]),
            ..Coverage::of_check(&stats, vec![], vec![])
        };
        let clean = Verdict {
            new: vec![],
            warnings: vec![],
            owners: BTreeMap::new(),
            stale: vec![],
            legalized: 0,
            suppressions: vec![],
            warned: Default::default(),
        };
        let report = |require_evidence| Report {
            verdict: &clean,
            stats: &stats,
            coverage: &coverage,
            diff: None,
            drift: None,
            require_evidence,
            spared: &NO_SPARED,
        };
        let sarif: serde_json::Value = serde_json::from_str(&report(false).sarif()).unwrap();
        let said: Vec<(String, String)> =
            sarif["runs"][0]["invocations"][0]["toolExecutionNotifications"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| {
                    (
                        n["level"].as_str().unwrap().to_string(),
                        n["message"]["text"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
        assert_eq!(said.len(), 3, "{said:?}");
        assert_eq!(said[0].0, "note");
        assert!(said[0].1.contains("limited to component web"), "{said:?}");
        assert_eq!(said[1].0, "warning");
        assert!(said[1].1.contains("tools/gen.go"), "{said:?}");
        assert!(said[2].1.contains("--no-cache"), "{said:?}");
        // The kind of each note tells a narrowing from a gap, not only the level.
        let kinds: Vec<&str> = sarif["runs"][0]["invocations"][0]["toolExecutionNotifications"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["descriptor"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["narrowed", "evidence-gap", "cache"]);
        let declared: Vec<&str> = sarif["runs"][0]["tool"]["driver"]["notifications"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert_eq!(declared, ["narrowed", "evidence-gap", "cache"]);
        let required: serde_json::Value = serde_json::from_str(&report(true).sarif()).unwrap();
        assert_eq!(
            required["runs"][0]["invocations"][0]["toolExecutionNotifications"][1]["level"],
            "error"
        );

        let xml = report(false).junit();
        assert!(
            xml.contains("<system-out>narrowed: the report is limited to component web"),
            "{xml}"
        );
        assert!(
            xml.contains(
                "\nevidence gap: 1 source files were not covered by an indexer, so the dependencies in or on them were not checked; to close: "
            ),
            "{xml}"
        );
        assert!(xml.contains("; files: tools/gen.go\n"), "{xml}");
        assert!(
            xml.contains("\ncache: 1 index(es) came from the cache"),
            "{xml}"
        );
        assert!(xml.contains("failures=\"0\""), "{xml}");
        // Under `--require-evidence` a gap fails the run, so it is a failed test case.
        let xml = report(true).junit();
        assert!(xml.contains("failures=\"1\""), "{xml}");
        assert!(xml.contains("classname=\"easement.evidence\""), "{xml}");
    }

    #[test]
    fn github_names_the_narrowing_the_gaps_and_the_cache_apart() {
        let stats = Stats {
            from_cache: 1,
            uncovered: vec!["tools/gen.go".into()],
            ..Stats::default()
        };
        let out = Report {
            verdict: &verdict(),
            stats: &stats,
            coverage: &Coverage::of_check(&stats, vec![], vec![]),
            diff: None,
            drift: None,
            require_evidence: false,
            spared: &NO_SPARED,
        }
        .github();
        assert!(
            out.contains(
                "::warning title=easement evidence-gap::1 source files were not covered by an indexer"
            ),
            "{out}"
        );
        assert!(out.contains("::notice title=easement cache::1 index(es) came from the cache"));
        assert!(out.contains("--no-cache"));
        assert!(
            out.ends_with("1 files not covered by an indexer\n"),
            "{out}"
        );
        assert!(
            !Report {
                verdict: &verdict(),
                stats: &Stats::default(),
                coverage: &Coverage::default(),
                diff: None,
                drift: None,
                require_evidence: false,
                spared: &NO_SPARED,
            }
            .github()
            .contains("::warning")
        );

        // A run narrowed by design is a notice: a warning on every pull request
        // would teach to skip the ones about code nobody read.
        let narrowed = Coverage {
            narrowed: super::narrowed(None, &["web".into()], &[]),
            ..Coverage::default()
        };
        let out = Report {
            verdict: &verdict(),
            stats: &Stats::default(),
            coverage: &narrowed,
            diff: None,
            drift: None,
            require_evidence: false,
            spared: &NO_SPARED,
        }
        .github();
        assert!(
            out.contains(
                "::notice title=easement narrowed::the report is limited to component web"
            ),
            "{out}"
        );
        assert!(!out.contains("::warning"), "{out}");
    }

    /// Files whose symbols no entry reads leave their dependencies out of the
    /// graph: an `unknown_scheme` gap, which `impact` and `why` keep.
    #[test]
    fn a_scheme_no_entry_reads_is_a_gap_that_hides_dependencies() {
        let stats = Stats {
            schemes_gaps: vec![
                Unjudged::new("1 files hold symbols of the scheme `scip-nowhere`".to_string())
                    .in_files(["src/a.ts".to_string()]),
            ],
            ..Stats::default()
        };
        let gaps = graph_gaps(&stats, &[]);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, GapKind::UnknownScheme);
        assert_eq!(gaps[0].files, ["src/a.ts"]);
        assert!(gaps[0].close.contains("easement doctor"));
        assert_eq!(
            serde_json::to_value(gaps[0].kind).unwrap(),
            serde_json::json!("unknown_scheme")
        );
    }
}
