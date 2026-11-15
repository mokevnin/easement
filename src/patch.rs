//! Edits `easement.yml` through a lossless YAML tree, so comments, `because` reasons,
//! quoting and the order of everything else stay as they are. An entry the tree does
//! not hold as a list item under `rules.<rule>.may_depend_on` (an alias, a merge) is
//! reported to be removed by hand, never guessed at.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use similar::TextDiff;
use yaml_edit::{Document, YamlNode};

use crate::config::{Config, field_key};

/// The text with the `may_depend_on` entries `removals` (`(rule, component)`) taken out,
/// and a note for every entry that could not be located.
pub fn remove_allowances(text: &str, removals: &[(String, String)]) -> (String, Vec<String>) {
    let skip = |rule: &str, target: &str| {
        format!(
            "{rule} -> {target}: not written as a plain `may_depend_on` list in this file, remove it by hand"
        )
    };
    let Ok(document) = Document::from_str(text) else {
        let skipped = removals
            .iter()
            .map(|(rule, target)| skip(rule, target))
            .collect();
        return (text.to_string(), skipped);
    };
    let skipped = removals
        .iter()
        .filter(|(rule, target)| !remove_one(&document, rule, target))
        .map(|(rule, target)| skip(rule, target))
        .collect();
    (document.to_string(), skipped)
}

/// The entries the patch leaves alone, by the list as written and the globs and
/// sets that hold them: the components whose rule it is, and the names no import
/// of theirs uses.
type Held = BTreeMap<(String, Vec<String>), (BTreeSet<String>, BTreeSet<String>)>;

/// The removals of `may_depend_on` entries (`(rule, component)`) that `config`
/// writes by name, and a note for each list whose glob or set holds the others:
/// the rule key covers other components, or the item covers other names, so the
/// patch leaves it alone and names it once, with what no import uses.
pub fn by_name(
    config: &Config,
    removals: Vec<(String, String)>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut plain = Vec::new();
    let mut held = Held::new();
    for (rule, target) in removals {
        let key = config.rule_key(&rule);
        let mut through: Vec<String> = (key != rule).then(|| key.to_string()).into_iter().collect();
        if let Some(covered_by) = config
            .written(&format!("rules.{rule}.may_depend_on"), &target)
            .and_then(|w| w.covered_by.clone())
            && !through.contains(&covered_by)
        {
            through.push(covered_by);
        }
        if through.is_empty() {
            plain.push((rule, target));
            continue;
        }
        let list = format!("rules.{}.may_depend_on", field_key(key));
        let (rules, targets) = held.entry((list, through)).or_default();
        rules.insert(rule);
        targets.insert(target);
    }
    let notes = held
        .into_iter()
        .map(|((list, through), (rules, targets))| {
            let join = |names: BTreeSet<String>| names.into_iter().collect::<Vec<_>>().join(", ");
            format!(
                "{list}: no import of {} uses {}, but the patch does not edit what comes through {}; remove it by hand if no component it covers needs it",
                join(rules),
                join(targets),
                through
                    .iter()
                    .map(|t| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    (plain, notes)
}

/// A unified diff of `old` into `new` for the file `name`, empty when nothing changed.
pub fn unified_diff(name: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// Takes the item naming `target` out of `rules.<rule>.may_depend_on`; an emptied
/// list is written `[]`, so it still reads as a list.
fn remove_one(document: &Document, rule: &str, target: &str) -> bool {
    let Some(list) = document
        .get_mapping("rules")
        .and_then(|rules| rules.get_mapping(rule))
        .and_then(|rule| rule.get_sequence("may_depend_on"))
    else {
        return false;
    };
    let Some(at) = (0..list.len()).find(|&i| {
        list.get(i)
            .and_then(|item| item_name(&item))
            .is_some_and(|name| name == target)
    }) else {
        return false;
    };
    list.remove(at).is_some()
}

/// The component an item names: `core`, or the `name` of `{ name: core, because: x }`.
fn item_name(item: &YamlNode) -> Option<String> {
    let scalar = match item.as_mapping() {
        Some(mapping) => mapping.get("name")?,
        None => item.clone(),
    };
    scalar.as_scalar().map(|scalar| scalar.as_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn gone(text: &str, rule: &str, target: &str) -> (String, Vec<String>) {
        remove_allowances(text, &[(rule.into(), target.into())])
    }

    #[test]
    fn removes_from_a_flow_list_and_keeps_comments() {
        let text = "version: 1\n# why\nrules:\n  web:\n    may_depend_on: [core, data] # both\n    because: layers\n";
        let (new, skipped) = gone(text, "web", "data");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(
            new,
            "version: 1\n# why\nrules:\n  web:\n    may_depend_on: [core] # both\n    because: layers\n"
        );
        let (all, _) = gone(&new, "web", "core");
        assert!(all.contains("may_depend_on: [] # both"), "{all}");
    }

    #[test]
    fn removes_block_items_in_every_written_form() {
        let text = "rules:
  web:
    may_depend_on:
      - core
      # kept
      - { name: data, because: reads rows }
      - name: cache
        because: warm
      - other
    because: layers
";
        let (a, _) = gone(text, "web", "core");
        assert!(!a.contains("- core") && a.contains("# kept"), "{a}");
        let (b, _) = gone(text, "web", "data");
        assert!(!b.contains("reads rows") && b.contains("- core"), "{b}");
        let (c, _) = gone(text, "web", "cache");
        assert!(
            !c.contains("warm") && !c.contains("cache") && c.contains("- other"),
            "{c}"
        );
    }

    #[test]
    fn the_last_item_leaves_an_empty_list_that_still_parses() {
        let text = "version: 1
components:
  a: { in: a }
  b: { in: b }
rules:
  a:
    may_depend_on:
      - b
";
        let (new, skipped) = gone(text, "a", "b");
        assert!(skipped.is_empty());
        assert!(new.contains("may_depend_on: []"), "{new}");
        let config: Config = serde_norway::from_str(&new).unwrap();
        assert!(config.rules["a"].may_depend_on.is_empty());
    }

    #[test]
    fn an_inline_rule_is_edited_in_place() {
        let text = "rules:\n  web: { may_depend_on: [core, data], because: x }\n  api: { may_depend_on: [core] }\n";
        let (new, skipped) = gone(text, "web", "core");
        assert!(skipped.is_empty());
        assert_eq!(
            new,
            "rules:\n  web: { may_depend_on: [data], because: x }\n  api: { may_depend_on: [core] }\n"
        );
    }

    #[test]
    fn an_item_over_several_lines_or_under_quoted_keys_is_removed_too() {
        let text = "rules:\n  web:\n    may_depend_on:\n      - { name: data,\n          because: long }\n";
        let (new, skipped) = gone(text, "web", "data");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(new, "rules:\n  web:\n    may_depend_on: []\n");
        let text = "rules:\n  \"web\":\n    'may_depend_on':\n      - \"core\" # c\n      - data\n";
        let (new, skipped) = gone(text, "web", "core");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(
            new,
            "rules:\n  \"web\":\n    'may_depend_on':\n      - data\n"
        );
    }

    #[test]
    fn what_it_cannot_place_is_reported_not_guessed() {
        let (same, skipped) = gone("rules:\n  web: *shared\n", "web", "core");
        assert_eq!(same, "rules:\n  web: *shared\n");
        assert_eq!(skipped.len(), 1);
        let (_, skipped) = gone("components: {}\n", "web", "core");
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn the_diff_names_the_file_and_is_empty_without_changes() {
        let diff = unified_diff("easement.yml", "a: 1\nb: 2\n", "a: 1\n");
        assert!(
            diff.starts_with("--- a/easement.yml\n+++ b/easement.yml\n"),
            "{diff}"
        );
        assert!(diff.contains("-b: 2"), "{diff}");
        assert_eq!(unified_diff("easement.yml", "a\n", "a\n"), "");
    }
}
