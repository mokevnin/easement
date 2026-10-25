use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobMatcher};

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
    pattern_len: usize,
    matchers: [GlobMatcher; 3],
}

impl ComponentMap {
    pub fn new(config: &Config) -> Result<Self> {
        let mut entries = Vec::new();
        let mut public = std::collections::HashMap::new();
        for (name, component) in &config.components {
            for pattern in component.patterns.as_vec() {
                entries.push(Entry {
                    component: name.clone(),
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
        Ok(Self { entries, public })
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
        self.entries
            .iter()
            .filter(|e| e.matchers.iter().any(|m| m.is_match(rel_path)))
            .max_by_key(|e| e.pattern_len)
            .map(|e| e.component.as_str())
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
