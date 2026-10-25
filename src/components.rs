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
}

struct Entry {
    component: String,
    pattern_len: usize,
    matchers: [GlobMatcher; 3],
}

impl ComponentMap {
    pub fn new(config: &Config) -> Result<Self> {
        let mut entries = Vec::new();
        for (name, component) in &config.components {
            for pattern in component.patterns.as_vec() {
                let pattern = pattern.trim_end_matches('/');
                let compile = |p: &str| -> Result<GlobMatcher> {
                    Ok(GlobBuilder::new(p)
                        .literal_separator(true)
                        .build()
                        .with_context(|| format!("bad pattern `{p}` in component `{name}`"))?
                        .compile_matcher())
                };
                entries.push(Entry {
                    component: name.clone(),
                    pattern_len: pattern.len(),
                    matchers: [
                        compile(pattern)?,
                        compile(&format!("{pattern}/**"))?,
                        compile(&format!("{pattern}.*"))?,
                    ],
                });
            }
        }
        Ok(Self { entries })
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
}
