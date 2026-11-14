//! The dependency decision: the one place where the policy judges a dependency.
//! `check` turns rejected decisions into violations, `why`, `explain` and
//! `suggest` render the verdict on a pair of components.

use serde::Serialize;

use crate::check::RuleId;
use crate::components::{ComponentMap, NamedSurface};
use crate::config::Config;
use crate::lang::Target;
use crate::lang::Unit;
use crate::lang::{Dep, Written};
use crate::policy::Site;
use crate::relpath::slash;

/// What permits one component to depend on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    SameComponent,
    /// The `tests` component of `tests: auto`, which no rule restricts.
    Tests,
    MayDependOn,
    Common,
    /// A lower layer, with no closed layer between.
    Layers,
}

impl Grant {
    /// How the policy names the grant.
    pub fn as_str(self) -> &'static str {
        match self {
            Grant::SameComponent => "same component",
            Grant::Tests => "tests: auto",
            Grant::MayDependOn => "may_depend_on",
            Grant::Common => "common",
            Grant::Layers => "layers",
        }
    }
}

/// Why no grant lets one component depend on another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A prohibition: `must_not_reach` when `reach`, else `must_not_depend_on`.
    Prohibited { reach: bool },
    /// Nothing grants it; `layers` says why they do not, when both are layers.
    NotGranted { layers: Option<String> },
}

/// A clause of the policy that rejects a dependency, as reports name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseId {
    MustNotReach,
    MustNotDependOn,
    NotGranted,
    AllowImporters,
    Surface,
    Slice,
    Isolated,
    Exclude,
    External,
}

impl ClauseId {
    pub fn as_str(self) -> &'static str {
        match self {
            ClauseId::MustNotReach => "must_not_reach",
            ClauseId::MustNotDependOn => "must_not_depend_on",
            ClauseId::NotGranted => "not_granted",
            ClauseId::AllowImporters => "allow_importers",
            ClauseId::Surface => "surface",
            ClauseId::Slice => "slice",
            ClauseId::Isolated => "isolated",
            ClauseId::Exclude => "exclude",
            ClauseId::External => "external",
        }
    }

    /// Whether a prohibition of the policy rejects it.
    pub fn prohibits(self) -> bool {
        matches!(self, ClauseId::MustNotReach | ClauseId::MustNotDependOn)
    }
}

impl Refusal {
    /// How the policy names the refusal, as reports give it.
    pub fn id(&self) -> ClauseId {
        match self {
            Refusal::Prohibited { reach: true } => ClauseId::MustNotReach,
            Refusal::Prohibited { reach: false } => ClauseId::MustNotDependOn,
            Refusal::NotGranted { .. } => ClauseId::NotGranted,
        }
    }
}

/// The clause of the policy that rejects a dependency.
#[derive(Debug, Clone)]
pub enum Clause<'a> {
    Refused(Refusal),
    /// `allow_importers` of the target does not list the source.
    ImporterNotListed(&'a [String]),
    /// The file by which the dependency enters the target's component, the target
    /// itself or a module that re-exports or aliases a name of it, is outside the
    /// public surface and in no named surface that lists the source: that file, and
    /// the named surfaces that hold it.
    OutsideSurface {
        entry: String,
        held: Vec<&'a NamedSurface>,
    },
    /// Both ends fill the placeholder `name` of their `in` patterns differently.
    SliceMismatch {
        name: String,
        here: String,
        there: String,
    },
    /// Two instances of an isolated component.
    OtherInstance {
        here: String,
        there: String,
    },
    /// `exclude` hides the target, so no rule could judge it.
    ExcludedTarget,
    /// An external module the rules of the source do not let it use: what is wrong with it.
    External(&'static str),
}

impl Clause<'_> {
    /// How the policy names the clause, as reports give it.
    pub fn id(&self) -> ClauseId {
        match self {
            Clause::Refused(refusal) => refusal.id(),
            Clause::ImporterNotListed(_) => ClauseId::AllowImporters,
            Clause::OutsideSurface { .. } => ClauseId::Surface,
            Clause::SliceMismatch { .. } => ClauseId::Slice,
            Clause::OtherInstance { .. } => ClauseId::Isolated,
            Clause::ExcludedTarget => ClauseId::Exclude,
            Clause::External(_) => ClauseId::External,
        }
    }

    /// The rule a dependency rejected by this clause violates.
    pub fn rule(&self) -> RuleId {
        match self {
            Clause::Refused(_) | Clause::ImporterNotListed(_) => RuleId::Dependency,
            Clause::OutsideSurface { .. } => RuleId::Privacy,
            Clause::SliceMismatch { .. } | Clause::OtherInstance { .. } => RuleId::Isolation,
            Clause::ExcludedTarget => RuleId::Excluded,
            Clause::External(_) => RuleId::External,
        }
    }
}

/// A named surface open to the source of a dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSurface<'a> {
    pub name: &'a str,
    pub paths: Vec<&'a str>,
}

/// Why a source is outside what the policy judges: no dependency of it is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outside {
    /// `exclude` hides the source.
    Excluded,
    /// `target_only` keeps the source as a target alone; `exclude` wins over it.
    TargetOnly,
    /// No component owns the source.
    Unowned,
}

/// The verdict on one dependency.
#[derive(Debug)]
pub struct Decision<'a> {
    /// The component of the importing code.
    pub from: &'a str,
    /// The component of the target: none for an external module, a target
    /// `exclude` hides, or one that belongs to no component.
    pub to: Option<&'a str>,
    /// The target path, or the external module.
    pub target: String,
    /// The grant that allows it, or the clause that rejects it. Allowed with no
    /// grant when nothing in the policy restricts it: an allowed external
    /// module, a target in no component.
    pub grant: Result<Option<Grant>, Clause<'a>>,
    /// How certainly the target was resolved.
    pub resolution: ResolutionStatus,
    /// All files or directories that may declare the imported name.
    pub candidates: Vec<String>,
    /// The reason the policy gives for the clause that rejected it.
    pub because: Option<&'a str>,
}

/// What the policy says of every dependency of one component on another,
/// before the paths of a dependency are known.
#[derive(Debug)]
pub struct PairDecision<'a> {
    pub grant: Result<Grant, Refusal>,
    /// The reason given with the grant, or the one behind the refusal.
    pub because: Option<&'a str>,
    /// `allow_importers` of the target, when it does not list the source.
    pub importers: Option<&'a [String]>,
    /// The public surface of the target, when the target is another component
    /// that declares `public` or `surfaces`: empty when it declares only `surfaces`.
    pub surface: Option<Vec<&'a str>>,
    /// The named surfaces of the target that list the source.
    pub open_named: Vec<OpenSurface<'a>>,
    /// Placeholders both components' `in` patterns name: a dependency must fill them alike.
    pub slices: Vec<&'a str>,
    /// Whether the pair is one isolated component, whose instances must not import each other.
    pub isolated: bool,
}

impl PairDecision<'_> {
    /// Whether the pair is rejected whatever paths the dependency has.
    pub fn rejected(&self) -> bool {
        self.grant.is_err() || self.importers.is_some()
    }

    /// What the verdict leaves to the paths of each dependency on `to`, as a reader says it.
    pub fn conditions(&self, to: &str) -> Vec<String> {
        let mut found = Vec::new();
        if let Some(public) = &self.surface {
            let mut ways = Vec::new();
            if !public.is_empty() {
                ways.push(format!("the public surface of {to}: {}", public.join(", ")));
            }
            for OpenSurface { name, paths } in &self.open_named {
                let whose = if ways.is_empty() {
                    format!("{to}'s")
                } else {
                    "its".to_string()
                };
                ways.push(format!("{whose} {name} surface: {}", paths.join(", ")));
            }
            found.push(if ways.is_empty() {
                format!("through no path: {to} opens no surface to it")
            } else {
                format!("only through {}", ways.join("; or "))
            });
        }
        for name in &self.slices {
            found.push(format!("only where both fill <{name}> alike"));
        }
        if self.isolated {
            found.push(format!("instances of {to} must not depend on each other"));
        }
        found
    }
}

/// The verdict on `from` depending on `to`.
pub fn pair<'a>(
    config: &'a Config,
    map: &'a ComponentMap,
    from: &str,
    to: &str,
) -> PairDecision<'a> {
    let grant = match config.grant(from, to) {
        Some(grant) => Ok(grant),
        None if config.forbids(from, to) => Err(Refusal::Prohibited {
            reach: config.forbids_reach(from, to),
        }),
        None => Err(Refusal::NotGranted {
            layers: config.layer_block(from, to),
        }),
    };
    let because = match grant {
        Ok(_) => config.allow_reason(from, to),
        Err(_) => config
            .forbid_reason(from, to)
            .or_else(|| config.reason(from)),
    };
    let target = &config.components[to];
    let other = from != to;
    let ours = map.placeholders(from);
    PairDecision {
        grant,
        because,
        importers: target
            .allow_importers
            .as_deref()
            .filter(|list| other && !list.iter().any(|c| c == from)),
        surface: target.public_surface().filter(|_| other),
        open_named: target
            .surfaces
            .iter()
            .filter(|(_, s)| other && s.importers.iter().any(|c| c == from))
            .map(|(name, s)| OpenSurface {
                name,
                paths: s.patterns.as_vec(),
            })
            .collect(),
        slices: map
            .placeholders(to)
            .into_iter()
            .filter(|name| ours.contains(name))
            .collect(),
        isolated: !other && target.isolated,
    }
}

/// Why the scan reads no import of the file `rel_path`: `exclude` hides it,
/// or `target_only` keeps it as a target alone.
pub fn unread(map: &ComponentMap, rel_path: &str) -> Option<Outside> {
    if map.is_excluded(rel_path) {
        Some(Outside::Excluded)
    } else if map.is_target_only(rel_path) {
        Some(Outside::TargetOnly)
    } else {
        None
    }
}

/// How certainly a dependency's target was resolved. It says nothing of whether
/// the declaration is a duplicate: an import that means every copy (a wildcard,
/// an overload) still tells how certain its target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    /// A single declaration: the target is proven.
    Unambiguous,
    /// Several declarations in one component (or all outside components): the
    /// component is known, the exact file is not.
    SameScope,
    /// Declarations in different components: the target is not proven.
    Ambiguous,
}

/// The verdict on one dependency, or why no rule judges the imports of its
/// source. The scan reads no import of an [`unread`] file, so `check` meets
/// only the unowned ones; a proposed dependency meets them all.
pub fn decide<'a>(
    config: &'a Config,
    map: &'a ComponentMap,
    dep: &Dep,
) -> Result<Decision<'a>, Outside> {
    let file = slash(&dep.file);
    if let Some(why) = unread(map, &file) {
        return Err(why);
    }
    let from = map.component_of_unit(&dep.from).ok_or(Outside::Unowned)?;
    let resolution_status = if dep.candidates.is_empty() {
        ResolutionStatus::Unambiguous
    } else {
        let scopes: std::collections::BTreeSet<Option<&str>> =
            dep.candidates.iter().map(|c| map.component_of(c)).collect();
        if scopes.len() <= 1 {
            ResolutionStatus::SameScope
        } else {
            ResolutionStatus::Ambiguous
        }
    };
    let decision = |to, target, grant, because, resolution, candidates| Decision {
        from,
        to,
        target,
        grant,
        because,
        resolution,
        candidates,
    };
    let unit = match &dep.target {
        Target::External(module) => {
            return Ok(match map.external_problem(from, &file, module) {
                Some(problem) => decision(
                    None,
                    module.clone(),
                    Err(Clause::External(problem.what)),
                    problem.because.or_else(|| config.reason(from)),
                    resolution_status,
                    dep.candidates.clone(),
                ),
                None => decision(
                    None,
                    module.clone(),
                    Ok(None),
                    None,
                    resolution_status,
                    Vec::new(),
                ),
            });
        }
        Target::Internal(unit) => unit.clone(),
    };
    let unit = if !dep.candidates.is_empty() {
        let here = map.component_of(slash(&dep.file).as_str());
        // Which copy an import means is not decided by the policy: take the copy
        // in the importing file's component, otherwise the first by path. A copy
        // `exclude` hides is not skipped, so the import fails as `excluded`
        // rather than being taken for the copy that breaks no rule.
        let own = dep
            .candidates
            .iter()
            .map(String::as_str)
            .filter(|c| here.is_some() && map.component_of(c) == here)
            .min();
        if let Some(chosen) = own {
            Unit::from(chosen)
        } else {
            dep.candidates
                .iter()
                .min()
                .map(|c| Unit::from(c.as_str()))
                .unwrap_or(unit)
        }
    } else {
        unit
    };
    let target = unit.key();
    if map.is_excluded(&target) {
        return Ok(decision(
            None,
            target,
            Err(Clause::ExcludedTarget),
            None,
            resolution_status,
            dep.candidates.clone(),
        ));
    }
    let Some(to) = map.component_of_unit(&unit) else {
        return Ok(decision(
            None,
            target,
            Ok(None),
            None,
            resolution_status,
            dep.candidates.clone(),
        ));
    };
    let component = |name: &str| config.components[name].because.as_deref();
    let sliced = |clause| (Err(clause), component(to).or_else(|| config.reason(from)));
    let (grant, because) = if from == to {
        match (map.instance_of(&file), map.instance_of(&target)) {
            (Some(here), Some(there)) if here != there => (
                Err(Clause::OtherInstance { here, there }),
                component(from).or_else(|| config.reason(from)),
            ),
            _ => slice_mismatch(map, &file, &target)
                .map_or((Ok(Some(Grant::SameComponent)), None), sliced),
        }
    } else {
        let verdict = pair(config, map, from, to);
        if let Err(refusal) = &verdict.grant {
            (Err(Clause::Refused(refusal.clone())), verdict.because)
        } else if let Some(list) = verdict.importers {
            (Err(Clause::ImporterNotListed(list)), component(to))
        } else if let Some((entry, (held, because))) = closed_entry(map, from, to, dep, &target) {
            (
                Err(Clause::OutsideSurface { entry, held }),
                because.or_else(|| component(to)),
            )
        } else if let Some(clause) = slice_mismatch(map, &file, &target) {
            sliced(clause)
        } else {
            (Ok(verdict.grant.ok()), verdict.because)
        }
    };
    Ok(decision(
        Some(to),
        target,
        grant,
        because,
        resolution_status,
        dep.candidates.clone(),
    ))
}

/// Where the policy writes the grant that lets `from` depend on `to`: none
/// for the same component, which no clause grants.
pub fn grant_site(config: &Config, from: &str, to: &str, grant: Grant) -> Option<Site> {
    match grant {
        Grant::SameComponent => None,
        Grant::Tests => Some(Site::of_key("tests")),
        Grant::MayDependOn => listed_site(config, from, "may_depend_on", to),
        Grant::Common => config
            .common
            .iter()
            .position(|c| c == to)
            .map(|at| Site::of_common(to, at)),
        Grant::Layers => Some(Site::of_key("layers")),
    }
}

/// Where the policy writes what refuses `from` a dependency on `to`: the
/// prohibition, or the `layers` that keep them apart; none when nothing
/// grants it.
pub fn refusal_site(config: &Config, from: &str, to: &str, refusal: &Refusal) -> Option<Site> {
    match refusal {
        Refusal::Prohibited { reach: true } => listed_site(config, from, "must_not_reach", to),
        Refusal::Prohibited { reach: false } => listed_site(config, from, "must_not_depend_on", to),
        Refusal::NotGranted { layers: Some(_) } => Some(Site::of_key("layers")),
        Refusal::NotGranted { layers: None } => None,
    }
}

/// The item that names `to` in the list `field` of the rule of `from`.
fn listed_site(config: &Config, from: &str, field: &str, to: &str) -> Option<Site> {
    let rule = config.rules.get(from)?;
    let list = match field {
        "may_depend_on" => &rule.may_depend_on,
        "must_not_depend_on" => &rule.must_not_depend_on,
        "must_not_reach" => &rule.must_not_reach,
        _ => return None,
    };
    let at = list.iter().position(|l| l.name() == to)?;
    Some(Site::of_entry(
        "rules",
        from,
        format!("rules.{from}.{field}[{at}]"),
    ))
}

impl Decision<'_> {
    /// Where the policy writes the grant or the clause of the decision, when
    /// one place does.
    pub fn site(&self, config: &Config) -> Option<Site> {
        let component = |name: &str, field: &str| {
            Some(Site::of_entry(
                "components",
                name,
                format!("components.{name}.{field}"),
            ))
        };
        let (from, to) = (self.from, self.to);
        match &self.grant {
            Ok(Some(grant)) => grant_site(config, from, to?, *grant),
            Ok(None) => None,
            Err(Clause::Refused(refusal)) => refusal_site(config, from, to?, refusal),
            Err(Clause::ImporterNotListed(_)) => component(to?, "allow_importers"),
            Err(Clause::OutsideSurface { held, .. }) => {
                let to = to?;
                match held.first() {
                    Some(surface) => component(to, &format!("surfaces.{}", surface.name)),
                    None if config.components[to].public.is_some() => component(to, "public"),
                    None => component(to, "surfaces"),
                }
            }
            Err(Clause::OtherInstance { .. }) => component(from, "isolated"),
            Err(Clause::SliceMismatch { .. }) => component(from, "in"),
            Err(Clause::ExcludedTarget | Clause::External(_)) => None,
        }
    }
}

/// The named surfaces of `to` that hold a file closed to a component, with the
/// reason of the first that gives one.
type Held<'a> = (Vec<&'a NamedSurface>, Option<&'a str>);

/// The named surfaces of `to` that hold `target`, with the reason of the
/// first that gives one, when `from` may not import it: it is outside the
/// public surface and in no named surface that lists `from`.
fn outside_surface<'a>(
    map: &'a ComponentMap,
    from: &str,
    to: &str,
    target: &str,
) -> Option<Held<'a>> {
    if map.is_public(to, target) {
        return None;
    }
    let holding = map.surfaces_holding(to, target);
    if holding
        .iter()
        .any(|s| s.importers.iter().any(|c| c == from))
    {
        return None;
    }
    let because = holding.iter().find_map(|s| s.because.as_deref());
    Some((holding, because))
}

/// The file by which `dep` of `from` enters `to`, when the surfaces of `to` keep it
/// closed to `from`, with the named surfaces that hold it. They judge the modules a
/// file names, not the names it writes: a name a public file re-exports is public,
/// wherever it is defined. A name the file reaches only through aliases, or through
/// a re-export in a component other than both ends, is judged where its way enters
/// `to`: at the first alias or re-exporting module of `to` on the way, else at the
/// file that defines it. It passes when one of its ways enters by a file open to
/// `from`.
fn closed_entry<'a>(
    map: &'a ComponentMap,
    from: &str,
    to: &str,
    dep: &Dep,
    target: &str,
) -> Option<(String, Held<'a>)> {
    // A module the file names enters `to` by itself.
    let ways: Vec<Vec<&str>> = match dep.written {
        Written::Module => vec![Vec::new()],
        Written::Name => {
            // Through aliases alone: a file that also writes the name itself names it.
            let aliased = match dep.mentions.is_empty() {
                true => &dep.aliased[..],
                false => &[],
            };
            let aliases = aliased.iter().map(|aliased| {
                aliased
                    .chain
                    .iter()
                    .map(|alias| alias.file.as_str())
                    .collect()
            });
            let third = |file: &&str| {
                map.component_of(file)
                    .is_some_and(|component| component != from && component != to)
            };
            let reexports = dep
                .routes
                .iter()
                .map(|route| {
                    route
                        .iter()
                        .map(|hop| hop.file.as_str())
                        .collect::<Vec<_>>()
                })
                .filter(|files| files.iter().any(third));
            aliases.chain(reexports).collect()
        }
    };
    let mut closed = None;
    for way in ways {
        let entry = way
            .into_iter()
            .find(|file| map.component_of(file) == Some(to))
            .unwrap_or(target);
        let held = outside_surface(map, from, to, entry)?;
        closed.get_or_insert((entry.to_string(), held));
    }
    closed
}

/// The placeholder both ends name in their `in` patterns
/// (`src/features/<feature>/ui`) and fill differently, if there is one.
fn slice_mismatch<'a>(map: &ComponentMap, file: &str, target: &str) -> Option<Clause<'a>> {
    let theirs = map.captures_of(target);
    map.captures_of(file).into_iter().find_map(|(name, here)| {
        let (_, there) = theirs.iter().find(|(n, _)| *n == name)?;
        (here != *there).then(|| Clause::SliceMismatch {
            name: name.to_string(),
            here,
            there: there.clone(),
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use std::path::Path;

    use crate::check;
    use crate::components::ComponentMap;
    use crate::config::Config;
    use crate::indexer::{CacheUse, Environment, Indexing, Jobs};
    use crate::scan::{self, Cache};

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            // A Cargo workspace keeps its manifests as `Cargo.toml.fixture`.
            let dest = to.join(if name == "Cargo.toml.fixture" {
                "Cargo.toml".into()
            } else {
                name
            });
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &dest);
            } else {
                fs::copy(entry.path(), dest).unwrap();
            }
        }
    }

    /// The packages a fixture's `modules.txt` lists, stubbed as installed for
    /// the TypeScript indexer, as `tests/cli.rs` does.
    fn stub_packages(root: &Path) {
        let Ok(list) = fs::read_to_string(root.join("modules.txt")) else {
            return;
        };
        for specifier in list.lines() {
            let mut parts = specifier.split('/');
            let first = parts.next().unwrap();
            let (name, rest): (String, Vec<&str>) = if first.starts_with('@') {
                (
                    format!("{first}/{}", parts.next().unwrap()),
                    parts.collect(),
                )
            } else {
                (first.to_string(), parts.collect())
            };
            let dir = root.join("node_modules").join(&name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("package.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0","types":"index.d.ts"}}"#),
            )
            .unwrap();
            let stub = "declare const stub: any;\nexport default stub;\n";
            fs::write(dir.join("index.d.ts"), stub).unwrap();
            if !rest.is_empty() {
                let file = dir.join(format!("{}.d.ts", rest.join("/")));
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, stub).unwrap();
            }
        }
        fs::remove_file(root.join("modules.txt")).unwrap();
    }

    /// Runs `each` on the config, map and dependencies of every fixture project.
    fn each_fixture(each: impl Fn(&str, &Config, &ComponentMap, &[crate::lang::Dep]) + Sync) {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut names: Vec<String> = fs::read_dir(&fixtures)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // As in `tests/cli.rs`: no scip-ruby is built for Linux on Arm.
        let no_ruby = cfg!(all(target_os = "linux", target_arch = "aarch64"))
            && which::which("scip-ruby").is_err();
        names.retain(|name| !(no_ruby && name.starts_with("ruby")));
        std::thread::scope(|s| {
            for name in &names {
                let (fixtures, each) = (&fixtures, &each);
                s.spawn(move || {
                    let tmp = tempfile::tempdir().unwrap();
                    copy_dir(&fixtures.join(name), tmp.path());
                    stub_packages(tmp.path());
                    if tmp.path().join("composer.json").exists() {
                        // scip-php reads the autoloader.
                        let composer = which::which("composer").expect("composer is needed");
                        let mut command = std::process::Command::new(composer);
                        // Not the repository of a git that runs the tests.
                        for name in crate::diff::repository_env() {
                            command.env_remove(name);
                        }
                        let status = command
                            .args(["install", "--no-interaction", "--quiet"])
                            .current_dir(tmp.path())
                            .status()
                            .expect("composer runs");
                        assert!(status.success(), "composer install in {name}");
                    }
                    let config =
                        crate::policy::Policy::load(&tmp.path().join("easement.yml"), None)
                            .unwrap()
                            .config;
                    let mut cache = Cache::new(Indexing {
                        cache: CacheUse::Off,
                        cache_dir: None,
                        environment: Environment::of_process(),
                        jobs: Jobs::default(),
                    });
                    let scan = scan::scan(tmp.path(), &config.scan_policy(), &mut cache).unwrap();
                    let map = ComponentMap::new(&config).unwrap();
                    each(name, &config, &map, &scan.deps);
                });
            }
        });
    }

    /// Each rejected decision is a dependency violation of `check`, and each of
    /// those a rejected decision.
    #[test]
    fn rejected_decisions_are_the_violations_check_reports_of_dependencies() {
        each_fixture(|name, config, map, deps| {
            let mut rejected: Vec<String> = deps
                .iter()
                .filter_map(|dep| Some((dep, decide(config, map, dep).ok()?)))
                .filter_map(|(dep, d)| match &d.grant {
                    Err(clause) => Some(format!(
                        "{} {}:{} {} -> {} {}",
                        clause.rule().as_str(),
                        slash(&dep.file),
                        dep.line,
                        d.from,
                        d.to.unwrap_or_default(),
                        d.target
                    )),
                    Ok(_) => None,
                })
                .collect();
            let (found, _, _) = check::evaluate(config, map, deps);
            let mut violations: Vec<String> = found
                .iter()
                .map(|v| {
                    format!(
                        "{} {}:{} {} -> {} {}",
                        v.rule.as_str(),
                        v.file,
                        v.line,
                        v.from,
                        v.to,
                        v.target
                    )
                })
                .collect();
            rejected.sort();
            violations.sort();
            assert_eq!(rejected, violations, "{name}");
        });
    }

    /// The config of the matrix below: `app` imports a name that several
    /// files may declare — in one component, in two, in none, or hidden by the
    /// policy.
    fn candidates_config() -> (Config, ComponentMap) {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  app: { in: app }
  lib: { in: lib }
  other: { in: other }
rules:
  app: { may_depend_on: [lib, other] }
exclude: [\"legacy/**\", \"lib/legacy/**\"]
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        (config, map)
    }

    /// The dependency of `app` on a name that `candidates` may declare.
    fn dep_of(candidates: &[&str]) -> Dep {
        let mut dep = Dep::module(
            "app/src/main/kotlin/com/acme/App.kt",
            1,
            "com.acme.thing.Thing",
            Target::Internal(Unit::from("lib/src/main/kotlin/com/acme/thing/Thing.kt")),
        );
        dep.candidates = candidates.iter().map(|c| c.to_string()).collect();
        dep
    }

    /// The scope of the candidates tells how certain the target is: the file of
    /// one component, the file of no component, or a file the policy hides. The
    /// target is the copy in the importing file's component, otherwise the first
    /// by path, and a file the policy hides is not skipped, so its dependency
    /// fails as `excluded`.
    #[test]
    fn the_scope_of_the_candidates_tells_how_certain_the_target_is() {
        let (config, map) = candidates_config();
        for (candidates, resolution, to, hidden) in [
            (
                ["lib/a/Thing.kt", "lib/b/Thing.kt"],
                ResolutionStatus::SameScope,
                Some("lib"),
                false,
            ),
            (
                ["lib/a/Thing.kt", "other/a/Thing.kt"],
                ResolutionStatus::Ambiguous,
                Some("lib"),
                false,
            ),
            (
                ["shared/a/Thing.kt", "shared/b/Thing.kt"],
                ResolutionStatus::SameScope,
                None,
                false,
            ),
            (
                ["shared/a/Thing.kt", "legacy/a/Thing.kt"],
                ResolutionStatus::SameScope,
                None,
                true,
            ),
            (
                ["legacy/a/Thing.kt", "legacy/b/Thing.kt"],
                ResolutionStatus::SameScope,
                None,
                true,
            ),
            (
                ["lib/a/Thing.kt", "legacy/a/Thing.kt"],
                ResolutionStatus::Ambiguous,
                None,
                true,
            ),
            (
                ["lib/a/Thing.kt", "shared/a/Thing.kt"],
                ResolutionStatus::Ambiguous,
                Some("lib"),
                false,
            ),
            (
                ["lib/a/Thing.kt", "lib/legacy/a/Thing.kt"],
                ResolutionStatus::SameScope,
                Some("lib"),
                false,
            ),
        ] {
            let decision = decide(&config, &map, &dep_of(&candidates)).unwrap();
            assert_eq!(decision.resolution, resolution, "{candidates:?}");
            assert_eq!(decision.to, to, "{candidates:?}");
            assert_eq!(
                matches!(decision.grant, Err(Clause::ExcludedTarget)),
                hidden,
                "{candidates:?}: {:?}",
                decision.grant
            );
        }
    }

    /// The status is `snake_case` in the public format; the callers leave the
    /// field out when it is `unambiguous`.
    #[test]
    fn the_status_serializes_as_snake_case() {
        let json = |status| serde_json::to_value(status).unwrap();
        assert_eq!(json(ResolutionStatus::SameScope), "same_scope");
        assert_eq!(json(ResolutionStatus::Ambiguous), "ambiguous");
        assert_eq!(json(ResolutionStatus::Unambiguous), "unambiguous");
    }

    /// A candidate the policy hides is not replaced by another one: the choice
    /// is the first by path whatever `exclude` says, so the dependency fails as
    /// `excluded` instead of being taken for the copy that breaks no rule.
    #[test]
    fn an_excluded_candidate_is_not_replaced_by_another() {
        let (config, map) = candidates_config();
        let decision = decide(
            &config,
            &map,
            &dep_of(&["legacy/a/Thing.kt", "lib/a/Thing.kt"]),
        )
        .unwrap();
        assert_eq!(decision.target, "legacy/a/Thing.kt");
        assert_eq!(decision.to, None);
        assert!(
            matches!(decision.grant, Err(Clause::ExcludedTarget)),
            "{:?}",
            decision.grant
        );
    }

    /// The copy in the component of the importing file is taken before the first
    /// by path.
    #[test]
    fn a_candidate_of_the_importing_component_is_preferred() {
        let (config, map) = candidates_config();
        let decision = decide(
            &config,
            &map,
            &dep_of(&["lib/a/Thing.kt", "app/z/Thing.kt"]),
        )
        .unwrap();
        assert_eq!(decision.target, "app/z/Thing.kt");
        assert_eq!(decision.to, Some("app"));
    }

    /// With no copy in the importing file's component, the first by path is
    /// taken, and `exclude` does not reorder the candidates.
    #[test]
    fn without_a_candidate_of_the_importing_component_the_first_by_path_is_taken() {
        let (config, map) = candidates_config();
        let chosen =
            |candidates: &[&str]| decide(&config, &map, &dep_of(candidates)).unwrap().target;
        assert_eq!(
            chosen(&["other/a/Thing.kt", "lib/a/Thing.kt"]),
            "lib/a/Thing.kt"
        );
        assert_eq!(
            chosen(&["lib/a/Thing.kt", "legacy/a/Thing.kt"]),
            "legacy/a/Thing.kt"
        );
    }

    /// When every candidate is hidden, the dependency fails as `excluded`: no
    /// other candidate lets it pass.
    #[test]
    fn a_dependency_of_only_hidden_candidates_fails_as_excluded() {
        let (config, map) = candidates_config();
        let decision = decide(
            &config,
            &map,
            &dep_of(&["legacy/a/Thing.kt", "legacy/b/Thing.kt"]),
        )
        .unwrap();
        assert!(
            matches!(decision.grant, Err(Clause::ExcludedTarget)),
            "{:?}",
            decision.grant
        );
        assert_eq!(decision.target, "legacy/a/Thing.kt");
        assert_eq!(decision.to, None);
    }
}
