use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::components::ComponentMap;
use crate::config::Config;

/// A path as the config sees it: relative to the project root, `/`-separated.
/// A path that exists relative to the working directory is taken from there,
/// anything else is already relative to the root.
pub fn root_relative(root: &Path, given: &Path) -> String {
    let resolved = dunce::canonicalize(given)
        .ok()
        .and_then(|abs| abs.strip_prefix(root).map(Path::to_path_buf).ok())
        .unwrap_or_else(|| PathBuf::from(given.to_string_lossy().trim_start_matches("./")));
    crate::scan::slash(&resolved)
}

/// Says which component a path belongs to and what that component may do.
pub fn explain(config: &Config, map: &ComponentMap, rel: &str) -> Result<String> {
    let Some((name, pattern)) = map.component_and_pattern(rel) else {
        bail!("{rel} belongs to no component, so easement does not check it");
    };
    let mut out = format!("{rel}\n  component: {name} (matches `{pattern}`)\n");
    let component = &config.components[name];
    if let Some(public) = &component.public {
        out.push_str(&format!(
            "  public API: {} (other components may import only these)\n",
            public.as_vec().join(", ")
        ));
    }
    let rule = config.rules.get(name);
    let mut allowed = Vec::new();
    let mut forbidden = Vec::new();
    let mut other = Vec::new();
    for to in config.components.keys().filter(|c| c.as_str() != name) {
        if config.forbids(name, to) {
            forbidden.push(to.as_str());
        } else if config.allows(name, to) {
            let why = if rule.is_some_and(|r| r.may_depend_on.contains(to)) {
                "may_depend_on"
            } else if config.common.contains(to) {
                "common"
            } else {
                "layers"
            };
            allowed.push(format!("{to} ({why})"));
        } else {
            other.push(to.as_str());
        }
    }
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
    if let Some(rule) = rule {
        if let Some(external) = &rule.may_use_external {
            out.push_str(&format!(
                "  external modules: only {}\n",
                list(&external.clone())
            ));
        }
        if !rule.forbid_external.is_empty() {
            out.push_str(&format!(
                "  forbidden external modules: {}\n",
                rule.forbid_external.join(", ")
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

    #[test]
    fn explains_allowed_forbidden_and_unmapped() {
        let config: Config = serde_norway::from_str(
            "version: 1
components:
  web: { in: src/web }
  core: { in: src/core }
  data: { in: src/data, public: src/data/api }
  shared: { in: src/shared }
common: [shared]
layers: [web, core]
rules:
  web:
    must_not_depend_on: [data]
    forbid_external: [moment]
    because: pages go through core
",
        )
        .unwrap();
        let map = ComponentMap::new(&config).unwrap();
        let out = explain(&config, &map, "src/web/page.ts").unwrap();
        assert!(out.contains("component: web (matches `src/web`)"));
        assert!(out.contains("may depend on: core (layers), shared (common)"));
        assert!(out.contains("forbidden: data"));
        assert!(out.contains("forbidden external modules: moment"));
        assert!(out.contains("because: pages go through core"));
        let data = explain(&config, &map, "src/data/db.ts").unwrap();
        assert!(data.contains("public API: src/data/api"));
        assert!(data.contains("not allowed: core, web"));
        assert!(explain(&config, &map, "stray.ts").is_err());
    }
}
