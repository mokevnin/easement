//! Svelte components. No SCIP indexer reads a `.svelte` file, so its imports are
//! taken from its `<script>` blocks with the TypeScript grammar and resolved the way
//! Vite and TypeScript resolve them: a relative path, a subpath import of the
//! nearest `package.json` (`#lib/*`), or a `paths` alias of its `tsconfig.json`
//! (`$lib`, which SvelteKit writes into `.svelte-kit/tsconfig.json`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use jsonc_parser::{JsonObject, JsonValue, ParseOptions, parse_to_value};
use regex::Regex;
use tree_sitter::{Language, Query};

use super::{Adapter, Import, Target, captured_nodes, parse};
use crate::relpath::{self, parent, slash};

const QUERY: &str = r#"
(import_statement source: (string (string_fragment) @node))
(export_statement source: (string (string_fragment) @node))
(call_expression function: (import) arguments: (arguments . (string (string_fragment) @node)))
"#;

/// What a bundler tries after a specifier that names no file as written.
const EXTENSIONS: &[&str] = &["ts", "js", "mts", "mjs", "tsx", "jsx", "svelte", "d.ts"];

/// The extensions an import may write for a file that is TypeScript on disk:
/// `./api.js` is `./api.ts`, as `moduleResolution: bundler` reads it.
const COMPILED: &[&str] = &[".js", ".mjs", ".cjs", ".jsx"];

pub struct Svelte {
    root: PathBuf,
    language: Language,
    query: Query,
    files: BTreeSet<String>,
    /// The aliases of each directory with a `package.json`, the deepest directory first.
    projects: Vec<Project>,
}

struct Project {
    dir: String,
    /// Pattern (one `*` at most) -> the path it stands for, relative to the root;
    /// the longest pattern first, as the more specific one wins.
    aliases: Vec<(String, String)>,
}

impl Svelte {
    /// The adapter over the projects whose `package.json` is among `files`,
    /// relative to `root`.
    pub fn new(root: &Path, files: &[PathBuf]) -> Result<Self> {
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let query = Query::new(&language, QUERY)?;
        let mut projects: Vec<Project> = files
            .iter()
            .filter(|f| f.file_name().is_some_and(|n| n == "package.json"))
            .map(|f| {
                let dir = slash(f.parent().unwrap_or(Path::new("")));
                let aliases = aliases(root, &dir);
                Project { dir, aliases }
            })
            .collect();
        projects.sort_by_key(|p| std::cmp::Reverse(p.dir.len()));
        Ok(Self {
            root: root.to_path_buf(),
            language,
            query,
            files: files.iter().map(|f| slash(f)).collect(),
            projects,
        })
    }

    /// The file `base` names after the bundler's probing, if the project has it.
    fn probe(&self, base: &str) -> Option<String> {
        let mut stems = vec![base];
        stems.extend(COMPILED.iter().filter_map(|ext| base.strip_suffix(ext)));
        stems.into_iter().find_map(|stem| {
            std::iter::once(stem.to_string())
                .chain(EXTENSIONS.iter().map(|e| format!("{stem}.{e}")))
                .chain(EXTENSIONS.iter().map(|e| format!("{stem}/index.{e}")))
                .find(|candidate| self.files.contains(candidate))
        })
    }

    /// The path a non-relative specifier stands for under the aliases of the
    /// project that holds `file`.
    fn alias(&self, file: &str, specifier: &str) -> Option<String> {
        let project = self
            .projects
            .iter()
            .find(|p| relpath::is_under(file, &p.dir))?;
        project
            .aliases
            .iter()
            .find_map(|(pattern, target)| match pattern.split_once('*') {
                None => (pattern == specifier).then(|| target.clone()),
                Some((prefix, suffix)) => {
                    let rest = specifier.strip_prefix(prefix)?.strip_suffix(suffix)?;
                    Some(target.replacen('*', rest, 1))
                }
            })
    }
}

/// The aliases of the project in `dir`: the subpath imports of its `package.json`,
/// then the `paths` of its `tsconfig.json` and of the one SvelteKit generates.
/// shortcut: `extends` is not followed; SvelteKit's own aliases sit in the generated file.
fn aliases(root: &Path, dir: &str) -> Vec<(String, String)> {
    let read = |rel: &str| -> Option<String> {
        std::fs::read_to_string(root.join(relpath::join(dir, rel))).ok()
    };
    let mut found = Vec::new();
    if let Some(text) = read("package.json")
        && let Some(package) = object(&text)
    {
        for (pattern, value) in package.get_object("imports").into_iter().flat_map(entries) {
            if let Some(target) = condition(&value).and_then(|t| relpath::resolve(dir, &t)) {
                found.push((pattern, target));
            }
        }
    }
    for config in [".svelte-kit/tsconfig.json", "tsconfig.json"] {
        let Some(text) = read(config) else { continue };
        let Some(tsconfig) = object(&text) else {
            continue;
        };
        let Some(options) = tsconfig.get_object("compilerOptions") else {
            continue;
        };
        let base = relpath::join(dir, parent(config));
        let base = match options.get_string("baseUrl") {
            Some(url) => relpath::resolve(&base, url).unwrap_or(base),
            None => base,
        };
        for (pattern, value) in options.get_object("paths").into_iter().flat_map(entries) {
            let first = match &value {
                JsonValue::Array(targets) => targets.iter().find_map(|t| match t {
                    JsonValue::String(s) => Some(s.to_string()),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(target) = first.and_then(|t| relpath::resolve(&base, &t)) {
                found.push((pattern, target));
            }
        }
    }
    found.sort_by_key(|(pattern, _)| std::cmp::Reverse(pattern.len()));
    found
}

/// The top-level object of a JSON (or JSONC) text.
fn object(text: &str) -> Option<JsonObject<'_>> {
    match parse_to_value(text, &ParseOptions::default()) {
        Ok(Some(JsonValue::Object(object))) => Some(object),
        _ => None,
    }
}

/// The properties of `object` in their written order.
fn entries<'a>(object: &JsonObject<'a>) -> impl Iterator<Item = (String, JsonValue<'a>)> {
    object.clone().into_iter().map(|(k, v)| (k.into_owned(), v))
}

/// The path a subpath import maps to: a string, or the first string under its
/// conditions (`{ "svelte": "./x.svelte", "default": "./x.js" }`).
fn condition(value: &JsonValue<'_>) -> Option<String> {
    match value {
        JsonValue::String(s) => Some(s.to_string()),
        JsonValue::Object(conditions) => entries(conditions).find_map(|(_, v)| condition(&v)),
        _ => None,
    }
}

/// Each `<script>` block: its text and the 0-based line of the file it starts on.
fn scripts(source: &str) -> Vec<(&str, usize)> {
    static SCRIPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<script\b[^>]*>(.*?)</script\s*>").expect("valid regex"));
    SCRIPT
        .captures_iter(source)
        .filter_map(|c| c.get(1))
        .map(|m| (m.as_str(), source[..m.start()].matches('\n').count()))
        .collect()
}

impl Adapter for Svelte {
    fn name(&self) -> &'static str {
        "svelte"
    }

    fn handles(&self, path: &Path) -> bool {
        super::registry::is(path, &super::registry::SVELTE)
    }

    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>> {
        let mut found = Vec::new();
        for (script, offset) in scripts(source) {
            let tree = parse(&self.language, path, script)?;
            found.extend(
                captured_nodes(&self.query, &tree, script)
                    .into_iter()
                    .map(|node| Import {
                        specifier: node
                            .utf8_text(script.as_bytes())
                            .unwrap_or_default()
                            .to_string(),
                        line: offset + node.start_position().row + 1,
                    }),
            );
        }
        Ok(found)
    }

    fn resolve(&self, file: &Path, specifier: &str) -> Target {
        let file = slash(file.strip_prefix(&self.root).unwrap_or(file));
        let base = if specifier.starts_with("./") || specifier.starts_with("../") {
            relpath::resolve(parent(&file), specifier)
        } else {
            self.alias(&file, specifier)
        };
        match base.and_then(|base| self.probe(&base)) {
            Some(found) => Target::Internal(PathBuf::from(found).into()),
            // A package, a virtual module (`$app/state`), or a generated one (`./$types`).
            None => Target::External(specifier.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_imports_of_every_script_block_with_their_lines() {
        let svelte = Svelte::new(Path::new("/nonexistent"), &[]).unwrap();
        let src = r#"<script module>
  export { helper } from './helper.js';
</script>

<script lang="ts">
  // import { gone } from './commented';
  import {
    a,
    b,
  } from '#lib/api.js';
  import './side-effect.css';
  const text = "import x from './in-a-string'";
  const lazy = () => import('./Lazy.svelte');
</script>

<p>import nothing from './markup'</p>
"#;
        let got: Vec<_> = svelte
            .imports(Path::new("A.svelte"), src)
            .unwrap()
            .into_iter()
            .map(|i| (i.specifier, i.line))
            .collect();
        assert_eq!(
            got,
            [
                ("./helper.js".to_string(), 2),
                ("#lib/api.js".to_string(), 10),
                ("./side-effect.css".to_string(), 11),
                ("./Lazy.svelte".to_string(), 13),
            ]
        );
    }

    #[test]
    fn resolves_like_vite_and_typescript() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "web/package.json",
            r##"{ "imports": { "#lib/*": "./src/lib/*", "#cfg": { "types": "./cfg.d.ts", "default": "./src/cfg.ts" } } }"##,
        );
        // Generated by SvelteKit, so ignored by git and absent from the walk.
        write(
            "web/.svelte-kit/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "$lib": ["../src/lib"], "$lib/*": ["../src/lib/*"] } } }"#,
        );
        write(
            "web/tsconfig.json",
            "{\n  // comments are allowed\n  \"compilerOptions\": { \"paths\": { \"~/*\": [\"./src/*\"] } }\n}",
        );
        let files = [
            "web/package.json",
            "web/tsconfig.json",
            "web/src/cfg.ts",
            "web/src/lib/api.ts",
            "web/src/lib/auth.svelte.ts",
            "web/src/lib/index.ts",
            "web/src/lib/ui/Button.svelte",
            "web/src/lib/ui/index.ts",
            "web/src/routes/+page.svelte",
        ];
        for f in &files[2..] {
            write(f, "");
        }
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let svelte = Svelte::new(root, &files).unwrap();
        let page = root.join("web/src/routes/+page.svelte");
        let go = |spec: &str| svelte.resolve(&page, spec);
        let internal = |p: &str| Target::Internal(PathBuf::from(p).into());
        assert_eq!(go("#lib/api.js"), internal("web/src/lib/api.ts"));
        assert_eq!(
            go("#lib/auth.svelte.js"),
            internal("web/src/lib/auth.svelte.ts")
        );
        assert_eq!(go("#lib/ui"), internal("web/src/lib/ui/index.ts"));
        assert_eq!(go("#cfg"), internal("web/src/cfg.ts"));
        assert_eq!(go("$lib"), internal("web/src/lib/index.ts"));
        assert_eq!(
            go("$lib/ui/Button.svelte"),
            internal("web/src/lib/ui/Button.svelte")
        );
        assert_eq!(go("~/lib/api"), internal("web/src/lib/api.ts"));
        assert_eq!(go("../lib/api.js"), internal("web/src/lib/api.ts"));
        assert_eq!(go("svelte"), Target::External("svelte".into()));
        assert_eq!(go("$app/state"), Target::External("$app/state".into()));
        assert_eq!(go("./$types"), Target::External("./$types".into()));
    }
}
