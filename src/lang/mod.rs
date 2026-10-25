use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

use crate::config::Config;

pub mod csharp;
pub mod go;
pub mod java;
pub mod kotlin;
pub mod php;
pub mod python;
pub mod rust;
pub mod swift;
pub mod ts;

/// One dependency found in a source file.
#[derive(Debug, Clone)]
pub struct Import {
    /// The specifier as written in the source, without quotes.
    pub specifier: String,
    /// 1-based line number.
    pub line: usize,
}

/// A use of a name that an import brings in: `db.Client` in Go is the
/// specifier of `db`'s import and the name `Client`.
#[derive(Debug, Clone)]
pub struct SymbolUse {
    /// The specifier of the import the name comes through.
    pub specifier: String,
    pub name: String,
    /// 1-based line number.
    pub line: usize,
}

/// Where an import points to after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file or package directory inside the project, relative to the root.
    Internal(PathBuf),
    /// A third-party or standard-library module.
    External(String),
    /// Looks internal but could not be resolved.
    Unresolved,
}

/// A language adapter: extracts imports from a file and resolves them.
///
/// Adding a language means implementing this trait; the core never sees
/// language-specific details.
pub trait Adapter {
    fn name(&self) -> &'static str;
    fn handles(&self, path: &Path) -> bool;
    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>>;
    fn resolve(&self, file: &Path, specifier: &str) -> Target;
    /// Names used through imports, for the symbol rules. Adapters that cannot
    /// tell report none, and those rules then say nothing about their files.
    fn symbol_uses(&self, _path: &Path, _source: &str) -> Result<Vec<SymbolUse>> {
        Ok(Vec::new())
    }
    /// Lines of imports the adapter cannot analyse because the target is
    /// computed at run time (`importlib.import_module(name)`, `require(path)`).
    fn dynamic_imports(&self, _path: &Path, _source: &str) -> Result<Vec<usize>> {
        Ok(Vec::new())
    }
}

pub fn adapters(root: &Path, config: &Config) -> Result<Vec<Box<dyn Adapter>>> {
    Ok(vec![
        Box::new(go::Go::new(root)?),
        Box::new(ts::TypeScript::new(root)?),
        Box::new(python::Python::new(root, &config.python.roots)?),
        Box::new(java::Java::new(root)?),
        Box::new(rust::Rust::new(root)?),
        Box::new(csharp::CSharp::new(root)?),
        Box::new(kotlin::Kotlin::new(root)?),
        Box::new(php::Php::new(root)?),
        Box::new(swift::Swift::new(root)?),
    ])
}

pub(crate) fn parse(language: &Language, path: &Path, source: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language)?;
    parser
        .parse(source, None)
        .with_context(|| format!("cannot parse {}", path.display()))
}

/// Every node captured as `@node` by `query`, in document order.
pub(crate) fn captured_nodes<'t>(query: &Query, tree: &'t Tree, source: &str) -> Vec<Node<'t>> {
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut nodes = Vec::new();
    while let Some(m) = matches.next() {
        nodes.extend(m.captures().iter().map(|c| c.node));
    }
    nodes
}

/// Lines of the `@arg` captures of `query` that `is_dynamic` accepts, given the
/// call's `@fn` text; the query matches calls and their first argument.
pub(crate) fn dynamic_calls(
    language: &Language,
    query: &Query,
    path: &Path,
    source: &str,
    is_dynamic: impl Fn(&str, Node) -> bool,
) -> Result<Vec<usize>> {
    let tree = parse(language, path, source)?;
    let fn_index = query.capture_index_for_name("fn");
    let arg_index = query
        .capture_index_for_name("arg")
        .context("query has no @arg capture")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut lines = Vec::new();
    while let Some(m) = matches.next() {
        let callee = fn_index
            .and_then(|i| m.captures().iter().find(|c| c.index == i))
            .and_then(|c| c.node.utf8_text(source.as_bytes()).ok())
            .unwrap_or_default();
        for capture in m.captures().iter().filter(|c| c.index == arg_index) {
            if is_dynamic(callee, capture.node) {
                lines.push(capture.node.start_position().row + 1);
            }
        }
    }
    Ok(lines)
}

/// Runs a tree-sitter query and returns the text of every `@spec` capture.
pub(crate) fn query_imports(
    language: &Language,
    query: &Query,
    path: &Path,
    source: &str,
) -> Result<Vec<Import>> {
    let tree = parse(language, path, source)?;
    let spec_index = query
        .capture_index_for_name("spec")
        .context("query has no @spec capture")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut imports = Vec::new();
    while let Some(m) = matches.next() {
        for capture in m.captures().iter().filter(|c| c.index == spec_index) {
            let text = capture.node.utf8_text(source.as_bytes())?;
            let specifier = text.trim_matches(|c| c == '"' || c == '\'' || c == '`');
            imports.push(Import {
                specifier: specifier.to_string(),
                line: capture.node.start_position().row + 1,
            });
        }
    }
    Ok(imports)
}

/// Path relative to `root` with `/` separators, if `path` is inside `root`.
pub fn relative(root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}
