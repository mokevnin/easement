use std::collections::{BTreeMap, BTreeSet};

use crate::check::Edge;
use crate::components::ComponentMap;
use crate::config::Config;
use crate::lang::Target;
use crate::scan::{Dep, slash};

/// What the current dependency graph says about the config: allowances nothing
/// uses, imports nothing allows, and components that look isolated already.
pub fn suggest(
    config: &Config,
    map: &ComponentMap,
    edges: &[Edge],
    deps: &[Dep],
    files: &[String],
) -> String {
    let mut used: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for e in edges {
        *used.entry((&e.from, &e.to)).or_default() += 1;
    }

    let mut out = String::new();

    let mut unused = Vec::new();
    for (from, rule) in &config.rules {
        for to in rule.may_depend_on.iter().map(|l| l.name()) {
            if !used.contains_key(&(from.as_str(), to)) {
                unused.push(format!("{from} -> {to}"));
            }
        }
    }
    section(
        &mut out,
        "Allowed in may_depend_on but never imported (remove to tighten the rules):",
        unused,
    );

    let mut missing: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for ((from, to), count) in &used {
        if !config.allows(from, to) {
            missing
                .entry(from)
                .or_default()
                .push(format!("{to} ({count} import(s))"));
        }
    }
    section(
        &mut out,
        "Imported but not allowed (add to may_depend_on, or fix the imports):",
        missing
            .into_iter()
            .map(|(from, targets)| format!("{from}: {}", targets.join(", ")))
            .collect(),
    );

    section(
        &mut out,
        "Instances that never import each other (candidates for `isolated: true`):",
        isolation_candidates(config, map, deps, files),
    );

    if out.is_empty() {
        out.push_str("Nothing to suggest: the config matches the dependency graph.\n");
    }
    out
}

fn section(out: &mut String, title: &str, items: Vec<String>) {
    if items.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(title);
    out.push('\n');
    for item in items {
        out.push_str(&format!("  {item}\n"));
    }
}

/// Components whose `in` pattern has a `*` (one directory per instance), that
/// are not isolated yet, have at least two instances and no import between them.
fn isolation_candidates(
    config: &Config,
    map: &ComponentMap,
    deps: &[Dep],
    files: &[String],
) -> Vec<String> {
    // (component, pattern) -> instances seen, and whether instances import each other
    let mut groups: BTreeMap<(String, String), (BTreeSet<String>, bool)> = BTreeMap::new();
    let instance = |file: &str, pattern: &str| {
        let n = pattern.split('/').count();
        file.split('/').take(n).collect::<Vec<_>>().join("/")
    };
    let locate = |file: &str| {
        let (component, pattern) = map.component_and_pattern(file)?;
        if config.components[component].isolated || !pattern.contains('*') {
            return None;
        }
        Some((
            (component.to_string(), pattern.to_string()),
            instance(file, pattern),
        ))
    };
    for file in files {
        if let Some((key, here)) = locate(file) {
            groups.entry(key).or_default().0.insert(here);
        }
    }
    for dep in deps {
        let Target::Internal(target) = &dep.target else {
            continue;
        };
        let (file, target) = (slash(&dep.file), slash(target));
        let Some((key, here)) = locate(&file) else {
            continue;
        };
        let entry = groups.entry(key.clone()).or_default();
        entry.0.insert(here.clone());
        if let Some((there_key, there)) = locate(&target)
            && there_key == key
            && there != here
        {
            entry.1 = true;
        }
    }
    groups
        .into_iter()
        .filter(|(_, (instances, crossing))| instances.len() > 1 && !crossing)
        .map(|((component, pattern), (instances, _))| {
            let names: Vec<_> = instances
                .iter()
                .map(|i| i.rsplit('/').next().unwrap_or(i).to_string())
                .collect();
            format!("{component} (`{pattern}`): {}", names.join(", "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn dep(file: &str, target: &str) -> Dep {
        Dep {
            file: PathBuf::from(file),
            line: 1,
            specifier: String::new(),
            target: Target::Internal(PathBuf::from(target)),
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            file: format!("{from}/x"),
            line: 1,
        }
    }

    #[test]
    fn suggests_removals_additions_and_isolation() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  web: { in: src/web }
  core: { in: src/core }
  data: { in: src/data }
  pages: { in: 'src/pages/*' }
rules:
  web: { may_depend_on: [core, data] }
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let edges = [edge("web", "core"), edge("core", "data")];
        let deps = [
            dep("src/pages/a/x.ts", "src/core/c.ts"),
            dep("src/pages/b/y.ts", "src/pages/b/z.ts"),
        ];
        let files = [
            "src/pages/a/x.ts".to_string(),
            "src/pages/b/y.ts".to_string(),
            "src/pages/b/z.ts".to_string(),
            "src/pages/c/leaf.ts".to_string(),
        ];
        let out = suggest(&config, &map, &edges, &deps, &files);
        assert!(out.contains("  web -> data\n"), "{out}");
        assert!(!out.contains("web -> core"), "{out}");
        assert!(out.contains("  core: data (1 import(s))\n"), "{out}");
        assert!(out.contains("  pages (`src/pages/*`): a, b, c\n"), "{out}");

        let crossing = [dep("src/pages/a/x.ts", "src/pages/b/y.ts")];
        assert!(!suggest(&config, &map, &edges, &crossing, &files).contains("pages"));
    }
}
