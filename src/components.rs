use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::config::{Ban, Config, ForbiddenName, TESTS};
use crate::lang::{Unit, registry};
use crate::stdlib::{STD, is_std};

/// Maps repository-relative paths (files or directories, `/`-separated) to components.
///
/// A pattern names a module, whether it is a directory or a file: `P` matches
/// `P` itself, everything below it (`P/**`) and a file `P.<ext>`. So
/// `src/app/cli` covers both `cli.py` and the `cli/` package. When several
/// components match, the higher `priority` wins, then the longest pattern. A
/// component skips the files its `except` patterns match.
pub struct ComponentMap {
    entries: Vec<Entry>,
    /// Whether `tests: auto` made a `tests` component.
    tests: bool,
    /// Component -> matchers of the paths it gives up, for components with `except`.
    except: std::collections::HashMap<String, Vec<ModuleMatcher>>,
    /// Files that may belong to no component in strict mode.
    unowned: GlobSet,
    /// Files that `exclude` hides.
    excluded: GlobSet,
    /// Files that `target_only` keeps as targets alone.
    target_only: GlobSet,
    /// Source files that may stay uncovered by an indexer.
    uncovered_ok: GlobSet,
    /// The `uncovered_ok` entries, in the order of the config.
    accepting: Vec<Accepting>,
    /// The index in `accepting` of the entry each glob of `uncovered_ok` comes from.
    accepting_of_glob: Vec<usize>,
    /// Component -> matchers of its public surface, for components that declare
    /// `public` or `surfaces`: empty when only `surfaces` is declared.
    public: std::collections::HashMap<String, Vec<ModuleMatcher>>,
    /// Component -> its named surfaces, in the order of their names.
    surfaces: std::collections::HashMap<String, Vec<NamedSurface>>,
    /// Component -> what it may do with external modules, for components that restrict it.
    externals: std::collections::HashMap<String, Externals>,
    /// The items of the top-level `forbid_external`, in their order.
    forbidden_externals: Vec<ForbiddenExternal>,
    /// By top-level list, the `allow_in` patterns of each item, as written and
    /// compiled, in the order of the items.
    path_grants: std::collections::BTreeMap<Ban, Vec<Vec<(String, ModuleMatcher)>>>,
    /// Path -> (part, the index in `entries` of the entry that owns it) for every
    /// lookup made so far: the entries never change, so an answer stays true for
    /// the life of the map. Scoped threads share the map, hence the lock.
    owners: Owners,
}

/// The answers of `matching_part` already given, by path and then by part.
type Owners =
    std::sync::RwLock<std::collections::HashMap<String, Vec<(Option<String>, Option<usize>)>>>;

/// A named surface of a component: paths open only to the components it lists.
#[derive(Debug)]
pub struct NamedSurface {
    pub name: String,
    pub importers: Vec<String>,
    pub because: Option<String>,
    matchers: Vec<ModuleMatcher>,
}

impl NamedSurface {
    fn holds(&self, rel_path: &str) -> bool {
        any_match(&self.matchers, rel_path)
    }
}

/// Which components may import a path from outside its component.
#[derive(Debug, PartialEq, Eq)]
pub enum Access<'a> {
    /// Every component the grants let in: the path is in the public surface.
    Everyone,
    /// Only the importers of the named surfaces that hold the path, none when
    /// no surface holds it.
    Only(std::collections::BTreeSet<&'a str>),
}

/// An `uncovered_ok` entry: its globs as written and why.
struct Accepting {
    patterns: Vec<String>,
    because: Option<String>,
}

/// The files no indexer read that one `uncovered_ok` entry accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcceptedGap {
    /// The globs of the entry.
    pub uncovered_ok: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
    pub files: Vec<String>,
}

/// `$std` in `forbid_external`, with the reason of its item.
struct StdForbidden {
    because: Option<String>,
}

struct Externals {
    /// `None`: any external module may be used.
    allowed: Option<GlobSet>,
    /// `$std` is in `may_use_external`.
    allowed_std: bool,
    forbidden: GlobSet,
    /// The reason given with each forbidden pattern, in the order of `forbidden`.
    reasons: Vec<Option<String>>,
    /// `$std` is in `forbid_external`.
    forbidden_std: Option<StdForbidden>,
}

/// An item of the top-level `forbid_external`: the module it forbids, and who may
/// use it all the same.
struct ForbiddenExternal {
    /// The module and what it contains; `None` for `$std`.
    module: Option<GlobMatcher>,
    /// The components that may use it all the same (`allow`); the paths that
    /// may are matched by [`ComponentMap::granted_in`].
    allow: Vec<String>,
    /// The components, then the paths, that may use it, as written, for messages.
    who: Vec<String>,
    /// The item's own reason.
    because: Option<String>,
}

impl ForbiddenExternal {
    /// Whether the item forbids `specifier`, imported by `file`, as a pattern of
    /// a component's own `forbid_external` would.
    fn names(&self, file: &str, specifier: &str) -> bool {
        match &self.module {
            None => is_std(file, specifier),
            Some(module) => prefixes(file, specifier).any(|p| module.is_match(p)),
        }
    }
}

/// Why a component may not use an external module: what is wrong, the reason
/// attached to the forbidden pattern, if it has one, and the item of the
/// top-level `forbid_external` that forbids it, when one does.
pub struct ExternalProblem<'a> {
    pub what: String,
    pub because: Option<&'a str>,
    pub item: Option<usize>,
}

fn glob_set<'a>(
    component: &str,
    key: &str,
    patterns: impl IntoIterator<Item = &'a str>,
) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            Glob::new(pattern)
                .with_context(|| format!("bad pattern `{pattern}` in {key} of `{component}`"))?,
        );
    }
    Ok(builder.build()?)
}

/// A specifier imported by `file` and its prefixes, cut at the separators of
/// the language of `file`, so the pattern `lodash` covers `lodash/fp` but not
/// `lodash.debounce`, `java.util` covers `java.util.List` and `serde` covers
/// `serde::Deserialize`.
fn prefixes<'a>(file: &str, specifier: &'a str) -> impl Iterator<Item = &'a str> {
    registry::module_parents(file, specifier).chain(std::iter::once(specifier))
}

/// A module pattern: the path itself, everything below it, and the file it names
/// without its extension.
#[derive(Debug)]
struct ModuleMatcher([GlobMatcher; 3]);

impl ModuleMatcher {
    fn is_match(&self, rel_path: &str) -> bool {
        self.0.iter().any(|m| m.is_match(rel_path))
    }
}

/// Whether one of `matchers` matches `rel_path`.
fn any_match(matchers: &[ModuleMatcher], rel_path: &str) -> bool {
    matchers.iter().any(|m| m.is_match(rel_path))
}

fn module_matchers(component: &str, pattern: &str) -> Result<ModuleMatcher> {
    let pattern = pattern.trim_end_matches('/');
    let compile = |p: &str| -> Result<GlobMatcher> {
        Ok(GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .with_context(|| format!("bad pattern `{p}` in component `{component}`"))?
            .compile_matcher())
    };
    Ok(ModuleMatcher([
        compile(pattern)?,
        compile(&format!("{pattern}/**"))?,
        compile(&format!("{pattern}.*"))?,
    ]))
}

/// How the own `forbid_external` of `component` writes a pattern that covers
/// `module`, when it has one: empty when spelled alike, else `` as `own` ``,
/// to follow `forbids` in an error.
pub fn own_external_ban(config: &Config, component: &str, module: &str) -> Option<String> {
    let own = config
        .rules
        .get(component)?
        .forbid_external
        .iter()
        .map(ForbiddenName::name)
        .find(|own| ComponentMap::covers_module(own, module))?;
    Some(if own == module {
        String::new()
    } else {
        format!(" as `{own}`")
    })
}

/// Whether the module `pattern` names lies inside one of the `in` patterns
/// of `component` and outside its `except` ones: a pattern is read as the
/// path it spells, wildcards and all.
pub fn lies_inside(
    component: &str,
    patterns: &[&str],
    except: &[&str],
    pattern: &str,
) -> Result<bool> {
    let pattern = pattern.trim_end_matches('/');
    for given_up in except {
        let ModuleMatcher([itself, below, _]) =
            module_matchers(component, given_up.trim_end_matches('/'))?;
        if itself.is_match(pattern) || below.is_match(pattern) {
            return Ok(false);
        }
    }
    for inside in patterns {
        let (glob, _) = glob_and_placeholders(inside.trim_end_matches('/'))?;
        let ModuleMatcher([itself, below, _]) = module_matchers(component, &glob)?;
        if itself.is_match(pattern) || below.is_match(pattern) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The glob of an `in` pattern, where a named placeholder (`<feature>`)
/// matches like `*`, and the placeholder names by segment index.
pub fn glob_and_placeholders(pattern: &str) -> Result<(String, Vec<(usize, String)>)> {
    let mut glob = Vec::new();
    let mut names: Vec<(usize, String)> = Vec::new();
    let mut deep = false;
    for (i, segment) in pattern.trim_end_matches('/').split('/').enumerate() {
        if !segment.contains(['<', '>']) {
            deep |= segment.contains("**");
            glob.push(segment.to_string());
            continue;
        }
        let name = segment
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .with_context(|| {
                format!("`{segment}` in `{pattern}`: a placeholder is a whole segment like `<feature>`, of a-z and _")
            })?;
        if deep {
            anyhow::bail!("`<{name}>` in `{pattern}` comes after `**`, so it has no fixed place");
        }
        if names.iter().any(|(_, n)| n == name) {
            anyhow::bail!("`<{name}>` appears twice in `{pattern}`");
        }
        names.push((i, name.to_string()));
        glob.push("*".to_string());
    }
    Ok((glob.join("/"), names))
}

struct Entry {
    component: String,
    /// The pattern as written, placeholders included.
    pattern: String,
    /// Named placeholders of the pattern by segment index.
    captures: Vec<(usize, String)>,
    pattern_len: usize,
    /// Number of `/`-separated parts of the pattern: where an instance of an isolated component ends.
    parts: usize,
    priority: i32,
    isolated: bool,
    /// The part of a file it owns, when it owns a part and not the file.
    part: Option<String>,
    matchers: ModuleMatcher,
}

impl ComponentMap {
    pub fn new(config: &Config) -> Result<Self> {
        let mut entries = Vec::new();
        let mut public = std::collections::HashMap::new();
        let mut surfaces = std::collections::HashMap::new();
        let mut externals = std::collections::HashMap::new();
        for (name, rule) in &config.rules {
            if rule.may_use_external.is_some() || !rule.forbid_external.is_empty() {
                let forbidden: Vec<_> = rule
                    .forbid_external
                    .iter()
                    .filter(|l| l.name() != STD)
                    .collect();
                externals.insert(
                    name.clone(),
                    Externals {
                        allowed: rule
                            .may_use_external
                            .as_deref()
                            .map(|p| {
                                glob_set(
                                    name,
                                    "may_use_external",
                                    p.iter().map(|l| l.name()).filter(|n| *n != STD),
                                )
                            })
                            .transpose()?,
                        allowed_std: rule
                            .may_use_external
                            .iter()
                            .flatten()
                            .any(|l| l.name() == STD),
                        forbidden: glob_set(
                            name,
                            "forbid_external",
                            forbidden.iter().map(|l| l.name()),
                        )?,
                        reasons: forbidden
                            .iter()
                            .map(|l| l.because().map(str::to_string))
                            .collect(),
                        forbidden_std: rule.forbid_external.iter().find(|l| l.name() == STD).map(
                            |l| StdForbidden {
                                because: l.because().map(str::to_string),
                            },
                        ),
                    },
                );
            }
        }
        let mut except = std::collections::HashMap::new();
        for (name, component) in &config.components {
            for pattern in component.patterns.as_vec() {
                let trimmed = pattern.trim_end_matches('/');
                let (glob, captures) = glob_and_placeholders(trimmed)?;
                entries.push(Entry {
                    component: name.clone(),
                    pattern: trimmed.to_string(),
                    captures,
                    pattern_len: glob.len(),
                    parts: glob.split('/').count(),
                    priority: component.priority,
                    isolated: component.isolated,
                    part: component.part.clone(),
                    matchers: module_matchers(name, &glob)?,
                });
            }
            let matchers = |patterns: Vec<&str>| {
                patterns
                    .into_iter()
                    .map(|p| module_matchers(name, p))
                    .collect::<Result<Vec<_>>>()
            };
            if let Some(patterns) = &component.except {
                except.insert(name.clone(), matchers(patterns.as_vec())?);
            }
            if let Some(paths) = component.public_surface() {
                public.insert(name.clone(), matchers(paths)?);
            }
            if !component.surfaces.is_empty() {
                let named = component
                    .surfaces
                    .iter()
                    .map(|(surface, declared)| {
                        Ok(NamedSurface {
                            name: surface.clone(),
                            importers: declared.importers.clone(),
                            because: declared.because.clone(),
                            matchers: matchers(declared.patterns.as_vec())?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                surfaces.insert(name.clone(), named);
            }
        }
        let mut path_grants = std::collections::BTreeMap::new();
        for ban in Ban::ALL {
            let grants = config
                .top_bans(ban)
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    item.allow_in()
                        .unwrap_or_default()
                        .iter()
                        .map(|pattern| {
                            let place = format!("{}[{i}].allow_in", ban.key());
                            Ok((pattern.clone(), module_matchers(&place, pattern)?))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?;
            path_grants.insert(ban, grants);
        }
        let forbidden_externals = config
            .forbid_external
            .iter()
            .map(|item| {
                let module = match item.name() {
                    STD => None,
                    pattern => Some(
                        Glob::new(pattern)
                            .with_context(|| format!("bad pattern `{pattern}` in forbid_external"))?
                            .compile_matcher(),
                    ),
                };
                let allow = item.allow().unwrap_or_default().to_vec();
                Ok(ForbiddenExternal {
                    module,
                    who: allow
                        .iter()
                        .chain(item.allow_in().unwrap_or_default())
                        .cloned()
                        .collect(),
                    allow,
                    because: item.because().map(str::to_string),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut uncovered_ok = GlobSetBuilder::new();
        let mut accepting = Vec::new();
        let mut accepting_of_glob = Vec::new();
        for (entry, accepted) in config.uncovered_ok.iter().enumerate() {
            accepting.push(Accepting {
                patterns: accepted
                    .patterns()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                because: accepted.because().map(str::to_string),
            });
            for pattern in accepted.patterns() {
                let pattern = pattern.trim_end_matches('/');
                for glob in [pattern.to_string(), format!("{pattern}/**")] {
                    uncovered_ok.add(
                        Glob::new(&glob)
                            .with_context(|| format!("bad uncovered_ok pattern `{pattern}`"))?,
                    );
                    accepting_of_glob.push(entry);
                }
            }
        }
        Ok(Self {
            entries,
            tests: config.tests.is_some(),
            except,
            unowned: crate::relpath::glob_set(&config.unowned, "unowned")?,
            excluded: crate::relpath::glob_set(&config.exclude, "exclude")?,
            target_only: crate::relpath::glob_set(&config.target_only, "target_only")?,
            uncovered_ok: uncovered_ok.build()?,
            accepting,
            accepting_of_glob,
            public,
            surfaces,
            externals,
            forbidden_externals,
            path_grants,
            owners: Owners::default(),
        })
    }

    /// The `allow_in` pattern of the item at `item` of the top-level list of
    /// `ban` that lets `rel_path` use what it forbids, the first one written,
    /// with its index in the list; the component that owns the file does not
    /// matter.
    pub fn granted_in(&self, ban: Ban, item: usize, rel_path: &str) -> Option<(usize, &str)> {
        self.path_grants
            .get(&ban)?
            .get(item)?
            .iter()
            .enumerate()
            .find(|(_, (_, matcher))| matcher.is_match(rel_path))
            .map(|(at, (pattern, _))| (at, pattern.as_str()))
    }

    /// Every `allow_in` pattern, with its top-level list and the index of its
    /// item, that matches none of `files`, in the order of the config.
    pub fn unmatched_grants(&self, files: &[String]) -> Vec<(Ban, usize, &str)> {
        self.path_grants
            .iter()
            .flat_map(|(ban, items)| {
                items.iter().enumerate().flat_map(move |(item, patterns)| {
                    patterns.iter().map(move |grant| (*ban, item, grant))
                })
            })
            .filter(|(_, _, (_, matcher))| !files.iter().any(|f| matcher.is_match(f)))
            .map(|(ban, item, (pattern, _))| (ban, item, pattern.as_str()))
            .collect()
    }

    /// Whether any of the external modules, each with the file that imports it,
    /// matches the pattern, which covers submodules the way `forbid_external` does.
    pub fn external_matches<'a>(
        pattern: &str,
        modules: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> bool {
        let Ok(glob) = Glob::new(pattern) else {
            return false;
        };
        let matcher = glob.compile_matcher();
        modules
            .into_iter()
            .any(|(file, module)| prefixes(file, module).any(|p| matcher.is_match(p)))
    }

    /// Whether the `forbid_external` or `may_use_external` pattern `pattern`
    /// covers all that the pattern `module` names, read with no file to tell its
    /// language: `lodash` covers `lodash/fp`, `okhttp3` covers `okhttp3.Client`,
    /// and `$std` covers only itself.
    pub fn covers_module(pattern: &str, module: &str) -> bool {
        if pattern == STD || module == STD {
            return pattern == module;
        }
        let Ok(glob) = Glob::new(pattern) else {
            return false;
        };
        let matcher = glob.compile_matcher();
        registry::module_parents_anywhere(module)
            .chain(std::iter::once(module))
            .any(|p| matcher.is_match(p))
    }

    /// Whether strict mode lets `rel_path` belong to no component.
    pub fn is_unowned(&self, rel_path: &str) -> bool {
        self.unowned.is_match(rel_path)
    }

    /// Whether `exclude` hides `rel_path`, a file or a directory: a directory
    /// is hidden when a pattern hides everything below it (`src/legacy/**`).
    pub fn is_excluded(&self, rel_path: &str) -> bool {
        self.excluded.is_match(rel_path) || self.excluded.is_match(format!("{rel_path}/-"))
    }

    /// Whether `target_only` names `rel_path`: a target of imports, never
    /// read as their source.
    pub fn is_target_only(&self, rel_path: &str) -> bool {
        self.target_only.is_match(rel_path)
    }

    /// Whether `uncovered_ok` accepts that no indexer covers `rel_path`.
    pub fn is_uncovered_ok(&self, rel_path: &str) -> bool {
        self.uncovered_ok.is_match(rel_path)
    }

    /// The files no indexer covered that no `uncovered_ok` entry accepts, and
    /// the entries that accept the others: the gaps a report names and the ones
    /// it names as accepted.
    pub fn split_uncovered(&self, uncovered: &[String]) -> (Vec<String>, Vec<AcceptedGap>) {
        let open = uncovered
            .iter()
            .filter(|f| !self.is_uncovered_ok(f))
            .cloned()
            .collect();
        (open, self.accepted_gaps(uncovered))
    }

    /// The `uncovered_ok` entries that accept some of the `unread` files, each
    /// with the files it accepts, in the order of the config. A file two entries
    /// accept belongs to the first; an entry that accepts none of them is no gap.
    pub fn accepted_gaps<'a>(
        &self,
        unread: impl IntoIterator<Item = &'a String>,
    ) -> Vec<AcceptedGap> {
        let mut by_entry: std::collections::BTreeMap<usize, Vec<String>> =
            std::collections::BTreeMap::new();
        for file in unread {
            let first = self
                .uncovered_ok
                .matches(file)
                .into_iter()
                .map(|glob| self.accepting_of_glob[glob])
                .min();
            if let Some(entry) = first {
                by_entry.entry(entry).or_default().push(file.clone());
            }
        }
        by_entry
            .into_iter()
            .map(|(entry, mut files)| {
                files.sort();
                let accepting = &self.accepting[entry];
                AcceptedGap {
                    uncovered_ok: accepting.patterns.clone(),
                    because: accepting.because.clone(),
                    files,
                }
            })
            .collect()
    }

    /// Whether the top-level `forbid_external` names `$std`, which binds every file.
    pub fn forbids_std(&self) -> bool {
        self.forbidden_externals.iter().any(|f| f.module.is_none())
    }

    /// Whether the top-level `forbid_external` or a rule of `component` names `$std`.
    pub fn uses_std(&self, component: &str) -> bool {
        self.forbids_std()
            || self
                .externals
                .get(component)
                .is_some_and(|r| r.allowed_std || r.forbidden_std.is_some())
    }

    /// Why `component`, or a file of no component for `None`, may not use the
    /// external module `specifier`, imported by `file`, if it may not: the
    /// top-level `forbid_external` first, then the rules of the component.
    pub fn external_problem(
        &self,
        component: Option<&str>,
        file: &str,
        specifier: &str,
    ) -> Option<ExternalProblem<'_>> {
        for (item, forbidden) in self.forbidden_externals.iter().enumerate() {
            if !forbidden.names(file, specifier)
                || component.is_some_and(|c| forbidden.allow.iter().any(|a| a == c))
                || self.granted_in(Ban::External, item, file).is_some()
            {
                continue;
            }
            let only = if forbidden.who.is_empty() {
                String::new()
            } else {
                let who: Vec<&str> = forbidden.who.iter().map(String::as_str).collect();
                format!(" (only {} may)", crate::check::in_words(&who))
            };
            return Some(ExternalProblem {
                what: format!("is forbidden{only}"),
                because: forbidden.because.as_deref(),
                item: Some(item),
            });
        }
        let rules = self.externals.get(component?)?;
        let std = || is_std(file, specifier);
        let forbidden = |because| ExternalProblem {
            what: "is forbidden".to_string(),
            because,
            item: None,
        };
        if let Some(found) = rules.forbidden_std.as_ref().filter(|_| std()) {
            return Some(forbidden(found.because.as_deref()));
        }
        if let Some(index) =
            prefixes(file, specifier).find_map(|p| rules.forbidden.matches(p).first().copied())
        {
            return Some(forbidden(rules.reasons[index].as_deref()));
        }
        match &rules.allowed {
            Some(allowed)
                if !prefixes(file, specifier).any(|p| allowed.is_match(p))
                    && !(rules.allowed_std && std()) =>
            {
                Some(ExternalProblem {
                    what: "is not in may_use_external".to_string(),
                    because: None,
                    item: None,
                })
            }
            _ => None,
        }
    }

    /// Whether `rel_path` is in the public surface of `component`: always,
    /// unless the component declares `public` that does not cover it, or
    /// `surfaces` and no `public`.
    pub fn is_public(&self, component: &str, rel_path: &str) -> bool {
        self.public
            .get(component)
            .is_none_or(|matchers| any_match(matchers, rel_path))
    }

    /// The named surfaces of `component` that hold `rel_path`, in the order of their names.
    pub fn surfaces_holding(&self, component: &str, rel_path: &str) -> Vec<&NamedSurface> {
        self.surfaces
            .get(component)
            .into_iter()
            .flatten()
            .filter(|s| s.holds(rel_path))
            .collect()
    }

    /// Which components may import `rel_path` from outside `component`, as
    /// its surfaces say; the grants and `allow_importers` are not asked.
    pub fn access(&self, component: &str, rel_path: &str) -> Access<'_> {
        if self.is_public(component, rel_path) {
            return Access::Everyone;
        }
        Access::Only(
            self.surfaces_holding(component, rel_path)
                .into_iter()
                .flat_map(|s| s.importers.iter().map(String::as_str))
                .collect(),
        )
    }

    /// Every `in` pattern, as (component, pattern), that matches none of
    /// `files`, in the order of the config.
    pub fn unmatched_patterns(&self, files: &[String]) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .filter(|e| !files.iter().any(|f| e.matchers.is_match(f)))
            .map(|e| (e.component.as_str(), e.pattern.as_str()))
            .collect()
    }

    /// The component of a node of the graph: code only a test build has belongs to
    /// the `tests` of `tests: auto`, whatever file it sits in.
    pub fn component_of_unit(&self, unit: &Unit) -> Option<&str> {
        if self.tests && unit.is_test() {
            return Some(TESTS);
        }
        if let Some(part) = unit.part_name()
            && let Some(entry) = self.matching_part(&unit.key(), Some(part))
        {
            return Some(entry.component.as_str());
        }
        self.component_of(&unit.key())
    }

    pub fn component_of(&self, rel_path: &str) -> Option<&str> {
        self.matching(rel_path).map(|e| e.component.as_str())
    }

    /// The component of `rel_path` and the `in` pattern that put it there.
    pub fn component_and_pattern(&self, rel_path: &str) -> Option<(&str, &str)> {
        self.matching(rel_path)
            .map(|e| (e.component.as_str(), e.pattern.as_str()))
    }

    fn matching(&self, rel_path: &str) -> Option<&Entry> {
        self.matching_part(rel_path, None)
    }

    /// The entry that owns `part` of `rel_path`, or the file itself for `None`,
    /// worked out once per path and part.
    fn matching_part(&self, rel_path: &str, part: Option<&str>) -> Option<&Entry> {
        let known = self
            .owners
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(rel_path)
            .and_then(|parts| parts.iter().find(|(p, _)| p.as_deref() == part))
            .map(|(_, owner)| *owner);
        let owner = known.unwrap_or_else(|| {
            let owner = self.owner(rel_path, part);
            let mut owners = self
                .owners
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let parts = owners.entry(rel_path.to_string()).or_default();
            // Another thread may have answered it since the read.
            if !parts.iter().any(|(p, _)| p.as_deref() == part) {
                parts.push((part.map(str::to_string), owner));
            }
            owner
        });
        owner.map(|i| &self.entries[i])
    }

    /// The index of the entry that owns `part` of `rel_path`: the highest
    /// `(priority, pattern_len)` among the matching entries, the last of equals.
    fn owner(&self, rel_path: &str, part: Option<&str>) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.part.as_deref() == part)
            .filter(|(_, e)| e.matchers.is_match(rel_path))
            .filter(|(_, e)| {
                !self
                    .except
                    .get(&e.component)
                    .is_some_and(|matchers| any_match(matchers, rel_path))
            })
            .max_by_key(|(_, e)| (e.priority, e.pattern_len))
            .map(|(i, _)| i)
    }

    /// The values the named placeholders of `rel_path`'s pattern take:
    /// `feature` is `orders` for `src/features/orders/ui/View.kt` under
    /// `src/features/<feature>/ui`. A file `orders.kt` and a directory
    /// `orders/` take the same value; a path that ends above a placeholder
    /// takes none.
    pub fn captures_of(&self, rel_path: &str) -> Vec<(&str, String)> {
        let Some(entry) = self.matching(rel_path) else {
            return Vec::new();
        };
        let segments: Vec<&str> = rel_path.split('/').collect();
        entry
            .captures
            .iter()
            .filter_map(|(i, name)| {
                let segment = segments.get(*i)?;
                let value = if *i + 1 == segments.len() {
                    registry::module_path(segment)
                } else {
                    segment
                };
                Some((name.as_str(), value.to_string()))
            })
            .collect()
    }

    /// The named placeholders of every `in` pattern of `component`.
    pub fn placeholders(&self, component: &str) -> std::collections::BTreeSet<&str> {
        self.entries
            .iter()
            .filter(|e| e.component == component)
            .flat_map(|e| e.captures.iter().map(|(_, name)| name.as_str()))
            .collect()
    }

    /// For a path in an isolated component: the instance it belongs to, such
    /// as `src/pages/contacts` for `src/pages/*`. A file `contacts.tsx` and a
    /// directory `contacts/` are the same instance.
    pub fn instance_of(&self, rel_path: &str) -> Option<String> {
        let entry = self.matching(rel_path).filter(|e| e.isolated)?;
        let segments: Vec<&str> = rel_path.split('/').collect();
        let n = entry.parts.min(segments.len());
        let instance = segments[..n].join("/");
        if segments.len() == n {
            let last = segments[n - 1];
            let stem = registry::module_path(last);
            return Some(format!(
                "{}{stem}",
                &instance[..instance.len() - last.len()]
            ));
        }
        Some(instance)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn map(yaml: &str) -> ComponentMap {
        let config: Config = serde_norway::from_str(yaml).unwrap();
        ComponentMap::new(&config).unwrap()
    }

    #[test]
    fn the_last_of_equal_priority_and_pattern_length_owns_a_path() {
        // `src/*` and `src/a` are as long as each other and both own
        // `src/a/x.go`; the entries come in the order of the component names,
        // and the later entry wins whichever pattern it has.
        let owners = map("version: 1
components:
  a_wide: { in: src/* }
  b_narrow: { in: src/a }
");
        assert_eq!(owners.component_of("src/a/x.go"), Some("b_narrow"));
        // Asked again, the remembered answer is the same.
        assert_eq!(owners.component_of("src/a/x.go"), Some("b_narrow"));
        let owners = map("version: 1
components:
  a_narrow: { in: src/a }
  b_wide: { in: src/* }
");
        assert_eq!(owners.component_of("src/a/x.go"), Some("b_wide"));
        assert_eq!(owners.component_of("src/a/x.go"), Some("b_wide"));
    }

    #[test]
    fn lookups_from_several_threads_agree_with_the_entries() {
        let owners = map("version: 1
components:
  app: { in: app }
  core: { in: app/core, priority: 1 }
  web: { in: web, part: views }
");
        let paths: Vec<String> = (0..200)
            .flat_map(|i| {
                [
                    format!("app/f{i}.go"),
                    format!("app/core/f{i}.go"),
                    format!("web/f{i}.go"),
                ]
            })
            .collect();
        // Worked out from the entries, never from what the map remembers.
        let expected: Vec<_> = paths
            .iter()
            .map(|p| (owners.owner(p, None), owners.owner(p, Some("views"))))
            .collect();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for (path, expected) in paths.iter().zip(&expected) {
                        let index = |entry: Option<&Entry>| {
                            entry.map(|e| {
                                owners
                                    .entries
                                    .iter()
                                    .position(|o| std::ptr::eq(o, e))
                                    .unwrap()
                            })
                        };
                        let got = (
                            index(owners.matching_part(path, None)),
                            index(owners.matching_part(path, Some("views"))),
                        );
                        assert_eq!(&got, expected, "{path}");
                    }
                });
            }
        });
        // Each path and part was remembered once, however many threads asked.
        let remembered = owners.owners.read().unwrap();
        assert_eq!(remembered.len(), paths.len());
        assert!(remembered.values().all(|parts| parts.len() == 2));
    }

    #[test]
    fn unread_files_are_accepted_by_the_first_uncovered_ok_entry_that_names_them() {
        let map = map("version: 1
components:
  app: { in: app }
uncovered_ok:
  - { name: [gen/**, tools/**], because: generated }
  - nothing/**
  - tools/a.go
");
        let unread: Vec<String> = ["tools/a.go", "gen/b.go", "app/c.go"]
            .map(String::from)
            .into();
        // tools/a.go is accepted by two entries and belongs to the first; an
        // entry that accepts none of the files is no gap.
        assert_eq!(
            map.accepted_gaps(&unread),
            [AcceptedGap {
                uncovered_ok: vec!["gen/**".into(), "tools/**".into()],
                because: Some("generated".into()),
                files: vec!["gen/b.go".into(), "tools/a.go".into()],
            }]
        );
    }

    #[test]
    fn test_code_of_a_file_is_tests_only_under_tests_auto() {
        use crate::lang::parts::{Kind, Part};
        let unit = Unit::in_part(
            Path::new("src/store.rs"),
            Part {
                name: "cfg(test)".to_string(),
                kind: Some(Kind::Test),
            },
        );
        let own = map("version: 1
components:
  store: { in: src/store.rs }
  tests: { in: spec }
");
        assert_eq!(own.component_of_unit(&unit), Some("store"));
        let auto = map("version: 1
components:
  store: { in: src/store.rs }
tests: auto
");
        assert_eq!(auto.component_of_unit(&unit), Some("tests"));
    }

    #[test]
    fn a_component_with_a_part_owns_only_that_part_of_a_file() {
        use crate::lang::parts::Part;
        let m = map("version: 1
components:
  crate: { in: crates/a/Cargo.toml }
  fakes: { in: 'crates/**/Cargo.toml', part: feature/fake }
");
        let part = |name: &str| {
            Unit::in_part(
                Path::new("crates/a/Cargo.toml"),
                Part {
                    name: name.to_string(),
                    kind: None,
                },
            )
        };
        assert_eq!(m.component_of_unit(&part("feature/fake")), Some("fakes"));
        assert_eq!(m.component_of_unit(&part("feature/other")), Some("crate"));
        // The file itself is not the part: another crate's manifest has no owner.
        assert_eq!(m.component_of("crates/a/Cargo.toml"), Some("crate"));
        assert_eq!(m.component_of("crates/b/Cargo.toml"), None);
    }

    #[test]
    fn a_pattern_matches_its_directory_or_a_file_of_its_name() {
        let m = map("version: 1
components:
  cli: { in: src/app/cli }
  pages: { in: 'src/pages/*' }
  ghost: { in: src/gone }
");
        let files = ["src/app/cli.py".to_string(), "src/pages/a/x.ts".to_string()];
        assert_eq!(m.unmatched_patterns(&files), vec![("ghost", "src/gone")]);
    }

    #[test]
    fn placeholders_capture_and_do_not_win_on_length() {
        let m = map("version: 1
components:
  slice: { in: 'src/features/<feature>' }
  ui: { in: 'src/features/<feature>/ui' }
  wild: { in: 'src/apps/*' }
  named: { in: 'src/apps/<app>' }
");
        assert_eq!(
            m.captures_of("src/features/orders/ui/View.kt"),
            vec![("feature", "orders".to_string())]
        );
        // A file and a directory of one name are one slice.
        assert_eq!(
            m.captures_of("src/features/orders.kt"),
            vec![("feature", "orders".to_string())]
        );
        assert!(m.captures_of("src/features").is_empty());
        // `<app>` is longer as text than `*`, but the patterns are equal.
        assert_eq!(m.component_of("src/apps/a/x.ts"), Some("wild"));
    }

    #[test]
    fn std_keeps_the_reasons_of_the_other_items_in_place() {
        let m = map("version: 1
components:
  core: { in: core }
rules:
  core:
    may_use_external: [$std, okhttp3]
    forbid_external:
      - { name: $std, because: std is wrapped }
      - { name: okhttp3.internal, because: internals change }
");
        assert!(m.uses_std("core"));
        let reason = |file, module| {
            m.external_problem(Some("core"), file, module)
                .map(|p| p.because)
        };
        assert_eq!(
            reason("A.kt", "java.util.UUID"),
            Some(Some("std is wrapped"))
        );
        assert_eq!(
            reason("A.kt", "okhttp3.internal.X"),
            Some(Some("internals change"))
        );
        assert_eq!(reason("A.kt", "okhttp3.Client"), None);
        assert!(
            m.external_problem(Some("core"), "A.kt", "retrofit2.Call")
                .is_some()
        );
    }

    #[test]
    fn matches_dirs_files_and_prefers_specific() {
        let m = map("version: 1
components:
  handler: { in: 'internal/handlers/*' }
  service: { in: 'internal/services/**' }
  domain: { in: internal/domain }
  repo: { in: 'internal/domain/*/repository' }
");
        assert_eq!(m.component_of("internal/handlers/books"), Some("handler"));
        assert_eq!(
            m.component_of("internal/handlers/books/h.go"),
            Some("handler")
        );
        assert_eq!(
            m.component_of("internal/services/a/b/c.go"),
            Some("service")
        );
        assert_eq!(m.component_of("internal/domain"), Some("domain"));
        assert_eq!(
            m.component_of("internal/domain/books/model.go"),
            Some("domain")
        );
        assert_eq!(
            m.component_of("internal/domain/books/repository"),
            Some("repo")
        );
        assert_eq!(
            m.component_of("internal/domain/books/repository/pg.go"),
            Some("repo")
        );
        assert_eq!(m.component_of("internal/domain.go"), Some("domain"));
        assert_eq!(m.component_of("internal/domainx/a.go"), None);
        assert_eq!(m.component_of("cmd/main.go"), None);
    }

    #[test]
    fn external_rules_match_prefixes() {
        let m = map("version: 1
components:
  domain: { in: src/domain }
  ui: { in: src/ui }
rules:
  domain: { forbid_external: [moment, '@acme/*'] }
  ui: { may_use_external: [react, java.util] }
");
        assert!(
            m.external_problem(Some("domain"), "x.ts", "moment/locale/ru")
                .is_some()
        );
        assert!(
            m.external_problem(Some("domain"), "x.ts", "@acme/sdk/v2")
                .is_some()
        );
        assert!(
            m.external_problem(Some("domain"), "x.ts", "lodash")
                .is_none()
        );
        assert!(
            m.external_problem(Some("ui"), "x.ts", "react-dom")
                .is_some()
        );
        assert!(
            m.external_problem(Some("ui"), "x.ts", "react/jsx-runtime")
                .is_none()
        );
        assert!(
            m.external_problem(Some("ui"), "X.java", "java.util.List")
                .is_none()
        );
        assert!(
            m.external_problem(Some("ui"), "X.java", "java.io.File")
                .is_some()
        );
        assert!(
            m.external_problem(Some("other"), "x.ts", "anything")
                .is_none()
        );
        assert_eq!(
            prefixes("src/lib.rs", "serde::de::Deserialize").collect::<Vec<_>>(),
            ["serde", "serde::de", "serde::de::Deserialize"]
        );
        assert_eq!(
            prefixes("src/A.php", "Illuminate\\Support\\Str").collect::<Vec<_>>(),
            [
                "Illuminate",
                "Illuminate\\Support",
                "Illuminate\\Support\\Str"
            ]
        );
    }

    /// A module is cut where its own language cuts it: a dot is part of an npm
    /// or a Go name, so `lodash` is not `lodash.debounce` and `golang` is not
    /// `golang.org/x/sync`.
    #[test]
    fn a_module_is_cut_at_the_separators_of_its_language() {
        let m = map("version: 1
components:
  web: { in: web }
rules:
  web: { forbid_external: [lodash, golang, java] }
");
        assert!(
            m.external_problem(Some("web"), "web/a.ts", "lodash.debounce")
                .is_none()
        );
        assert!(
            m.external_problem(Some("web"), "web/package.json", "lodash.debounce")
                .is_none()
        );
        assert!(
            m.external_problem(Some("web"), "web/a.ts", "lodash/fp")
                .is_some()
        );
        assert!(
            m.external_problem(Some("web"), "web/a.go", "golang.org/x/sync")
                .is_none()
        );
        assert!(
            m.external_problem(Some("web"), "web/A.java", "java.util.List")
                .is_some()
        );
        assert!(!ComponentMap::external_matches(
            "lodash",
            [("web/a.ts", "lodash.debounce")]
        ));
        assert!(ComponentMap::external_matches(
            "lodash",
            [("web/a.ts", "lodash/fp")]
        ));
    }

    #[test]
    fn priority_beats_length_and_except_hands_files_on() {
        let m = map("version: 1
components:
  feature: { in: src/deep/feature, except: '**/*.stories.tsx' }
  src: { in: src }
  tests: { in: '**/*.test.ts', priority: 1 }
  long: { in: src/deep/feature/long/path }
");
        // `feature` gives its stories up, so the shorter `src` gets them.
        assert_eq!(m.component_of("src/deep/feature/a.ts"), Some("feature"));
        assert_eq!(
            m.component_of("src/deep/feature/a.stories.tsx"),
            Some("src")
        );
        // A priority outranks a longer pattern.
        assert_eq!(
            m.component_of("src/deep/feature/long/path/x.ts"),
            Some("long")
        );
        assert_eq!(
            m.component_of("src/deep/feature/long/path/x.test.ts"),
            Some("tests")
        );
    }

    #[test]
    fn instances_of_isolated_components() {
        let m = map("version: 1
components:
  pages: { in: 'src/pages/*', isolated: true }
  plain: { in: src/plain }
");
        let instance = |p: &str| m.instance_of(p);
        assert_eq!(
            instance("src/pages/contacts/index.ts").as_deref(),
            Some("src/pages/contacts")
        );
        assert_eq!(
            instance("src/pages/contacts").as_deref(),
            Some("src/pages/contacts")
        );
        // A file and a directory of the same name are one instance.
        assert_eq!(
            instance("src/pages/contacts.tsx").as_deref(),
            Some("src/pages/contacts")
        );
        // So are a declaration file and its directory (#107).
        assert_eq!(
            instance("src/pages/contacts.d.ts").as_deref(),
            Some("src/pages/contacts")
        );
        assert_eq!(instance("src/plain/a.ts"), None);
        assert_eq!(instance("src/elsewhere/a.ts"), None);
    }

    #[test]
    fn unowned_covers_directories() {
        let m = map("version: 1
components:
  a: { in: a }
unowned: [gen, 'tools/*.ts']
");
        assert!(m.is_unowned("gen/x/y.ts"));
        assert!(m.is_unowned("tools/x.ts"));
        assert!(!m.is_unowned("other/x.ts"));
    }

    #[test]
    fn public_api() {
        let m = map("version: 1
components:
  billing: { in: src/billing, public: [src/billing/index, src/billing/api] }
  shop: { in: src/shop }
");
        assert!(m.is_public("billing", "src/billing/index.ts"));
        assert!(m.is_public("billing", "src/billing/api/types.ts"));
        assert!(!m.is_public("billing", "src/billing/internal/db.ts"));
        assert!(m.is_public("shop", "src/shop/anything.ts"));
    }
}
