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
    /// How it is written, which tells what its index records of it.
    pub form: Form,
}

/// How an import is written, as far as it decides whether an index records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Form {
    /// A form the index records whenever it resolves it: `import … from`, Go's `import`.
    Static,
    /// An import for its side effects alone (`import "./polyfill"`): the index
    /// records it only when its target is a module, with an `import` or `export`.
    SideEffect,
    /// A `require()` call where the compiler may not know `require`, and so records
    /// nothing of it.
    Require,
    /// An import whose target is computed at run time (`` import(`./${name}`) ``,
    /// `import.meta.glob`): the text names no one target, and the index records none.
    Computed,
}

/// Why the dependency of an import a file writes is not in the graph: the index
/// takes it to no source target, and its text names none either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cause {
    /// The index holds no module on it.
    NoTarget,
    /// The index holds only a wildcard declaration on it (`declare module "*.css"`).
    Wildcard,
    /// The index holds nothing of an import for its side effects alone ([`Form::SideEffect`]).
    SideEffect,
    /// The index holds nothing of a `require()` call ([`Form::Require`]).
    Require,
    /// Its target is computed at run time ([`Form::Computed`]).
    Computed,
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
impl From<String> for Unit {
    fn from(path: String) -> Self {
        Self {
            path: path.into(),
            part: None,
        }
    }
}

/// Where an import points to after resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// A file or package directory inside the project.
    Internal(Unit),
    /// A third-party or standard-library module.
    External(String),
}

impl Target {
    /// What it is called: the path of a file of the project, with `/` separators,
    /// or the external module.
    pub fn name(&self) -> String {
        match self {
            Target::Internal(unit) => unit.key(),
            Target::External(module) => module.clone(),
        }
    }
}

/// A resolved dependency of one project file.
#[derive(Debug, Clone)]
pub struct Dep {
    /// Where it is written: diff mode, suppressions, the baseline and the
    /// report read the file and the line.
    pub file: PathBuf,
    pub line: usize,
    pub specifier: String,
    /// The target the adapter found first by path, or the external module.
    /// Readers that must not lose a copy read `candidates`; `decision::decide`
    /// picks the copy an import means.
    pub target: Target,
    /// All files or directories that may declare the imported name, in the
    /// order of their paths. Empty when the declaration is unambiguous.
    pub candidates: Vec<String>,
    /// The node it leaves: the file itself.
    pub from: Unit,
    /// Whether the code names a module or writes a name of one.
    pub written: Written,
    /// Every line of `file` that refers to the file of the target, `line` among
    /// them, whatever part it is in: only an index tells them; empty otherwise.
    pub mentions: Vec<Mention>,
    /// For a name: the modules it may have come through.
    pub through: Through,
    /// Every use of an alias that stands for the target, by line: empty when the
    /// file writes the target itself.
    pub aliased: Vec<Aliased>,
    /// For a name: each way it came through re-exports, as the line of every module
    /// on the way that re-exports it, from the module the file names to the last one
    /// before the file that defines it. Empty when the file writes the name from that
    /// file, or when the index shows no module on the way re-exporting it.
    pub routes: Vec<Vec<Hop>>,
}

/// A line of a module that re-exports a name, on the way to the file that defines it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hop {
    pub file: String,
    pub line: usize,
}

/// A use of an alias that stands for the target of a dependency: the line of the
/// file that writes it, and the definition of each alias from the one written to
/// the one whose definition names the target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Aliased {
    pub line: usize,
    pub chain: Vec<AliasDefinition>,
}

/// Where an alias is defined.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AliasDefinition {
    pub file: String,
    pub line: usize,
}

/// The modules a name may have come into a file through, one of which may
/// re-export it from the file that defines it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Through {
    /// A module, or a name whose file imports no other module it could come through.
    #[default]
    Nothing,
    /// The files of the other modules its line names (`chunk` of `chunk::Block`).
    Line(Vec<String>),
    /// When its line names none: the files of the modules the file imports whole,
    /// on lines that name nothing else (`use chunk::*`). Such a line may as well
    /// import one class (`use App\Domain\Order;`), so only a module that refers to
    /// the name itself can have brought it.
    Glob(Vec<String>),
}

impl Through {
    pub fn files(&self) -> &[String] {
        match self {
            Through::Nothing => &[],
            Through::Line(files) | Through::Glob(files) => files,
        }
    }
}

/// A line of a file that refers to another file: to a module of it, or to a name
/// it defines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mention {
    pub line: usize,
    /// The symbol of the name written there; `None` for a module.
    pub symbol: Option<String>,
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
            candidates: Vec::new(),
            written: Written::Module,
            mentions: Vec::new(),
            through: Through::Nothing,
            aliased: Vec::new(),
            routes: Vec::new(),
        }
    }

    /// Every internal target the import may mean, with the part it names: all
    /// `candidates` when several places may declare it, otherwise the file or
    /// directory it resolves to. Empty for an external module. Readers that must
    /// not lose a copy, or the part a target belongs to, ask here.
    pub fn targets(&self) -> Vec<Unit> {
        if !self.candidates.is_empty() {
            return self
                .candidates
                .iter()
                .map(|candidate| Unit::from(candidate.as_str()))
                .collect();
        }
        match &self.target {
            Target::Internal(unit) => vec![unit.clone()],
            Target::External(_) => Vec::new(),
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

/// The result of resolving an import: the deterministic target, every
/// candidate that may declare the imported name, and the name that is
/// ambiguous when more than one candidate may.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The first candidate by path, or the external module: `decision::decide`
    /// picks the copy an import means.
    pub target: Target,
    pub candidates: Vec<String>,
    /// The name more than one candidate may declare, when they may: only a
    /// declaration that one place should hold, never overloads or extensions.
    pub ambiguity: Option<String>,
}

/// A language adapter: extracts imports from a file and resolves them, for a
/// language no indexer reads (see `registry` for the rest of a language).
pub trait Adapter {
    fn name(&self) -> &'static str;
    fn handles(&self, path: &Path) -> bool;
    fn imports(&self, path: &Path, source: &str) -> Result<Vec<Import>>;
    /// The target of an import of `specifier`, every candidate that may
    /// declare it, and the name that is ambiguous when more than one
    /// candidate may.
    fn resolve(&self, file: &Path, specifier: &str) -> Resolution;
}

/// The adapters of the languages that have a file among `files`, everything but
/// the directories that the walk of `root` saw (relative to it, in the order of the
/// walk): the excluded files and the links too, which stay targets that an import
/// resolves to. Each adapter reads
/// what it needs from this list, so the tree is walked once.
pub fn adapters(root: &Path, files: &[PathBuf]) -> Result<Vec<Box<dyn Adapter>>> {
    let mut adapters: Vec<Box<dyn Adapter>> = Vec::new();
    for &language in &registry::LANGUAGES {
        let Some(build) = language.adapter else {
            continue;
        };
        if !files.iter().any(|f| registry::is(f, language)) {
            continue;
        }
        adapters.push(build(root, files)?);
    }
    Ok(adapters)
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

/// What the walk of `root` sees but the directories, relative to it, in the order
/// of the walk: what the scan gives the adapters.
#[cfg(test)]
pub(crate) fn walked(root: &Path) -> Vec<PathBuf> {
    crate::relpath::walker(root)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| !t.is_dir()))
        .filter_map(|e| relative(root, e.path()))
        .collect()
}

/// Path relative to `root` with `/` separators, if `path` is inside `root`.
pub fn relative(root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(files: &[&str]) -> Vec<&'static str> {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        adapters(Path::new("/nonexistent"), &files)
            .unwrap()
            .iter()
            .map(|a| a.name())
            .collect()
    }

    /// A dependency on a part keeps it: a reader that asks for its targets must
    /// not lose the part the policy judges.
    #[test]
    fn targets_keep_the_part_of_a_single_target() {
        let part = parts::Part {
            name: "feature/fake".into(),
            kind: None,
        };
        let dep = Dep::module(
            "crates/core/Cargo.toml",
            6,
            "fakes",
            Target::Internal(Unit::in_part(Path::new("crates/core/Cargo.toml"), part)),
        );
        let targets = dep.targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].part_name(), Some("feature/fake"));
    }

    #[test]
    fn a_project_gets_the_adapters_of_the_languages_it_has_files_of() {
        assert!(names(&["main.go", "web/app.ts", "README.md"]).is_empty());
        assert!(names(&[]).is_empty());
        assert_eq!(names(&["main.go", "app/A.kt"]), ["kotlin"]);
        // A Swift file without a `Package.swift` is still read, its imports external.
        assert_eq!(names(&["App/main.swift"]), ["swift"]);
        assert_eq!(
            names(&["build.gradle.kts", "Package.swift"]),
            ["kotlin", "swift"]
        );
    }
}
