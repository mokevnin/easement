use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

pub mod go;
pub mod kotlin;
pub mod parts;
pub mod registry;
pub mod swift;
pub mod typescript;

/// One dependency found in a source file.
#[derive(Debug, Clone)]
pub struct Import {
    /// The specifier as written in the source, without quotes.
    pub specifier: String,
    /// 1-based line number.
    pub line: usize,
}

/// A node of the dependency graph: a file or a package directory inside the
/// project (what a Go import names), relative to the root. A dependency leaves
/// one and reaches one; the file and line it is written at are its place, kept
/// apart in [`crate::lang::Dep`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unit {
    path: PathBuf,
    part: Option<parts::Part>,
}

impl Unit {
    /// A part of the file at `path`.
    pub fn in_part(path: &Path, part: parts::Part) -> Self {
        Self {
            path: path.into(),
            part: Some(part),
        }
    }

    /// Whether the unit is code that only a test build has.
    pub fn is_test(&self) -> bool {
        self.part
            .as_ref()
            .is_some_and(|part| part.kind == Some(parts::Kind::Test))
    }

    /// The name of the part of the file, when it is one.
    pub fn part_name(&self) -> Option<&str> {
        self.part.as_ref().map(|part| part.name.as_str())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The path with `/` separators, as components and rules read it.
    pub fn key(&self) -> String {
        crate::relpath::slash(&self.path)
    }
}

impl From<PathBuf> for Unit {
    fn from(path: PathBuf) -> Self {
        Self { path, part: None }
    }
}

impl From<&Path> for Unit {
    fn from(path: &Path) -> Self {
        Self {
            path: path.into(),
            part: None,
        }
    }
}

impl From<&str> for Unit {
    fn from(path: &str) -> Self {
        Self {
            path: path.into(),
            part: None,
        }
    }
}

/// Where an import points to after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file or package directory inside the project.
    Internal(Unit),
    /// A third-party or standard-library module.
    External(String),
}

/// A resolved dependency of one project file.
#[derive(Debug, Clone)]
pub struct Dep {
    /// Where it is written: diff mode, suppressions, the baseline and the
    /// report read the file and the line.
    pub file: PathBuf,
    pub line: usize,
    pub specifier: String,
    pub target: Target,
    /// The node it leaves: the file itself.
    pub from: Unit,
    /// Whether the code names a module or writes a name of one.
    pub written: Written,
}

impl Dep {
    /// A module that `file` names at `line`, as a whole file.
    pub fn module(
        file: impl Into<PathBuf>,
        line: usize,
        specifier: impl Into<String>,
        target: Target,
    ) -> Self {
        let file = file.into();
        Dep {
            from: Unit::from(file.as_path()),
            file,
            line,
            specifier: specifier.into(),
            target,
            written: Written::Module,
        }
    }
}

/// What the code writes that makes a dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Written {
    /// A module it names: an import, a qualified path, a manifest entry.
    Module,
    /// A name at the top level of a module that another file defines, also when a
    /// re-export reaches it (`chunk::Block` of `blob`).
    Name,
}

/// A type a file defines: a class, a struct, an interface, a trait.
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub file: String,
    pub line: usize,
    pub name: String,
}

/// A method or function with the lines it spans, 1-based and inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub symbol: String,
    pub first: usize,
    pub last: usize,
}

/// A name that more than one component declares: the files that declare it
/// stand for the same name, and which one an import means is not written down
/// in the code.
#[derive(Debug, Clone)]
pub struct Ambiguous {
    /// The name as it is written, `package.Name`.
    pub name: String,
    /// The files that declare it, in the order of their paths.
    pub files: Vec<String>,
}

/// A language adapter: extracts imports from a file and resolves them, for a
/// language no indexer reads (see `registry` for the rest of a language).
pub trait Adapter {
    fn name(&self) -> &'static str;
    fn handles(&self, path: &Path) -> bool;
    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>>;
    fn resolve(&self, file: &Path, specifier: &str) -> Target;
    /// The name an import of `specifier` names, when more than one component
    /// of the project declares it: which one the import means is not written
    /// down in the code.
    fn ambiguity(&self, specifier: &str) -> Option<Ambiguous>;
}

pub fn adapters<'a>(
    root: &'a Path,
    map: &'a crate::components::ComponentMap,
) -> Result<Vec<Box<dyn Adapter + 'a>>> {
    Ok(vec![
        Box::new(kotlin::Kotlin::new(root, map)?),
        Box::new(swift::Swift::new(root, map)?),
    ])
}

/// The languages the adapters read: the ones no indexer covers.
pub fn adapter_languages() -> [&'static registry::Language; 2] {
    [&registry::KOTLIN, &registry::SWIFT]
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

/// Path relative to `root` with `/` separators, if `path` is inside `root`.
pub fn relative(root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}
