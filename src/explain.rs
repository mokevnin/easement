use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::check::RuleId;
use crate::components::ComponentMap;
use crate::config::{Listed, TESTS};
use crate::decision::{self, Clause, ClauseId, Decision, Grant, Outside, Refusal};
use crate::lang::registry;
use crate::policy::{Policy, Provenance, Site};

/// A path as the config sees it: relative to the project root, `/`-separated.
/// An absolute path, and a path that exists relative to the working directory,
/// are taken from there, whether or not the file exists yet; anything else is
/// already relative to the root.
pub fn root_relative(root: &Path, given: &Path) -> String {
    // A relative path that does not exist is read against the root, not the
    // working directory.
    let resolved = (given.is_absolute() || given.exists())
        .then(|| crate::relpath::real(given).ok())
        .flatten()
        .and_then(|abs| abs.strip_prefix(root).map(Path::to_path_buf).ok())
        .unwrap_or_else(|| PathBuf::from(given.to_string_lossy().trim_start_matches("./")));
    crate::relpath::slash(&resolved)
}

/// Which component owns a path, and why no rule judges its imports when none does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Owned,
    /// No component owns it.
    Unowned,
    /// `exclude` hides it, whatever component its path matches.
    Excluded,
}

/// Who owns a path and how.
#[derive(Debug, Serialize)]
pub struct Ownership {
    pub status: Status,
    /// The component whose `in` pattern matches the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// The `in` pattern that puts it there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The values the named placeholders of the pattern take.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub captures: BTreeMap<String, String>,
    /// The instance of an isolated component it belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Whether its name or place makes it test code.
    pub test: bool,
    /// Whether `target_only` keeps it a target alone, whose imports are not read.
    pub target_only: bool,
    /// Where the policy defines the component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

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

/// The decision on one dependency, with its paths.
#[derive(Debug, Serialize)]
pub struct DependencyDecision {
    /// The path it imports, or the external module.
    pub target: String,
    /// The component of the target, when one owns it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<Grant>,
    /// The clause that rejects it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_by: Option<ClauseId>,
    /// The rule it breaks when it is rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes the grant or the clause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    /// How certainly the target was resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<crate::decision::ResolutionStatus>,
    /// Other files that may declare the same name, when resolution is not
    /// unambiguous.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
}

impl DependencyDecision {
    pub fn of(policy: &Policy, decision: &Decision) -> Self {
        Self {
            target: decision.target.clone(),
            to: decision.to.map(str::to_string),
            allowed: decision.grant.is_ok(),
            grant: decision.grant.as_ref().ok().copied().flatten(),
            rejected_by: decision.grant.as_ref().err().map(Clause::id),
            rule: decision.grant.as_ref().err().map(Clause::rule),
            because: decision.because.map(str::to_string),
            provenance: decision
                .site(&policy.config)
                .map(|site| policy.provenance(&site)),
            resolution: if decision.resolution == crate::decision::ResolutionStatus::Unambiguous {
                None
            } else {
                Some(decision.resolution)
            },
            candidates: decision.candidates.clone(),
        }
    }
}

/// The decision on depending on one other component.
#[derive(Debug, Serialize)]
pub struct Toward {
    pub to: String,
    #[serde(flatten)]
    pub decision: PairDecision,
}

/// A named surface of the component.
#[derive(Debug, Serialize)]
pub struct SurfaceView {
    pub name: String,
    pub paths: Vec<String>,
    pub importers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub provenance: Provenance,
}

/// An item of a rule list, with the reason given for it.
#[derive(Debug, Serialize)]
pub struct Named {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
}

fn named(items: &[Listed]) -> Vec<Named> {
    items
        .iter()
        .map(|l| Named {
            name: l.name().to_string(),
            because: l.because().map(str::to_string),
        })
        .collect()
}

/// What the rule of the component asks besides its dependencies on others.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fan_out: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_instability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub provenance: Provenance,
}

/// What the owning component may do.
#[derive(Debug, Serialize)]
pub struct ComponentPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub priority: i32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
    pub isolated: bool,
    /// The public surface, when the component restricts it: empty when it
    /// declares only named surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<Vec<String>>,
    pub surfaces: Vec<SurfaceView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_importers: Option<Vec<String>>,
    /// The components it may depend on, with the grant of each.
    pub grants: Vec<Toward>,
    /// The components a prohibition keeps it off.
    pub prohibitions: Vec<Toward>,
    /// The other components it may not depend on: nothing grants it, or
    /// their `allow_importers` does not list it.
    pub not_allowed: Vec<Toward>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleView>,
}

/// Which component a path belongs to and what that component may do.
#[derive(Debug, Serialize)]
pub struct Explanation {
    pub path: String,
    pub ownership: Ownership,
    #[serde(flatten)]
    pub component: Option<ComponentPolicy>,
}

/// Says which component a path belongs to and what that component may do.
pub fn explain(policy: &Policy, map: &ComponentMap, rel: &str) -> Explanation {
    let config = &policy.config;
    let owned = map.component_and_pattern(rel);
    let status = match (decision::unread(map, rel), owned) {
        (Some(Outside::Excluded), _) => Status::Excluded,
        (_, None) => Status::Unowned,
        (_, Some(_)) => Status::Owned,
    };
    let ownership = Ownership {
        status,
        component: owned.map(|(name, _)| name.to_string()),
        pattern: owned.map(|(_, pattern)| pattern.to_string()),
        captures: map
            .captures_of(rel)
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        instance: map.instance_of(rel),
        test: registry::is_test_file(rel),
        target_only: map.is_target_only(rel),
        provenance: owned.map(|(name, _)| {
            policy.provenance(&if name == TESTS && config.tests.is_some() {
                Site::of_key("tests")
            } else {
                Site::of_entry("components", name, format!("components.{name}.in"))
            })
        }),
    };
    Explanation {
        path: rel.to_string(),
        ownership,
        component: owned.map(|(name, _)| component_policy(policy, map, name)),
    }
}

fn component_policy(policy: &Policy, map: &ComponentMap, name: &str) -> ComponentPolicy {
    let config = &policy.config;
    let component = &config.components[name];
    let mut grants = Vec::new();
    let mut prohibitions = Vec::new();
    let mut not_allowed = Vec::new();
    for to in config.components.keys().filter(|c| c.as_str() != name) {
        let decision = PairDecision::of(policy, map, name, to);
        let list = if decision.allowed {
            &mut grants
        } else if decision.prohibited() {
            &mut prohibitions
        } else {
            &mut not_allowed
        };
        list.push(Toward {
            to: to.clone(),
            decision,
        });
    }
    let strings = |items: Vec<&str>| items.into_iter().map(str::to_string).collect::<Vec<_>>();
    ComponentPolicy {
        owner: component.owner.clone(),
        priority: component.priority,
        except: component
            .except
            .as_ref()
            .map(|e| strings(e.as_vec()))
            .unwrap_or_default(),
        isolated: component.isolated,
        public: component.public_surface().map(strings),
        surfaces: component
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
            .collect(),
        allow_importers: component.allow_importers.clone(),
        grants,
        prohibitions,
        not_allowed,
        rule: config.rules.get(name).map(|rule| RuleView {
            may_use_external: rule.may_use_external.as_deref().map(named),
            forbid_external: named(&rule.forbid_external),
            must_depend_on: named(&rule.must_depend_on),
            must_use_external: named(&rule.must_use_external),
            requires_file: named(&rule.requires_file),
            forbid_file: named(&rule.forbid_file),
            requires_directive: named(&rule.requires_directive),
            max_fan_out: rule.max_fan_out,
            max_instability: rule.max_instability,
            because: rule.because.clone(),
            provenance: policy.provenance(&Site::of_entry("rules", name, format!("rules.{name}"))),
        }),
    }
}

impl Explanation {
    /// The answer as text.
    pub fn text(&self) -> Result<String> {
        render(self)
    }
}

/// The text form of an [`Explanation`]: an error for a path no component owns.
pub fn render(explanation: &Explanation) -> Result<String> {
    let rel = &explanation.path;
    let ownership = &explanation.ownership;
    let (Some(name), Some(pattern), Some(component)) = (
        &ownership.component,
        &ownership.pattern,
        &explanation.component,
    ) else {
        bail!("{rel} belongs to no component, so easement does not check it");
    };
    let mut out = format!("{rel}\n  component: {name} (matches `{pattern}`)\n");
    if ownership.status == Status::Excluded {
        out.push_str("  excluded: `exclude` hides it, so no rule judges it\n");
    } else if ownership.target_only {
        out.push_str(
            "  target_only: its dependencies are not read; it is judged only as a target\n",
        );
    }
    match &component.public {
        Some(public) if public.is_empty() => {
            out.push_str("  public surface: none (only the named surfaces are open)\n");
        }
        Some(public) => out.push_str(&format!(
            "  public surface: {} (other components may use only these)\n",
            public.join(", ")
        )),
        None => {}
    }
    for surface in &component.surfaces {
        let because = surface
            .because
            .as_deref()
            .map(|b| format!(": {b}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  surface {}: {} (open to {}{because})\n",
            surface.name,
            surface.paths.join(", "),
            surface.importers.join(", ")
        ));
    }
    if let Some(owner) = &component.owner {
        out.push_str(&format!("  owner: {owner}\n"));
    }
    if component.priority != 0 {
        out.push_str(&format!("  priority: {}\n", component.priority));
    }
    if !component.except.is_empty() {
        out.push_str(&format!("  except: {}\n", component.except.join(", ")));
    }
    if component.isolated {
        out.push_str("  isolated: instances of this component must not depend on each other\n");
        if let Some(instance) = &ownership.instance {
            out.push_str(&format!("  instance: {instance}\n"));
        }
    }
    if let Some(importers) = &component.allow_importers {
        out.push_str(&format!("  open only to: {}\n", importers.join(", ")));
    }
    let allowed: Vec<String> = component
        .grants
        .iter()
        .map(|Toward { to, decision }| {
            let grant = decision.grant.map(Grant::as_str).unwrap_or_default();
            let because = decision
                .because
                .as_deref()
                .map(|b| format!(": {b}"))
                .unwrap_or_default();
            let conditions: String = decision
                .conditions
                .iter()
                .map(|c| format!("; {c}"))
                .collect();
            format!("{to} ({grant}{because}{conditions})")
        })
        .collect();
    let forbidden: Vec<String> = component
        .prohibitions
        .iter()
        .map(|Toward { to, decision }| {
            if decision.rejected_by == Some(ClauseId::MustNotReach) {
                format!("{to} (not even through others)")
            } else {
                to.clone()
            }
        })
        .collect();
    let other: Vec<String> = component
        .not_allowed
        .iter()
        .map(|Toward { to, decision }| match &decision.allow_importers {
            Some(importers) if decision.grant.is_some() => {
                format!("{to} (open only to {})", importers.join(", "))
            }
            _ => to.clone(),
        })
        .collect();
    let list = |items: &[String]| {
        if items.is_empty() {
            "nothing".to_string()
        } else {
            items.join(", ")
        }
    };
    out.push_str(&format!("  may depend on: {}\n", list(&allowed)));
    if !forbidden.is_empty() {
        out.push_str(&format!("  forbidden: {}\n", forbidden.join(", ")));
    }
    if !other.is_empty() {
        out.push_str(&format!("  not allowed: {}\n", other.join(", ")));
    }
    if let Some(rule) = &component.rule {
        let names = |items: &[Named]| {
            items
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(external) = &rule.may_use_external {
            let only = if external.is_empty() {
                "nothing".to_string()
            } else {
                names(external)
            };
            out.push_str(&format!("  external modules: only {only}\n"));
        }
        if !rule.forbid_external.is_empty() {
            out.push_str(&format!(
                "  forbidden external modules: {}\n",
                names(&rule.forbid_external)
            ));
        }
        if !rule.must_depend_on.is_empty() {
            out.push_str(&format!(
                "  every directory must depend on: {}\n",
                names(&rule.must_depend_on)
            ));
        }
        if !rule.must_use_external.is_empty() {
            out.push_str(&format!(
                "  every directory must use external: {}\n",
                names(&rule.must_use_external)
            ));
        }
        if !rule.requires_file.is_empty() {
            out.push_str(&format!(
                "  every directory must contain: {}\n",
                names(&rule.requires_file)
            ));
        }
        if !rule.forbid_file.is_empty() {
            out.push_str(&format!(
                "  forbidden files: {}\n",
                names(&rule.forbid_file)
            ));
        }
        if !rule.requires_directive.is_empty() {
            out.push_str(&format!(
                "  every directory must match: {}\n",
                names(&rule.requires_directive)
            ));
        }
        if let Some(max) = rule.max_fan_out {
            out.push_str(&format!("  max_fan_out: {max}\n"));
        }
        if let Some(max) = rule.max_instability {
            out.push_str(&format!("  max_instability: {max}\n"));
        }
        if let Some(because) = &rule.because {
            out.push_str(&format!("  because: {because}\n"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute path names its file under the root whether or not the file
    /// exists yet, as a new file `propose` is asked about does not, and through
    /// a link to the root as well (`/var` is `/private/var` on macOS).
    #[test]
    fn an_absolute_path_of_a_file_not_written_yet_is_relative_to_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(root_relative(&root, &root.join("src/new.ts")), "src/new.ts");
        assert_eq!(
            root_relative(&root, &dir.path().join("src/new.ts")),
            "src/new.ts"
        );
        assert_eq!(
            root_relative(&root, &root.join("lib/deeper/new.ts")),
            "lib/deeper/new.ts"
        );
        assert_eq!(
            root_relative(&root, Path::new("./src/new.ts")),
            "src/new.ts"
        );
    }

    /// The text `explain` prints for `rel` under the policy `yaml`.
    fn explained(yaml: &str) -> impl Fn(&str) -> Result<String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("easement.yml");
        std::fs::write(&path, yaml).unwrap();
        let policy = Policy::load(&path, None).unwrap();
        let map = ComponentMap::new(&policy.config).unwrap();
        move |rel| render(&explain(&policy, &map, rel))
    }

    #[test]
    fn explains_allowed_forbidden_and_unmapped() {
        let explain = explained(
            "version: 1
components:
  web: { in: src/web }
  core: { in: src/core }
  data: { in: src/data, public: src/data/api, allow_importers: [core], priority: 2 }
  pages: { in: 'src/pages/*', isolated: true }
  shared: { in: src/shared }
common: [shared]
layers: [web, core]
rules:
  web:
    must_not_depend_on: [data]
    must_not_reach: [pages]
    forbid_external: [moment]
    requires_file: [README.md]
    because: pages go through core
",
        );
        let out = explain("src/web/page.ts").unwrap();
        assert!(out.contains("component: web (matches `src/web`)"));
        assert!(out.contains("may depend on: core (layers), shared (common)"));
        assert!(out.contains("forbidden: data, pages (not even through others)"));
        assert!(out.contains("forbidden external modules: moment"));
        assert!(out.contains("every directory must contain: README.md"));
        assert!(out.contains("because: pages go through core"));
        let data = explain("src/data/db.ts").unwrap();
        assert!(data.contains("public surface: src/data/api"));
        assert!(data.contains("open only to: core"));
        assert!(data.contains("priority: 2"));
        let page = explain("src/pages/home/a.ts").unwrap();
        assert!(page.contains("instance: src/pages/home"));
        assert!(data.contains("not allowed: core, pages, web"));
        assert!(explain("stray.ts").is_err());
    }

    #[test]
    fn ownership_says_what_hides_a_path_and_whether_it_is_test_code() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("easement.yml");
        std::fs::write(
            &path,
            "version: 1
components:
  app: { in: src }
exclude: [src/gen]
target_only: ['**/*.pb.go']
tests: auto
",
        )
        .unwrap();
        let policy = Policy::load(&path, None).unwrap();
        let map = ComponentMap::new(&policy.config).unwrap();
        let explained = |rel| explain(&policy, &map, rel);

        let hidden = explained("src/gen/api.go");
        assert_eq!(hidden.ownership.status, Status::Excluded);
        assert!(
            render(&hidden)
                .unwrap()
                .contains("  excluded: `exclude` hides it, so no rule judges it\n")
        );
        let generated = explained("src/api.pb.go");
        assert_eq!(generated.ownership.status, Status::Owned);
        assert!(generated.ownership.target_only);
        assert!(render(&generated).unwrap().contains(
            "  target_only: its dependencies are not read; it is judged only as a target\n"
        ));

        // Test code by its name is what `tests: auto` gives to `tests`, and a
        // module named `tests` under `src/` stays the crate's own.
        for (rel, test) in [
            ("src/retry_test.go", true),
            ("src/retry.go", false),
            ("tests/api.rs", true),
            ("src/store/tests/fixtures.rs", false),
            ("src/page.test.ts", true),
        ] {
            let ownership = explained(rel).ownership;
            assert_eq!(ownership.test, test, "{rel}");
            assert_eq!(ownership.component.as_deref() == Some(TESTS), test, "{rel}");
        }
    }

    #[test]
    fn an_allowed_component_names_its_surface_and_importers_it_does_not_list() {
        let explain = explained(
            "version: 1
components:
  web: { in: src/web }
  billing: { in: src/billing, public: src/billing/api }
  data: { in: src/data, allow_importers: [billing] }
rules:
  web: { may_depend_on: [billing, data] }
",
        );
        let out = explain("src/web/page.ts").unwrap();
        assert!(
            out.contains(
                "may depend on: billing (may_depend_on; only through the public surface of billing: src/billing/api)\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("not allowed: data (open only to billing)\n"),
            "{out}"
        );
    }
}
