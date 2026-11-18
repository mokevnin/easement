//! What easement reads from code, whatever its language: the imports a file
//! writes, the dependencies they make and what an adapter answers.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tree_sitter::Node;

use super::parts;

/// One dependency found in a source file.
#[derive(Debug, Clone)]
pub struct Import {
    /// The specifier as written in the source, without quotes.
    pub specifier: String,
    /// 1-based line number.
    pub line: usize,
    /// How it is written, which tells what its index records of it.
    pub form: Form,
    /// The text its index records it on, when that is not the specifier: the
    /// `y as z` of Python's `from . import y as z`, whose specifier is `.y`.
    pub recorded_on: Option<String>,
}

impl Import {
    /// The import of `specifier` in `form` at `line`, recorded on its specifier.
    pub fn new(specifier: impl Into<String>, line: usize, form: Form) -> Self {
        Self {
            specifier: specifier.into(),
            line,
            form,
            recorded_on: None,
        }
    }

    /// The import of `specifier` in `form`, at the line `node` starts on.
    pub(crate) fn at(node: Node, specifier: impl Into<String>, form: Form) -> Self {
        Self::new(specifier, node.start_position().row + 1, form)
    }

    /// The text its index records it on: [`Import::recorded_on`], else the specifier.
    pub fn recorded_text(&self) -> &str {
        self.recorded_on.as_deref().unwrap_or(&self.specifier)
    }
}

/// How an import is written, as far as it decides whether an index records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Form {
    /// A form the index records whenever it resolves it: `import … from`, Go's `import`.
    Static,
    /// An import for its side effects alone (`import "./polyfill"`): the index
    /// records it only when its target is a module, with an `import` or `export`.
    SideEffect,
    /// An import whose target is computed at run time (`` import(`./${name}`) ``,
    /// `import.meta.glob`): the text names no one target, and the index records none.
    Computed,
    /// An import call: a call that imports the module its literal argument names,
    /// which the index may not record (`require("./x")` where the compiler does not
    /// know `require`, `importlib.import_module("a.b")`). `as_import` says whether
    /// the argument is read as the language reads the specifier of an import
    /// (`require`'s), or in the call's own terms (a package it is given), so that
    /// its text names no file.
    ImportCall { as_import: bool },
}

/// Why the dependency of an import a file writes is not in the graph: the index
/// takes it to no source target, and its text names none either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cause {
    /// The index holds no module on it.
    NoTarget,
    /// The index takes it to a file of the project that the walk leaves out (one
    /// `.gitignore`, `.ignore` or git's excludes hide), which easement never reads.
    Unwalked,
    /// The index holds only a wildcard declaration on it (`declare module "*.css"`).
    Wildcard,
    /// The index holds nothing of an import for its side effects alone ([`Form::SideEffect`]).
    SideEffect,
    /// Its target is computed at run time ([`Form::Computed`]).
    Computed,
    /// The index holds nothing of an import call ([`Form::ImportCall`]).
    ImportCall,
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
    /// For a module of several files (a Go package): the names the file writes that
    /// its other files define. The module stands for them, so no dependency of their
    /// own does and no `mentions` hold them. Empty for a module of one file.
    pub elsewhere: Vec<Elsewhere>,
}

/// A name a file writes through a module that another file of the module defines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Elsewhere {
    /// The file that defines it.
    pub file: String,
    pub mention: Mention,
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
    /// The symbol of the name the alias stands for; `None` for a module.
    pub symbol: Option<String>,
    /// What the symbol rules call that name, when it is one the index gives a call of.
    pub called: Option<Qualified>,
}

impl Aliased {
    /// The use as a line that refers to the target: the name the alias stands for
    /// there, or, for a module, the module named without a name of it.
    pub fn mention(&self) -> Mention {
        Mention {
            line: self.line,
            symbol: self.symbol.clone(),
            called: self.called.clone(),
            bare: self.symbol.is_none(),
        }
    }
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
    /// For a name: what the symbol rules call it. `None` for a module, and for a
    /// name the index gives no such call of.
    pub called: Option<Qualified>,
    /// For a module: the file writes no name through any module of this file,
    /// neither on a line that names one nor a name defined in one, so the index
    /// gives none of its uses to a name (`import "./polyfill"`). `false` for a name.
    pub bare: bool,
}

/// A name as the symbol rules call it: the qualifier and the name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qualified {
    pub qualifier: String,
    pub name: String,
}

/// `qualifier.name`, as a symbol violation's target writes a name.
impl std::fmt::Display for Qualified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.qualifier, self.name)
    }
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
            elsewhere: Vec::new(),
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

/// A type of the project that implements or extends another type of the
/// project, directly or through its ancestors: a fact of its own, never a
/// dependency (ADR 0008).
#[derive(Debug, Clone)]
pub struct Implementation {
    /// The type that implements or extends.
    pub sub: TypeDef,
    /// The type it implements or extends.
    pub supertype: TypeDef,
    /// The types of the project between the two on some chain of ancestors, by
    /// file and line.
    pub between: Vec<TypeDef>,
}

/// A declaration whose index gives no relations although it may implement or
/// extend a type, with the files of the project its lines refer to.
#[derive(Debug, Clone)]
pub struct UnrelatedDeclaration {
    pub file: String,
    /// The 1-based line it starts on.
    pub line: usize,
    /// Why its index gives it no relations.
    pub because: &'static str,
    /// The files that define the names its lines refer to.
    pub writes: std::collections::BTreeSet<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
