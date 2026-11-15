//! The symbol rules: `forbid_symbol` and `public_symbols` judge the names a file
//! uses, members included, and say what they could not judge.

use std::collections::{BTreeMap, BTreeSet};

use crate::check::{Place, RuleId, SymbolRef, Unjudged, Violation, name_first};
use crate::components::ComponentMap;
use anyhow::{Result, bail};

use crate::config::{Config, ForbiddenName, TESTS};
use crate::lang::{Dep, Target, registry};
use crate::policy::Policy;
use crate::relpath::slash;

/// `forbid_symbol` and `public_symbols`: names used through imports.
pub fn symbols(config: &Config, map: &ComponentMap, uses: &[SymbolRef]) -> Vec<Violation> {
    let mut found = Vec::new();
    for used in uses {
        let file = slash(&used.file);
        let owner = map.component_of_unit(&used.from);
        // A file that belongs to no component is judged by the top-level list alone.
        let from = owner.unwrap_or_default();
        let mut report = |message: String, target: String, to: &str, because: Option<&str>| {
            found.push(Violation {
                evidence: vec![Place::new(file.clone(), used.line)],
                file: file.clone(),
                line: used.line,
                message,
                because: because.map(str::to_string),
                from: from.to_string(),
                to: to.to_string(),
                target,
                specifier: used.specifier.clone(),
                ..Violation::of(RuleId::Symbol)
            });
        };
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
        let mut seen = std::collections::BTreeSet::new();
        for (listed, fallback, item) in global.chain(own) {
            let pattern = listed.name();
            if names_symbol(pattern, &used.specifier, &used.name)
                || used
                    .method
                    .as_ref()
                    .is_some_and(|(owner, method)| names_symbol(pattern, owner, method))
            {
                // A component the item allows may use the name.
                if owner.is_some_and(|component| listed.allows(component)) {
                    continue;
                }
                // So may a file in a path the item allows, whatever owns it.
                if item.is_some_and(|item| map.granted_in(item, &file).is_some()) {
                    continue;
                }
                // A name in both lists is one violation.
                if !seen.insert(pattern) {
                    continue;
                }
                let who = match owner {
                    Some(component) => component.to_string(),
                    None => "unowned files".to_string(),
                };
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
                    format!(" (only {} may)", allowed.join(", "))
                };
                report(
                    format!("{who} must not use {pattern}{only}"),
                    pattern.to_string(),
                    "",
                    listed.because().or(fallback),
                );
            }
        }
        if owner.is_some()
            && let Some(Target::Internal(unit)) = &used.target
            && let Some(to) = map.component_of_unit(unit)
            && to != from
            && let Some(public) = &config.components[to].public_symbols
            && !public.contains(&used.name)
        {
            report(
                format!(
                    "{from} -> {to}: {} is not in the public symbols of {to} ({})",
                    used.name,
                    public.join(", ")
                ),
                format!("{}.{}", used.specifier, used.name),
                to,
                config.components[to].because.as_deref(),
            );
        }
    }
    found
}

/// Fails when an `allow_in` pattern of the top-level `forbid_symbol` matches a
/// file of a component whose own `forbid_symbol` lists the same name: the
/// component's list still forbids it there, so the grant says what never holds.
/// Only the files tell, so it is found when the policy is judged, not when it
/// is read.
pub fn contradicted_grants(config: &Config, map: &ComponentMap, files: &[String]) -> Result<()> {
    for (i, listed) in config.forbid_symbol.iter().enumerate() {
        let symbol = listed.name();
        for file in files {
            let Some(pattern) = map.granted_in(i, file) else {
                continue;
            };
            let owner = if config.tests.is_some() && registry::is_test_file(file) {
                Some(TESTS)
            } else {
                map.component_of(file)
            };
            let Some(component) = owner else {
                continue;
            };
            let own = config.rules.get(component).and_then(|rule| {
                rule.forbid_symbol
                    .iter()
                    .map(ForbiddenName::name)
                    .find(|own| may_name_the_same(own, symbol))
            });
            if let Some(own) = own {
                let spelled = if own == symbol {
                    String::new()
                } else {
                    format!(" as `{own}`")
                };
                bail!(
                    "forbid_symbol[{i}].allow_in lets `{pattern}` use `{symbol}`, but {file} there belongs to `{component}`, whose rules.{component}.forbid_symbol forbids it{spelled}"
                );
            }
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
