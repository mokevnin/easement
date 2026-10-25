use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_norway::{Mapping, Value};

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
    /// Components from the top layer to the bottom one: a layer may depend on
    /// any layer below it, never on one above.
    #[serde(default)]
    pub layers: Vec<String>,
    /// Fail on files that belong to no component.
    #[serde(default)]
    pub strict: bool,
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
    /// Components that are never allowed, even if `common` or `layers` would allow them.
    #[serde(default)]
    pub must_not_depend_on: Vec<String>,
    /// External modules the component may use; unset means no restriction.
    #[serde(default)]
    pub may_use_external: Option<Vec<String>>,
    /// External modules the component must not use.
    #[serde(default)]
    pub forbid_external: Vec<String>,
    /// Most components the component may depend on.
    #[serde(default)]
    pub max_fan_out: Option<usize>,
    /// Highest instability, `out / (in + out)`, from 0 (stable) to 1.
    #[serde(default)]
    pub max_instability: Option<f64>,
    /// Why the rule exists; shown with its violations.
    #[serde(default)]
    pub because: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let value = load_value(path, &mut Vec::new())?;
        let config: Config = serde_norway::from_value(value)
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
            for to in &rule.must_not_depend_on {
                check(to, &format!("rules.{from}.must_not_depend_on"))?;
                if rule.may_depend_on.contains(to) {
                    bail!("rules.{from} both allows and forbids `{to}`");
                }
            }
            if rule
                .max_instability
                .is_some_and(|i| !(0.0..=1.0).contains(&i))
            {
                bail!("rules.{from}.max_instability must be between 0 and 1");
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
        let mut layers = BTreeSet::new();
        for name in &self.layers {
            check(name, "layers")?;
            if !layers.insert(name) {
                bail!("component `{name}` is listed twice in layers");
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

    pub fn forbids(&self, from: &str, to: &str) -> bool {
        self.rules
            .get(from)
            .is_some_and(|r| r.must_not_depend_on.iter().any(|c| c == to))
    }

    pub fn allows(&self, from: &str, to: &str) -> bool {
        from == to
            || !self.forbids(from, to)
                && (self.common.iter().any(|c| c == to)
                    || self.above(from, to)
                    || self
                        .rules
                        .get(from)
                        .is_some_and(|r| r.may_depend_on.iter().any(|c| c == to)))
    }

    /// Whether `from` is a higher layer than `to`.
    fn above(&self, from: &str, to: &str) -> bool {
        let at = |name: &str| self.layers.iter().position(|l| l == name);
        matches!((at(from), at(to)), (Some(f), Some(t)) if f < t)
    }
}

/// Reads a config file and merges the files it `extends` underneath it.
///
/// `extends` is a path or a list of paths, relative to the file that names them.
/// Patterns in every file stay relative to the root of the config being loaded.
/// `components` and `rules` merge by name, the later definition replacing the
/// earlier one; `common` and `exclude` are concatenated; any other key is
/// replaced by the extending file.
fn load_value(path: &Path, stack: &mut Vec<PathBuf>) -> Result<Value> {
    let canonical = dunce::canonicalize(path)
        .with_context(|| format!("cannot read config {}", path.display()))?;
    if stack.contains(&canonical) {
        bail!("config {} extends itself", path.display());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config {}", path.display()))?;
    let Value::Mapping(mut own) = serde_norway::from_str::<Value>(&text)
        .with_context(|| format!("invalid config {}", path.display()))?
    else {
        bail!("config {} is not a mapping", path.display());
    };
    let bases = match own.remove("extends") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(one)) => vec![one],
        Some(other) => serde_norway::from_value::<Vec<String>>(other)
            .with_context(|| format!("`extends` in {} must be a path or a list", path.display()))?,
    };
    stack.push(canonical);
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut merged = Mapping::new();
    for base in bases {
        let Value::Mapping(base) = load_value(&dir.join(base), stack)? else {
            unreachable!("load_value returns mappings");
        };
        merge(&mut merged, base);
    }
    stack.pop();
    merge(&mut merged, own);
    Ok(Value::Mapping(merged))
}

fn merge(into: &mut Mapping, from: Mapping) {
    for (key, value) in from {
        let name = key.as_str().unwrap_or_default();
        match (into.get_mut(&key), value) {
            (Some(Value::Mapping(old)), Value::Mapping(new))
                if name == "components" || name == "rules" =>
            {
                for (k, v) in new {
                    old.insert(k, v);
                }
            }
            (Some(Value::Sequence(old)), Value::Sequence(new))
                if name == "common" || name == "exclude" =>
            {
                for v in new {
                    if !old.contains(&v) {
                        old.push(v);
                    }
                }
            }
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extends_merges_by_name_and_detects_loops() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
        write(
            "base.yml",
            "version: 1
components:
  a: { in: a }
  b: { in: b }
common: [b]
exclude: [gen]
rules:
  a: { may_depend_on: [b], because: base }
",
        );
        write(
            "easement.yml",
            "extends: base.yml
components:
  c: { in: c }
  b: { in: other/b }
common: [c]
exclude: [vendor]
rules:
  a: { because: own }
",
        );
        let c = Config::load(&dir.path().join("easement.yml")).unwrap();
        assert_eq!(c.components.len(), 3);
        assert_eq!(c.components["b"].patterns.as_vec(), ["other/b"]);
        assert_eq!(c.common, ["b", "c"]);
        assert_eq!(c.exclude, ["gen", "vendor"]);
        // The rule of the extending file replaces the base one as a whole.
        assert_eq!(c.reason("a"), Some("own"));
        assert!(c.rules["a"].may_depend_on.is_empty());

        write("loop.yml", "extends: loop.yml\nversion: 1\n");
        let err = Config::load(&dir.path().join("loop.yml")).unwrap_err();
        assert!(err.to_string().contains("extends itself"));
    }

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
    fn layers_allow_downwards_and_forbidden_wins() {
        let c = parse(
            "version: 1
components:
  top: { in: top }
  mid: { in: mid }
  low: { in: low }
  m: { in: m }
common: [m]
layers: [top, mid, low]
rules:
  top: { must_not_depend_on: [low, m] }
",
        )
        .unwrap();
        assert!(c.allows("top", "mid"));
        assert!(c.allows("mid", "low"));
        assert!(!c.allows("mid", "top"));
        assert!(!c.allows("low", "mid"));
        assert!(!c.allows("top", "low"));
        assert!(!c.allows("top", "m"));
        assert!(c.allows("mid", "m"));
    }

    #[test]
    fn rejects_contradictions() {
        let err = parse(
            "version: 1
components:
  a: { in: a }
  b: { in: b }
rules:
  a: { may_depend_on: [b], must_not_depend_on: [b] }
",
        )
        .unwrap_err();
        assert!(err.to_string().contains("both allows and forbids"));
        let err = parse(
            "version: 1
components:
  a: { in: a }
layers: [a, a]
",
        )
        .unwrap_err();
        assert!(err.to_string().contains("twice"));
        let err = parse(
            "version: 1
components:
  a: { in: a }
rules:
  a: { max_instability: 1.5 }
",
        )
        .unwrap_err();
        assert!(err.to_string().contains("max_instability"));
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
