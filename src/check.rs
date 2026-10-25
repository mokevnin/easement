use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::components::ComponentMap;
use crate::config::Config;
use crate::lang::Target;
use crate::scan::{Dep, slash};

/// A dependency the config does not allow.
#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub file: String,
    pub line: usize,
    pub from: String,
    pub to: String,
    pub target: String,
    pub specifier: String,
}

/// Identity of a violation for the baseline: line numbers are left out so that
/// unrelated edits do not invalidate it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub file: String,
    pub target: String,
    pub from: String,
    pub to: String,
}

impl Violation {
    pub fn key(&self) -> Key {
        Key {
            file: self.file.clone(),
            target: self.target.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub files: usize,
    pub internal_deps: usize,
    pub external_deps: usize,
    pub unresolved_deps: usize,
    pub unmapped_files: usize,
    /// `file:line: specifier` for imports that look internal but did not resolve.
    pub unresolved: Vec<String>,
}

/// Evaluates every internal dependency against the rules.
/// Violations are unique per (file, target); the first line wins.
pub fn evaluate(config: &Config, map: &ComponentMap, deps: &[Dep]) -> (Vec<Violation>, Stats) {
    let mut stats = Stats::default();
    let mut found: BTreeMap<Key, Violation> = BTreeMap::new();
    let mut unmapped = std::collections::BTreeSet::new();
    for dep in deps {
        let target = match &dep.target {
            Target::Internal(path) => slash(path),
            Target::External(_) => {
                stats.external_deps += 1;
                continue;
            }
            Target::Unresolved => {
                stats.unresolved_deps += 1;
                stats.unresolved.push(format!(
                    "{}:{}: {}",
                    slash(&dep.file),
                    dep.line,
                    dep.specifier
                ));
                continue;
            }
        };
        stats.internal_deps += 1;
        let file = slash(&dep.file);
        let Some(from) = map.component_of(&file) else {
            unmapped.insert(file);
            continue;
        };
        let Some(to) = map.component_of(&target) else {
            continue;
        };
        if config.allows(from, to) {
            continue;
        }
        let violation = Violation {
            file,
            line: dep.line,
            from: from.to_string(),
            to: to.to_string(),
            target,
            specifier: dep.specifier.clone(),
        };
        found.entry(violation.key()).or_insert(violation);
    }
    stats.unmapped_files = unmapped.len();
    (found.into_values().collect(), stats)
}
