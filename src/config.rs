use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const DEFAULT_CONFIG: &str = "easement.yml";
pub const DEFAULT_BASELINE: &str = "easement.baseline.yml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub components: BTreeMap<String, Component>,
    #[serde(default)]
    pub common: Vec<String>,
    #[serde(default)]
    pub rules: BTreeMap<String, Rule>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub baseline: Option<PathBuf>,
    #[serde(default)]
    pub python: PythonConfig,
    /// Fail on dependency cycles between components: `true` for all of them,
    /// or a list to limit the check.
    #[serde(default)]
    pub no_cycles: NoCycles,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
pub enum NoCycles {
    #[default]
    Off,
    All(bool),
    Among(Vec<String>),
}

impl NoCycles {
    pub fn covers(&self, component: &str) -> bool {
        match self {
            NoCycles::Off => false,
            NoCycles::All(on) => *on,
            NoCycles::Among(list) => list.iter().any(|c| c == component),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonConfig {
    /// Directories that act as `sys.path` entries, relative to the project root.
    #[serde(default = "default_python_roots")]
    pub roots: Vec<PathBuf>,
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            roots: default_python_roots(),
        }
    }
}

fn default_python_roots() -> Vec<PathBuf> {
    vec![".".into(), "src".into()]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    #[serde(rename = "in")]
    pub patterns: Patterns,
    /// When set, other components may import only these paths of this component.
    #[serde(default)]
    pub public: Option<Patterns>,
    /// Why the public API exists; shown with privacy violations.
    #[serde(default)]
    pub because: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Patterns {
    One(String),
    Many(Vec<String>),
}

impl Patterns {
    pub fn as_vec(&self) -> Vec<&str> {
        match self {
            Patterns::One(p) => vec![p.as_str()],
            Patterns::Many(ps) => ps.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub may_depend_on: Vec<String>,
    /// Why the rule exists; shown with its violations.
    #[serde(default)]
    pub because: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let config: Config = serde_norway::from_str(&text)
            .with_context(|| format!("invalid config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported config version {}, expected 1", self.version);
        }
        if self.components.is_empty() {
            bail!("config declares no components");
        }
        let known: BTreeSet<&str> = self.components.keys().map(String::as_str).collect();
        let check = |name: &str, place: &str| -> Result<()> {
            if !known.contains(name) {
                bail!("unknown component `{name}` in {place}");
            }
            Ok(())
        };
        for name in &self.common {
            check(name, "common")?;
        }
        for (from, rule) in &self.rules {
            check(from, "rules")?;
            for to in &rule.may_depend_on {
                check(to, &format!("rules.{from}.may_depend_on"))?;
            }
        }
        for (name, component) in &self.components {
            if component.patterns.as_vec().is_empty() {
                bail!("component `{name}` has no `in` patterns");
            }
            if component
                .public
                .as_ref()
                .is_some_and(|p| p.as_vec().is_empty())
            {
                bail!("component `{name}` has an empty `public` list");
            }
        }
        if let NoCycles::Among(list) = &self.no_cycles {
            for name in list {
                check(name, "no_cycles")?;
            }
        }
        Ok(())
    }

    pub fn reason(&self, from: &str) -> Option<&str> {
        self.rules.get(from).and_then(|r| r.because.as_deref())
    }

    pub fn allows(&self, from: &str, to: &str) -> bool {
        from == to
            || self.common.iter().any(|c| c == to)
            || self
                .rules
                .get(from)
                .is_some_and(|r| r.may_depend_on.iter().any(|c| c == to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<Config> {
        let config: Config = serde_norway::from_str(yaml)?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn allows_self_common_and_declared() {
        let c = parse(
            "version: 1
components:
  a: { in: a }
  b: { in: b }
  c: { in: c }
  m: { in: m }
common: [m]
rules:
  a: { may_depend_on: [b] }
",
        )
        .unwrap();
        assert!(c.allows("a", "a"));
        assert!(c.allows("a", "b"));
        assert!(c.allows("c", "m"));
        assert!(!c.allows("a", "c"));
        assert!(!c.allows("b", "a"));
    }

    #[test]
    fn rejects_unknown_component() {
        let err = parse(
            "version: 1
components:
  a: { in: a }
rules:
  a: { may_depend_on: [ghost] }
",
        )
        .unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }
}
