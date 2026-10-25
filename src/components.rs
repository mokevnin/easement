use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};

use crate::config::Config;

/// Maps repository-relative paths (files or directories, `/`-separated) to components.
///
/// A pattern names a module, whether it is a directory or a file: `P` matches
/// `P` itself, everything below it (`P/**`) and a file `P.<ext>`. So
/// `src/app/cli` covers both `cli.py` and the `cli/` package. When several
/// components match, the longest pattern wins.
pub struct ComponentMap {
    entries: Vec<Entry>,
    /// Component -> matchers of its public paths, for components that declare them.
    public: std::collections::HashMap<String, Vec<[GlobMatcher; 3]>>,
    /// Component -> what it may do with external modules, for components that restrict it.
    externals: std::collections::HashMap<String, Externals>,
}

struct Externals {
    /// `None`: any external module may be used.
    allowed: Option<GlobSet>,
    forbidden: GlobSet,
}

fn glob_set(component: &str, key: &str, patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            Glob::new(pattern)
                .with_context(|| format!("bad pattern `{pattern}` in {key} of `{component}`"))?,
        );
    }
    Ok(builder.build()?)
}

/// A specifier and its prefixes cut at the separators of the languages (`/`,
/// `.`, `::`, `\`), so the pattern `lodash` covers `lodash/fp`, `java.util`
/// covers `java.util.List` and `serde` covers `serde::Deserialize`.
fn prefixes(specifier: &str) -> impl Iterator<Item = &str> {
    specifier
        .match_indices(['/', '.', ':', '\\'])
        .filter(|(i, sep)| *sep != ":" || specifier[i + 1..].starts_with(':'))
        .map(|(i, _)| &specifier[..i])
        .chain(std::iter::once(specifier))
}

fn module_matchers(component: &str, pattern: &str) -> Result<[GlobMatcher; 3]> {
    let pattern = pattern.trim_end_matches('/');
    let compile = |p: &str| -> Result<GlobMatcher> {
        Ok(GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .with_context(|| format!("bad pattern `{p}` in component `{component}`"))?
            .compile_matcher())
    };
    Ok([
        compile(pattern)?,
        compile(&format!("{pattern}/**"))?,
        compile(&format!("{pattern}.*"))?,
    ])
}

struct Entry {
    component: String,
    pattern: String,
    pattern_len: usize,
    matchers: [GlobMatcher; 3],
}

impl ComponentMap {
    pub fn new(config: &Config) -> Result<Self> {
        let mut entries = Vec::new();
        let mut public = std::collections::HashMap::new();
        let mut externals = std::collections::HashMap::new();
        for (name, rule) in &config.rules {
            if rule.may_use_external.is_some() || !rule.forbid_external.is_empty() {
                externals.insert(
                    name.clone(),
                    Externals {
                        allowed: rule
                            .may_use_external
                            .as_deref()
                            .map(|p| glob_set(name, "may_use_external", p))
                            .transpose()?,
                        forbidden: glob_set(name, "forbid_external", &rule.forbid_external)?,
                    },
                );
            }
        }
        for (name, component) in &config.components {
            for pattern in component.patterns.as_vec() {
                entries.push(Entry {
                    component: name.clone(),
                    pattern: pattern.trim_end_matches('/').to_string(),
                    pattern_len: pattern.trim_end_matches('/').len(),
                    matchers: module_matchers(name, pattern)?,
                });
            }
            if let Some(patterns) = &component.public {
                let matchers = patterns
                    .as_vec()
                    .into_iter()
                    .map(|p| module_matchers(name, p))
                    .collect::<Result<Vec<_>>>()?;
                public.insert(name.clone(), matchers);
            }
        }
        Ok(Self {
            entries,
            public,
            externals,
        })
    }

    /// Why `component` may not use the external module `specifier`, if it may not.
    pub fn external_problem(&self, component: &str, specifier: &str) -> Option<&'static str> {
        let rules = self.externals.get(component)?;
        if prefixes(specifier).any(|p| rules.forbidden.is_match(p)) {
            return Some("is forbidden");
        }
        match &rules.allowed {
            Some(allowed) if !prefixes(specifier).any(|p| allowed.is_match(p)) => {
                Some("is not in may_use_external")
            }
            _ => None,
        }
    }

    /// Whether `rel_path` may be imported from outside `component`: always,
    /// unless the component declares a public API that does not cover it.
    pub fn is_public(&self, component: &str, rel_path: &str) -> bool {
        self.public.get(component).is_none_or(|matchers| {
            matchers
                .iter()
                .any(|ms| ms.iter().any(|m| m.is_match(rel_path)))
        })
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
        self.entries
            .iter()
            .filter(|e| e.matchers.iter().any(|m| m.is_match(rel_path)))
            .max_by_key(|e| e.pattern_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(yaml: &str) -> ComponentMap {
        let config: Config = serde_norway::from_str(yaml).unwrap();
        ComponentMap::new(&config).unwrap()
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
        assert!(m.external_problem("domain", "moment/locale/ru").is_some());
        assert!(m.external_problem("domain", "@acme/sdk/v2").is_some());
        assert!(m.external_problem("domain", "lodash").is_none());
        assert!(m.external_problem("ui", "react-dom").is_some());
        assert!(m.external_problem("ui", "react/jsx-runtime").is_none());
        assert!(m.external_problem("ui", "java.util.List").is_none());
        assert!(m.external_problem("ui", "java.io.File").is_some());
        assert!(m.external_problem("other", "anything").is_none());
        assert_eq!(
            prefixes("serde::de::Deserialize").collect::<Vec<_>>(),
            ["serde", "serde::de", "serde::de::Deserialize"]
        );
        assert_eq!(
            prefixes("Illuminate\\Support\\Str").collect::<Vec<_>>(),
            [
                "Illuminate",
                "Illuminate\\Support",
                "Illuminate\\Support\\Str"
            ]
        );
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
