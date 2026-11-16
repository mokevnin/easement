use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::check::RuleId;
use crate::components::ComponentMap;
use crate::config::{Ban, TESTS};
use crate::decision::{self, Clause, ClauseId, Decision, Grant, Outside};
use crate::lang::registry;
use crate::policy::{Policy, Provenance, Site};
use crate::resolved::{self, Dependencies, FileSymbols, RuleView, SurfaceView, Toward};

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
    /// How the symbol rules bind the path: the names it may not use, those of the
    /// top-level `forbid_symbol` it may use all the same, and the `public_symbols`
    /// of the components it may depend on.
    #[serde(skip_serializing_if = "FileSymbols::is_empty")]
    pub symbols: FileSymbols,
    /// The items of the top-level `forbid_external`, which bind the path whatever
    /// component owns it, or none: each with what lets the path use the module all
    /// the same, if anything does.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbid_external: Vec<ExternalBan>,
}

/// An item of the top-level `forbid_external` as it binds one path.
#[derive(Debug, Serialize)]
pub struct ExternalBan {
    /// The module, as written.
    pub name: String,
    /// The component of the path that `allow` lets use the module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<String>,
    /// The `allow_in` pattern that lets the path use the module, the first one
    /// the item writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_in: Option<String>,
    /// Whether the path may use the module: `allow` or `allow_in` lets it, and
    /// the own rule of its component neither forbids the module nor leaves it
    /// out of a `may_use_external`.
    pub allowed: bool,
    /// Who may use the module, the components and then the paths, as written.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    /// Where the policy writes what lets the path use it, or else the item.
    pub provenance: Provenance,
}

/// How each item of the top-level `forbid_external` binds `rel`.
fn external_bans(policy: &Policy, map: &ComponentMap, rel: &str) -> Vec<ExternalBan> {
    let config = &policy.config;
    let owner = map.component_of(rel);
    config
        .forbid_external
        .iter()
        .enumerate()
        .map(|(item, listed)| {
            let allow = owner
                .filter(|component| listed.allows(component))
                .map(str::to_string);
            let allow_in = map.granted_in(Ban::External, item, rel);
            let field = match (&allow, allow_in) {
                (Some(_), _) => format!("forbid_external[{item}].allow"),
                (None, Some((at, _))) => format!("forbid_external[{item}].allow_in[{at}]"),
                (None, None) => format!("forbid_external[{item}]"),
            };
            // The rule of the component still applies: its own `forbid_external`, and
            // `may_use_external`, which has to cover the module.
            let own = owner.is_some_and(|component| {
                let module = listed.name();
                crate::components::own_external_ban(config, component, module).is_some()
                    || config
                        .rules
                        .get(component)
                        .and_then(|rule| rule.may_use_external.as_deref())
                        .is_some_and(|allowed| {
                            !allowed
                                .iter()
                                .any(|l| ComponentMap::covers_module(l.name(), module))
                        })
            });
            ExternalBan {
                name: listed.name().to_string(),
                allowed: (allow.is_some() || allow_in.is_some()) && !own,
                allow,
                allow_in: allow_in.map(|(_, pattern)| pattern.to_string()),
                only: listed.who_may(),
                because: listed.because().map(str::to_string),
                provenance: policy.provenance(&Site::in_key(Ban::External.key(), field)),
            }
        })
        .collect()
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
    let component = owned.map(|(name, _)| component_policy(policy, map, name));
    let grants = component.as_ref().map_or(&[][..], |c| c.grants.as_slice());
    Explanation {
        path: rel.to_string(),
        ownership,
        symbols: FileSymbols::of(policy, map, rel, grants),
        component,
        forbid_external: external_bans(policy, map, rel),
    }
}

fn component_policy(policy: &Policy, map: &ComponentMap, name: &str) -> ComponentPolicy {
    let component = &policy.config.components[name];
    let Dependencies {
        grants,
        prohibitions,
        not_allowed,
    } = Dependencies::of(policy, map, name);
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
        surfaces: resolved::surfaces(policy, name),
        allow_importers: component.allow_importers.clone(),
        grants,
        prohibitions,
        not_allowed,
        rule: RuleView::of(policy, name),
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
        // Only the top-level lists bind a file of no component.
        if (explanation.forbid_external.is_empty() && explanation.symbols.is_empty())
            || ownership.status == Status::Excluded
        {
            bail!("{rel} belongs to no component, so easement does not check it");
        }
        let mut out = format!("{rel}\n  component: none, so only the top-level lists judge it\n");
        symbol_and_external_lines(explanation, &mut out);
        return Ok(out);
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
        out.push_str(&format!("  {}\n", resolved::surface_line(surface)));
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
    let allowed = resolved::grant_items(&component.grants);
    let forbidden = resolved::prohibition_items(&component.prohibitions);
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
    out.push_str(&format!(
        "  may depend on: {}\n",
        resolved::listed_or_nothing(&allowed)
    ));
    if !forbidden.is_empty() {
        out.push_str(&format!("  forbidden: {}\n", forbidden.join(", ")));
    }
    if !other.is_empty() {
        out.push_str(&format!("  not allowed: {}\n", other.join(", ")));
    }
    if let Some(rule) = &component.rule {
        for line in rule.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    symbol_and_external_lines(explanation, &mut out);
    Ok(out)
}

/// The lines of the symbol rules and of the items of the top-level
/// `forbid_external` that bind the path.
fn symbol_and_external_lines(explanation: &Explanation, out: &mut String) {
    for line in explanation.symbols.lines() {
        out.push_str(&format!("  {line}\n"));
    }
    for ban in &explanation.forbid_external {
        let because = ban
            .because
            .as_deref()
            .map(|b| format!(": {b}"))
            .unwrap_or_default();
        let granted = match (&ban.allow, &ban.allow_in) {
            (Some(component), _) => Some(format!("allow `{component}`")),
            (None, Some(pattern)) => Some(format!("allow_in `{pattern}`")),
            (None, None) => None,
        };
        let line = match granted {
            Some(granted) if ban.allowed => format!(
                "may use, although forbid_external forbids it: {} ({granted}{because})",
                ban.name
            ),
            _ => format!(
                "forbid_external forbids: {}{}",
                ban.name,
                resolved::who_may_and_why(&ban.only, ban.because.as_deref())
            ),
        };
        out.push_str(&format!("  {line}\n"));
    }
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

    /// A grant through a glob of `allow` points at the glob, and says it in text.
    #[test]
    fn a_name_allowed_through_a_glob_names_the_glob() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("easement.yml");
        std::fs::write(
            &path,
            "version: 1
components:
  core: { in: src/core }
  ee-sso: { in: src/ee/sso }
  web: { in: src/web }
forbid_symbol:
  - { name: lic.Key, allow: [core, 'ee-*'], because: only the licensed code reads the key }
  - os.Exit
rules:
  web: { forbid_symbol: [os.Exit, lic.Key, log.Fatal, log.Panic], because: the web layer returns errors }
",
        )
        .unwrap();
        let policy = Policy::load(&path, None).unwrap();
        let map = ComponentMap::new(&policy.config).unwrap();
        let sso = explain(&policy, &map, "src/ee/sso/login.go");
        let allowed = &sso.symbols.allowed[0];
        assert_eq!(allowed.allow.as_deref(), Some("ee-sso"));
        assert_eq!(allowed.provenance.field, "forbid_symbol[0].allow[1]");
        assert_eq!(allowed.provenance.through, ["ee-*"]);
        // A name the top-level list and the component's own both forbid is given
        // once, as the top-level item writes it, as a violation tells it.
        let web = explain(&policy, &map, "src/web/page.go");
        let forbidden: Vec<(&str, &str)> = web
            .symbols
            .forbidden
            .iter()
            .map(|f| (f.name.as_str(), f.provenance.field.as_str()))
            .collect();
        assert_eq!(
            forbidden,
            [
                ("lic.Key", "forbid_symbol[0]"),
                ("os.Exit", "forbid_symbol[1]"),
                ("log.Fatal", "rules.web.forbid_symbol[2]"),
                ("log.Panic", "rules.web.forbid_symbol[3]"),
            ]
        );
        assert_eq!(web.symbols.forbidden[1].because, None);
        // The own names that say the same share a line in text.
        let said = render(&web).unwrap();
        assert!(
            said.contains("  may not use: log.Fatal, log.Panic (the web layer returns errors)\n"),
            "{said}"
        );
        assert!(
            render(&sso).unwrap().contains(
                "  may use, although forbid_symbol forbids it: lic.Key (allow `ee-sso` through `ee-*`: only the licensed code reads the key)\n"
            )
        );
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
