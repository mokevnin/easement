//! Edits `easement.yml` as text, so comments, `because` reasons and the order of
//! everything else stay as they are. Only the shapes below are understood; an entry
//! written any other way is reported to be removed by hand, never guessed at.

use similar::TextDiff;

/// The text with the `may_depend_on` entries `removals` (`(rule, component)`) taken out,
/// and a note for every entry that could not be located.
pub fn remove_allowances(text: &str, removals: &[(String, String)]) -> (String, Vec<String>) {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut skipped = Vec::new();
    for (rule, target) in removals {
        if !remove_one(&mut lines, rule, target) {
            skipped.push(format!(
                "{rule} -> {target}: not written as a plain `may_depend_on` list in this file, remove it by hand"
            ));
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    (out, skipped)
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

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

/// Whether `key` (possibly quoted) is the mapping key at the start of `trimmed`;
/// returns what follows the colon.
fn after_key<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    for form in [key.to_string(), format!("\"{key}\""), format!("'{key}'")] {
        if let Some(rest) = trimmed.strip_prefix(&form)
            && let Some(rest) = rest.strip_prefix(':')
            && (rest.is_empty() || rest.starts_with(' '))
        {
            return Some(rest.trim_start());
        }
    }
    None
}

/// The lines of the block under the line `at` (everything deeper than it).
fn block_end(lines: &[String], at: usize) -> usize {
    let base = indent(&lines[at]);
    let mut end = at + 1;
    let mut last = at + 1;
    while end < lines.len() {
        if !is_blank_or_comment(&lines[end]) {
            if indent(&lines[end]) <= base {
                break;
            }
            last = end + 1;
        }
        end += 1;
    }
    last
}

fn remove_one(lines: &mut Vec<String>, rule: &str, target: &str) -> bool {
    let Some(rules_at) = lines.iter().position(|l| l.starts_with("rules:")) else {
        return false;
    };
    let rules_end = block_end(lines, rules_at);
    let Some(rule_at) = (rules_at + 1..rules_end).find(|&i| {
        !is_blank_or_comment(&lines[i]) && after_key(lines[i].trim_start(), rule).is_some()
    }) else {
        return false;
    };
    let rest = after_key(lines[rule_at].trim_start(), rule).unwrap_or_default();
    // `rule: { may_depend_on: [a, b], because: x }` on one line.
    if rest.starts_with('{') {
        let line = lines[rule_at].clone();
        let Some(key_at) = line.find("may_depend_on:") else {
            return false;
        };
        let after = &line[key_at + "may_depend_on:".len()..];
        let Some(open) = after.find('[') else {
            return false;
        };
        let Some(close) = after[open..].find(']') else {
            return false;
        };
        let list_start = key_at + "may_depend_on:".len() + open;
        let list_end = list_start + close + 1;
        let Some(edited) = edit_flow_list(&line[list_start..list_end], target) else {
            return false;
        };
        lines[rule_at] = format!("{}{edited}{}", &line[..list_start], &line[list_end..]);
        return true;
    }
    if !rest.is_empty() && !rest.starts_with('#') {
        return false;
    }
    let rule_end = block_end(lines, rule_at);
    let Some(key_at) = (rule_at + 1..rule_end).find(|&i| {
        indent(&lines[i]) > indent(&lines[rule_at])
            && after_key(lines[i].trim_start(), "may_depend_on").is_some()
    }) else {
        return false;
    };
    let after = after_key(lines[key_at].trim_start(), "may_depend_on").unwrap_or_default();
    if after.starts_with('[') {
        let line = lines[key_at].clone();
        let Some(open) = line.find('[') else {
            return false;
        };
        let Some(close) = line.rfind(']') else {
            return false;
        };
        let Some(edited) = edit_flow_list(&line[open..=close], target) else {
            return false;
        };
        lines[key_at] = format!("{}{edited}{}", &line[..open], &line[close + 1..]);
        return true;
    }
    if !after.is_empty() && !after.starts_with('#') {
        return false;
    }
    remove_block_item(lines, key_at, target)
}

/// `[a, b, c]` without `target`, or `None` when the list holds anything but plain names.
fn edit_flow_list(list: &str, target: &str) -> Option<String> {
    let inner = list.strip_prefix('[')?.strip_suffix(']')?;
    if inner.contains(['{', '}', '[', '#']) {
        return None;
    }
    let items: Vec<&str> = inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let kept: Vec<&str> = items
        .iter()
        .copied()
        .filter(|s| unquote(s) != target)
        .collect();
    (kept.len() < items.len()).then(|| format!("[{}]", kept.join(", ")))
}

/// Removes the item of the block list under `key_at` that names `target`.
fn remove_block_item(lines: &mut Vec<String>, key_at: usize, target: &str) -> bool {
    let key_indent = indent(&lines[key_at]);
    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut i = key_at + 1;
    while i < lines.len() {
        let line = &lines[i];
        if is_blank_or_comment(line) {
            i += 1;
            continue;
        }
        // List items may sit at the indent of their key.
        if indent(line) < key_indent
            || (indent(line) == key_indent && !line.trim().starts_with("- "))
        {
            break;
        }
        if line.trim().starts_with("- ") {
            let dash = indent(line);
            let mut end = i + 1;
            let mut last = i + 1;
            while end < lines.len() {
                if !is_blank_or_comment(&lines[end]) {
                    if indent(&lines[end]) <= dash {
                        break;
                    }
                    last = end + 1;
                }
                end += 1;
            }
            items.push((i, last));
            i = last;
        } else {
            i += 1;
        }
    }
    let Some(position) = items
        .iter()
        .position(|&(start, end)| item_name(&lines[start..end]).as_deref() == Some(target))
    else {
        return false;
    };
    let (start, end) = items[position];
    lines.drain(start..end);
    if items.len() == 1 {
        // An empty block would read as null; say it is empty.
        let key = lines[key_at].trim_end().to_string();
        let key = key.split('#').next().unwrap_or(&key).trim_end().to_string();
        lines[key_at] = format!("{key} []");
    }
    true
}

/// The component an item names: `- core`, `- { name: core, because: x }` on one
/// line, or `- name: core` with more keys under it.
fn item_name(item: &[String]) -> Option<String> {
    let first = item.first()?.trim().strip_prefix("- ")?.trim();
    if let Some(inner) = first.strip_prefix('{') {
        let inner = inner.strip_suffix('}')?;
        if item.len() > 1 {
            return None;
        }
        return inner.split(',').find_map(|part| {
            part.trim()
                .strip_prefix("name:")
                .map(|n| unquote(n).to_string())
        });
    }
    if let Some(name) = first.strip_prefix("name:") {
        return Some(unquote(name.split('#').next().unwrap_or(name)).to_string());
    }
    if first.contains(':') || first.starts_with(['[', '{', '&', '*']) {
        return None;
    }
    Some(unquote(first.split(" #").next().unwrap_or(first)).to_string())
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
    fn what_it_cannot_place_is_reported_not_guessed() {
        let text = "rules:\n  web:\n    may_depend_on:\n      - { name: data,\n          because: long }\n";
        let (same, skipped) = gone(text, "web", "data");
        assert_eq!(same, text);
        assert_eq!(skipped.len(), 1);
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
