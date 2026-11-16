//! The symbol rules: `forbid_symbol` and `public_symbols` judge the names a file
//! uses, members included, and say what they could not judge.

use std::collections::{BTreeMap, BTreeSet};

use crate::check::{Place, RuleId, SymbolRef, Unjudged, Violation, in_words, name_first};
use crate::components::ComponentMap;
use anyhow::{Result, bail};

use crate::config::{Ban, Config, ForbiddenName};
use crate::lang::{Dep, Target, registry};
use crate::policy::Policy;
use crate::relpath::slash;

/// `forbid_symbol` and `public_symbols`: names used through imports.
pub fn symbols(config: &Config, map: &ComponentMap, uses: &[SymbolRef]) -> Vec<Violation> {
    let mut found = Vec::new();
    for used in uses {
        let file = slash(&used.file);
        let owner = map.component_of_unit(&used.from);
        let defined_by = match &used.target {
            Some(Target::Internal(unit)) => map.component_of_unit(unit),
            _ => None,
        };
        let called = std::iter::once((used.specifier.as_str(), used.name.as_str()))
            .chain(
                used.method
                    .as_ref()
                    .map(|(owner, method)| (owner.as_str(), method.as_str())),
            )
            .collect();
        let judged = judge(
            config,
            map,
            &Use {
                file: &file,
                owner,
                defined_by,
                called,
                public_as: vec![&used.name],
                name: &used.name,
                target: format!("{}.{}", used.specifier, used.name),
            },
        );
        found.extend(judged.found.into_iter().map(|said| Violation {
            evidence: vec![Place::new(file.clone(), used.line)],
            file: file.clone(),
            line: used.line,
            message: said.message,
            because: said.because,
            // A file that belongs to no component is judged by the top-level list alone.
            from: owner.unwrap_or_default().to_string(),
            to: said.to,
            target: said.target,
            specifier: used.specifier.clone(),
            ..Violation::of(RuleId::Symbol)
        }));
    }
    found
}

/// One use of a name, as the symbol rules judge it: by the paths and the
/// components alone, so a use the code does not have yet (a policy example) is
/// judged as the scan judges one it has.
pub struct Use<'a> {
    /// The file that uses the name, relative to the root.
    pub file: &'a str,
    /// The component of the code that uses it; none for a file of no component.
    pub owner: Option<&'a str>,
    /// The component that defines the name, when one is known to.
    pub defined_by: Option<&'a str>,
    /// What `forbid_symbol` may call the name, each a qualifier and a name: the
    /// name itself, and a member also as the type it is used through.
    pub called: Vec<(&'a str, &'a str)>,
    /// The names `public_symbols` may list it as: the name, or the type it is a member of.
    pub public_as: Vec<&'a str>,
    /// The name a `public_symbols` violation tells.
    pub name: &'a str,
    /// The name as a violation's target tells it.
    pub target: String,
}

impl<'a> Use<'a> {
    /// The use of `symbol` in `file`, written as `forbid_symbol` writes a name:
    /// `example.com/app/ent.Client`, and a member after its type, `ent.Client.Query`
    /// or `db::client::Client.query`. A ban on the member or on its type names a
    /// member, and `public_symbols` judges its type, as for a use the scan finds.
    /// Where the language cannot tell a member from a name under a module without
    /// the index (see [`registry::MemberType`]), the name is read both ways: a ban
    /// on either names it, and `public_symbols` listing either lets it through.
    /// `None` when `symbol` has no qualifier.
    pub fn written(
        file: &'a str,
        owner: Option<&'a str>,
        defined_by: Option<&'a str>,
        symbol: &'a str,
    ) -> Option<Self> {
        let (qualifier, name) = symbol.rsplit_once('.')?;
        let ty = registry::member_type(file, qualifier);
        let public_as = match ty {
            Some(ty) if ty.certain => vec![ty.name],
            Some(ty) => vec![name, ty.name],
            None => vec![name],
        };
        Some(Use {
            file,
            owner,
            defined_by,
            called: std::iter::once((qualifier, name))
                .chain(ty.map(|ty| (ty.qualifier, ty.name)))
                .collect(),
            name: public_as[0],
            public_as,
            target: symbol.to_string(),
        })
    }

    /// Whether `pattern`, a `forbid_symbol` name, names the name used.
    fn is(&self, pattern: &str) -> bool {
        self.called
            .iter()
            .any(|(specifier, name)| names_symbol(pattern, specifier, name))
    }
}

/// What the symbol rules say of one use.
pub struct Judged {
    /// One for each rule item that forbids it.
    pub found: Vec<Found>,
    /// The grant that lets in a name an item forbids, when one does:
    /// `forbid_symbol allow` or `forbid_symbol allow_in`.
    pub granted: Option<&'static str>,
}

/// A symbol rule that forbids a use.
pub struct Found {
    pub message: String,
    pub because: Option<String>,
    /// The component that defines the name, for `public_symbols`; else empty.
    pub to: String,
    pub target: String,
}

/// The decision of `forbid_symbol` (the top-level list and that of the user's
/// component) and of the `public_symbols` of the component that defines the name.
pub fn judge(config: &Config, map: &ComponentMap, used: &Use) -> Judged {
    let mut found = Vec::new();
    let mut granted = None;
    let owner = used.owner;
    let from = owner.unwrap_or_default();
    let own = owner
        .and_then(|component| config.rules.get(component))
        .into_iter()
        .flat_map(|rule| {
            rule.forbid_symbol
                .iter()
                .map(move |listed| (listed, rule.because.as_deref(), None))
        });
    let global = config
        .forbid_symbol
        .iter()
        .enumerate()
        .map(|(item, listed)| (listed, None, Some(item)));
    let mut seen = BTreeSet::new();
    for (listed, fallback, item) in global.chain(own) {
        let pattern = listed.name();
        if !used.is(pattern) {
            continue;
        }
        // A component the item allows may use the name.
        if owner.is_some_and(|component| listed.allows(component)) {
            granted.get_or_insert("forbid_symbol allow");
            continue;
        }
        // So may a file in a path the item allows, whatever owns it.
        if item.is_some_and(|item| map.granted_in(Ban::Symbol, item, used.file).is_some()) {
            granted.get_or_insert("forbid_symbol allow_in");
            continue;
        }
        // A name in both lists is one violation.
        if !seen.insert(pattern) {
            continue;
        }
        let who = crate::check::who(from);
        let allowed: Vec<&str> = listed
            .allow()
            .into_iter()
            .chain(listed.allow_in())
            .flatten()
            .map(String::as_str)
            .collect();
        let only = if allowed.is_empty() {
            String::new()
        } else {
            format!(" (only {} may)", in_words(&allowed))
        };
        found.push(Found {
            message: format!("{who} must not use {pattern}{only}"),
            because: listed.because().or(fallback).map(str::to_string),
            to: String::new(),
            target: pattern.to_string(),
        });
    }
    if owner.is_some()
        && let Some(to) = used.defined_by
        && to != from
        && let Some(public) = &config.components[to].public_symbols
        && !used
            .public_as
            .iter()
            .any(|name| public.iter().any(|p| p == name))
    {
        found.push(Found {
            message: format!(
                "{from} -> {to}: {} is not in the public symbols of {to} ({})",
                used.name,
                public.join(", ")
            ),
            because: config.components[to].because.clone(),
            to: to.to_string(),
            target: used.target.clone(),
        });
    }
    Judged { found, granted }
}

/// How the own `forbid_symbol` of `component` writes `symbol`, when it lists a
/// name that can name the same symbol: empty when spelled alike, else
/// `` as `own` ``, to follow `forbids` in an error.
pub fn own_ban(config: &Config, component: &str, symbol: &str) -> Option<String> {
    let own = config
        .rules
        .get(component)?
        .forbid_symbol
        .iter()
        .map(ForbiddenName::name)
        .find(|own| may_name_the_same(own, symbol))?;
    Some(if own == symbol {
        String::new()
    } else {
        format!(" as `{own}`")
    })
}

/// Fails when an `allow_in` pattern of the top-level `forbid_symbol` matches a
/// file of a component whose own `forbid_symbol` lists the same name: the
/// component's list still forbids it there, so the grant says what never holds.
/// A file belongs to the component that owns it and to the component of each
/// part of it that `uses` use names from (test code, a part of a manifest).
/// Only the files tell, so it is found when the policy is judged, not when it
/// is read.
pub fn contradicted_grants(
    config: &Config,
    map: &ComponentMap,
    files: &[String],
    uses: &[SymbolRef],
) -> Result<()> {
    let mut parts: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for used in uses {
        if let Some(component) = map.component_of_unit(&used.from) {
            parts
                .entry(slash(&used.file))
                .or_default()
                .insert(component);
        }
    }
    for (i, listed) in config.forbid_symbol.iter().enumerate() {
        let symbol = listed.name();
        // The first pattern, then the first file by path, whatever order the walk took.
        let first = files
            .iter()
            .filter_map(|file| {
                let (at, pattern) = map.granted_in(Ban::Symbol, i, file)?;
                let (component, spelled) = map
                    .component_of(file)
                    .into_iter()
                    .chain(parts.get(file).into_iter().flatten().copied())
                    .find_map(|component| Some((component, own_ban(config, component, symbol)?)))?;
                Some((at, file, pattern, component, spelled))
            })
            .min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        if let Some((_, file, pattern, component, spelled)) = first {
            bail!(
                "forbid_symbol[{i}].allow_in lets `{pattern}` use `{symbol}`, but {file} there belongs to `{component}`, whose rules.{component}.forbid_symbol forbids it{spelled}"
            );
        }
    }
    Ok(())
}

/// Why an index left the package of some names unnamed, as the code that reads
/// the index knows it.
pub struct Unnamed {
    /// What it could not name: `the npm package`.
    pub what: &'static str,
    /// Why, and how to have it named.
    pub why: &'static str,
}

/// Why `forbid_symbol` left uses of names unchecked: the files in `unnamed`
/// use names whose package the index left unnamed, for the reason `cause` gives.
/// Only files that a `forbid_symbol` list covers count: all of them for the top-level
/// list, else those of a component with its own.
pub fn symbols_partial(
    config: &Config,
    map: &ComponentMap,
    unnamed: &[String],
    cause: &Unnamed,
) -> Vec<Unjudged> {
    let unnamed: Vec<&str> = unnamed
        .iter()
        .map(String::as_str)
        .filter(|file| forbids_symbols(config, map, file))
        .collect();
    if unnamed.is_empty() {
        return Vec::new();
    }
    const SHOWN: usize = 3;
    let files = name_first(
        &unnamed,
        SHOWN,
        ", ",
        |file| file.to_string(),
        |more| format!(" and {more} more files"),
    );
    vec![
        Unjudged::new(format!(
            "`forbid_symbol` cannot tell {} of the names used in {files}: {}",
            cause.what, cause.why
        ))
        .in_files(unnamed),
    ]
}

/// Whether a `forbid_symbol` list covers `file`: the top-level one, or that of
/// its component.
fn forbids_symbols(config: &Config, map: &ComponentMap, file: &str) -> bool {
    !config.forbid_symbol.is_empty()
        || map
            .component_of(file)
            .and_then(|component| config.rules.get(component))
            .is_some_and(|rule| !rule.forbid_symbol.is_empty())
}

/// The files among `files` that a symbol rule applies to although the registry
/// says their language has no verified uses of names: the rule passes there
/// without judging anything. One per rule and language.
///
/// `forbid_symbol` applies to every file when the top-level list is set, else to
/// the files of a component with its own; `public_symbols` to the files of a
/// component that depend on, or use a name of, another component that has it.
/// `uses` are the uses of names of those languages, which no rule reads.
pub fn symbols_unverified(
    config: &Config,
    map: &ComponentMap,
    files: &[&str],
    deps: &[Dep],
    uses: &[SymbolRef],
) -> Vec<Unjudged> {
    // The language of a file whose uses of names easement does not read.
    let unverified = |file: &str| registry::of(file).filter(|language| !language.symbols);
    let mut found: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
    for file in files.iter().copied() {
        if let Some(language) = unverified(file)
            && forbids_symbols(config, map, file)
        {
            found
                .entry(("forbid_symbol", language.name))
                .or_default()
                .push(file);
        }
    }
    let files: BTreeSet<&str> = files.iter().copied().collect();
    let reached = deps
        .iter()
        .map(|dep| (&dep.file, &dep.from, Some(&dep.target)))
        .chain(
            uses.iter()
                .map(|used| (&used.file, &used.from, used.target.as_ref())),
        );
    for (file, from, target) in reached {
        let file = slash(file);
        let Some(&file) = files.get(file.as_str()) else {
            continue;
        };
        let Some(language) = unverified(file) else {
            continue;
        };
        let Some(from) = map.component_of_unit(from) else {
            continue;
        };
        let Some(Target::Internal(unit)) = target else {
            continue;
        };
        if let Some(to) = map.component_of_unit(unit)
            && to != from
            && config.components[to].public_symbols.is_some()
        {
            found
                .entry(("public_symbols", language.name))
                .or_default()
                .push(file);
        }
    }
    found
        .into_iter()
        .map(|((rule, language), mut files)| {
            files.sort_unstable();
            files.dedup();
            Unjudged::new(format!(
                "`{rule}` cannot judge the names used in {} {language} files: easement does not read uses of names from {language}, so those uses were not checked",
                files.len()
            ))
            .in_files(files)
        })
        .collect()
}

/// `forbid_symbol` names that match no symbol of the index while one of the same
/// name sits deeper under their qualifier (`I18n.locale`, defined as
/// `I18n::Base.locale`): misspelled, they can never fire, so they fail as
/// `unmatched` at their line, as an `in` pattern that matches no file does. A name the
/// index has never seen is left alone: banning what the code does not use yet is
/// what the rule is for. Only a whole index can tell; `complete` says it is one.
pub fn unmatched_symbols(
    policy: &Policy,
    known: &[(String, String)],
    complete: bool,
    config_file: &str,
) -> Vec<Violation> {
    never_firing(&policy.config, known, complete)
        .into_iter()
        .map(|(component, pattern, meant)| Violation {
            file: config_file.to_string(),
            line: match component {
                Some(from) => policy.config_line(&["rules", from, "forbid_symbol"], &pattern),
                None => policy.config_line(&["forbid_symbol"], &pattern),
            },
            message: format!(
                "`forbid_symbol` `{pattern}` of {} matches no symbol of the index, so it can never fire; did you mean `{meant}`?",
                component.unwrap_or("the top level")
            ),
            from: component.unwrap_or_default().to_string(),
            target: pattern,
            ..Violation::of(RuleId::Unmatched)
        })
        .collect()
}

/// The `forbid_symbol` names that can never fire, each with its component (none
/// for the top level) and the name it probably means.
fn never_firing<'a>(
    config: &'a Config,
    known: &[(String, String)],
    complete: bool,
) -> Vec<(Option<&'a str>, String, String)> {
    if !complete {
        return Vec::new();
    }
    let mut found = Vec::new();
    let lists = std::iter::once((None, &config.forbid_symbol)).chain(
        config
            .rules
            .iter()
            .map(|(component, rule)| (Some(component.as_str()), &rule.forbid_symbol)),
    );
    for (component, list) in lists {
        for listed in list {
            let pattern = listed.name();
            let Some((qualifier, wanted)) = pattern.rsplit_once('.') else {
                continue;
            };
            if known
                .iter()
                .any(|(specifier, name)| names_symbol(pattern, specifier, name))
            {
                continue;
            }
            // The name sits deeper under the qualifier, or the qualifier names its
            // module with a file suffix left on (`types.d` of `types.d.ts`).
            let meant = known
                .iter()
                .filter(|(_, name)| name == wanted)
                .map(|(specifier, _)| specifier)
                .find(|specifier| {
                    registry::nests(qualifier, specifier)
                        || registry::spells_with_suffix(qualifier, specifier)
                });
            if let Some(specifier) = meant {
                found.push((
                    component,
                    pattern.to_string(),
                    format!("{specifier}.{wanted}"),
                ));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Whether two `forbid_symbol` names can name one symbol: the same name under
/// the same qualifier, or under one that ends the other (`tags.DeleteIn` and
/// `example.com/app/tags.DeleteIn`).
pub fn may_name_the_same(a: &str, b: &str) -> bool {
    match (a.rsplit_once('.'), b.rsplit_once('.')) {
        (Some((a_qualifier, a_name)), Some((b_qualifier, b_name))) => {
            names_symbol(a, b_qualifier, b_name) || names_symbol(b, a_qualifier, a_name)
        }
        _ => a == b,
    }
}

/// Whether `pattern` (`pkg.Name` or `import/path.Name`) names the symbol
/// `name` used through the import `specifier`.
fn names_symbol(pattern: &str, specifier: &str, name: &str) -> bool {
    let Some((qualifier, wanted)) = pattern.rsplit_once('.') else {
        return false;
    };
    wanted == name
        && (qualifier == specifier
            || specifier
                .strip_suffix(qualifier)
                .is_some_and(|rest| rest.ends_with('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forbidding(names: &[&str]) -> Config {
        let list: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
        serde_norway::from_str(&format!(
            "version: 1
components:
  web: {{ in: web }}
rules:
  web: {{ forbid_symbol: [{}] }}
",
            list.join(", ")
        ))
        .unwrap()
    }

    /// The first contradiction is told whatever order the files are walked in:
    /// the first pattern of the item, then the first file by path.
    #[test]
    fn a_contradicted_grant_is_told_the_same_in_any_order_of_files() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  web: { in: web }
rules:
  web: { forbid_symbol: [d.T] }
forbid_symbol: [{ name: d.T, allow_in: [web/b, web/a] }]
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let said = |files: &[&str]| {
            let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
            contradicted_grants(&config, &map, &files, &[])
                .unwrap_err()
                .to_string()
        };
        let first = "forbid_symbol[0].allow_in lets `web/b` use `d.T`, but web/b/x.go there belongs to `web`, whose rules.web.forbid_symbol forbids it";
        assert_eq!(said(&["web/a/x.go", "web/b/y.go", "web/b/x.go"]), first);
        assert_eq!(said(&["web/b/x.go", "web/b/y.go", "web/a/x.go"]), first);
    }

    /// A name an example writes is read as `forbid_symbol` reads a use: a member
    /// also as the type it is used through, whether its language joins them with
    /// `.` or `::`; a module path is never cut into a type.
    #[test]
    fn a_written_name_is_read_with_the_type_of_a_member() {
        let read = |symbol| {
            let used = Use::written("web/a.go", Some("web"), None, symbol).unwrap();
            (used.called, used.public_as)
        };
        assert_eq!(
            read("example.com/app/ent.Client"),
            (vec![("example.com/app/ent", "Client")], vec!["Client"])
        );
        assert_eq!(
            read("example.com/app/ent.Client.Query"),
            (
                vec![
                    ("example.com/app/ent.Client", "Query"),
                    ("example.com/app/ent", "Client")
                ],
                vec!["Client"]
            )
        );
        let read_in = |file, symbol| {
            let used = Use::written(file, Some("web"), None, symbol).unwrap();
            (used.called, used.public_as)
        };
        assert_eq!(
            read_in("web/a.rs", "db::client::Client.query"),
            (
                vec![("db::client::Client", "query"), ("db::client", "Client")],
                vec!["query", "Client"]
            )
        );
        // Go joins a member with `.`, not `::`; a file of no language reads no type.
        assert_eq!(
            read("db::client::Client.query").0,
            [("db::client::Client", "query")]
        );
        assert_eq!(
            read_in("web/a.txt", "ent.Client.Query").0,
            [("ent.Client", "Query")]
        );
        assert!(Use::written("web/a.go", None, None, "Client").is_none());
    }

    #[test]
    fn who_may_use_a_name_reads_as_a_sentence() {
        assert_eq!(in_words(&["usecase"]), "usecase");
        assert_eq!(in_words(&["usecase", "wiring"]), "usecase and wiring");
        assert_eq!(
            in_words(&["wiring", "runner", "internal/x"]),
            "wiring, runner and internal/x"
        );
    }

    fn known(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(q, n)| (q.to_string(), n.to_string()))
            .collect()
    }

    #[test]
    fn a_name_defined_only_deeper_under_its_qualifier_is_flagged_with_the_candidate() {
        let config = forbidding(&["I18n.locale"]);
        let found = never_firing(&config, &known(&[("I18n::Base", "locale")]), true);
        assert_eq!(
            found,
            [(
                Some("web"),
                "I18n.locale".to_string(),
                "I18n::Base.locale".to_string()
            )]
        );
    }

    /// A qualifier with a file suffix left on names no module: `types.d` was the
    /// name of `types.d.ts` before a declaration file was named by its module (#107).
    #[test]
    fn a_qualifier_spelled_with_a_file_suffix_is_flagged_with_its_module() {
        let known = known(&[("src/types", "Foo"), ("pkg/foo", "Thing")]);
        for (pattern, meant) in [
            ("src/types.d.Foo", "src/types.Foo"),
            ("src/types.d.ts.Foo", "src/types.Foo"),
            ("pkg/foo.cjs.Thing", "pkg/foo.Thing"),
        ] {
            let config = forbidding(&[pattern]);
            assert_eq!(
                never_firing(&config, &known, true),
                [(Some("web"), pattern.to_string(), meant.to_string())],
                "{pattern}"
            );
        }
    }

    #[test]
    fn a_name_that_matches_or_that_nothing_defines_stays_silent() {
        let known = known(&[("I18n::Base", "locale"), ("log", "Println")]);
        for pattern in [
            // Matches exactly.
            "I18n::Base.locale",
            // Banned before anyone uses it, with another package of the same name around.
            "fmt.Println",
            // Not a nested namespace of the qualifier.
            "Time.locale",
            // Not defined at all.
            "I18n.deploy",
            // A suffix no language drops from a file name.
            "I18n::Base.x.locale",
        ] {
            let config = forbidding(&[pattern]);
            assert!(never_firing(&config, &known, true).is_empty(), "{pattern}");
        }
    }

    #[test]
    fn unjudged_names_say_why_the_index_left_them_unnamed() {
        let config = forbidding(&["i18next.i18next"]);
        let map = ComponentMap::new(&config).unwrap();
        let cause = Unnamed {
            what: "the package",
            why: "update the indexer",
        };
        let notes = symbols_partial(&config, &map, &["web/a.ts".to_string()], &cause);
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].reason,
            "`forbid_symbol` cannot tell the package of the names used in web/a.ts: update the indexer"
        );
        assert_eq!(notes[0].files, ["web/a.ts"]);
    }

    #[test]
    fn public_symbols_over_files_without_verified_uses_is_a_gap() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  core: { in: core, public_symbols: [Order] }
  web: { in: web }
  api: { in: api }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let dep = |from: &str, to: &str| Dep::module(from, 1, "", Target::Internal(to.into()));
        let deps = [
            dep("web/A.kt", "core/Order.kt"),
            dep("web/B.kt", "web/A.kt"),
            dep("api/c.go", "core/Order.kt"),
            dep("core/Repo.kt", "core/Order.kt"),
        ];
        // A method called on a value of `core`'s type, in a file that writes no name of
        // `core`: a use of a name, and no dependency.
        let member = SymbolRef {
            file: "web/W.java".into(),
            line: 4,
            from: "web/W.java".into(),
            specifier: "core".into(),
            name: "Order".into(),
            method: None,
            target: Some(Target::Internal("core/Order.java".into())),
        };
        let files = [
            "web/A.kt",
            "web/B.kt",
            "api/c.go",
            "core/Repo.kt",
            "web/W.java",
        ];
        let notes = symbols_unverified(&config, &map, &files, &deps, &[member]);
        // Only a kotlin file that imports `core` from another component, and a java file
        // that uses a name of it, are unjudged: Go uses are read, and `public_symbols`
        // binds no file of its own component.
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(notes[0].files, ["web/W.java"]);
        assert_eq!(notes[1].files, ["web/A.kt"]);
        assert!(
            notes[1].reason.contains("`public_symbols`") && notes[1].reason.contains("kotlin"),
            "{notes:?}"
        );
        // Without a symbol rule nothing is unjudged.
        let plain: Config = serde_norway::from_str(
            "version: 1\ncomponents:\n  core: { in: core }\n  web: { in: web }\n",
        )
        .unwrap();
        let map = ComponentMap::new(&plain).unwrap();
        assert!(symbols_unverified(&plain, &map, &files, &deps, &[]).is_empty());
    }

    #[test]
    fn nothing_is_said_when_the_index_is_not_whole() {
        let config = forbidding(&["I18n.locale"]);
        let known = known(&[("I18n::Base", "locale")]);
        assert!(never_firing(&config, &known, false).is_empty());
    }

    #[test]
    fn a_symbol_used_in_test_code_is_judged_as_tests_not_as_the_file() {
        use crate::lang::parts::{Kind, Part};
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  store: { in: src/store.rs }
tests: auto
rules:
  store: { forbid_symbol: [testkit.Clock] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let used = |from: crate::lang::Unit| SymbolRef {
            file: "src/store.rs".into(),
            from,
            line: 3,
            specifier: "testkit".into(),
            name: "Clock".into(),
            method: None,
            target: Some(Target::External("testkit".into())),
        };
        let test_code = crate::lang::Unit::in_part(
            std::path::Path::new("src/store.rs"),
            Part {
                name: "cfg(test)".into(),
                kind: Some(Kind::Test),
            },
        );
        assert_eq!(
            symbols(
                &config,
                &map,
                &[used(crate::lang::Unit::from("src/store.rs"))]
            )
            .len(),
            1
        );
        assert!(symbols(&config, &map, &[used(test_code)]).is_empty());
    }
}
