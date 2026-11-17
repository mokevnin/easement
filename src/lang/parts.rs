//! The parts of a file that a dependency can belong to instead of the file:
//! code a language only builds for its tests, for example.

use std::ops::RangeInclusive;

/// What the core's own rules read of a part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Code that only a test build has: `tests: auto` takes it.
    Test,
}

/// A part of a file, as the language that found it names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Part {
    pub name: String,
    pub kind: Option<Kind>,
}

/// The lines of a file (1-based, both ends included) that a part holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub lines: RangeInclusive<usize>,
    pub part: Part,
}

impl Span {
    /// Test-only code on `lines`: `1..=usize::MAX` for a whole file.
    pub fn test(lines: RangeInclusive<usize>) -> Self {
        Self {
            lines,
            part: Part {
                name: "cfg(test)".to_string(),
                kind: Some(Kind::Test),
            },
        }
    }
}

/// What a finder found in a source, and what it could not tell.
#[derive(Debug, Default)]
pub struct Found {
    pub spans: Vec<Span>,
    pub doubts: Vec<Doubt>,
    /// The test-only `mod name;` (one `#[cfg(test)]` governs, or one in test code): for
    /// each, the files its code may be in, of which the one that exists is test code as
    /// a whole.
    pub files: Vec<Vec<String>>,
    /// The other `mod name;` of the file, test code as well when the file is.
    pub modules: Vec<Module>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Module {
    /// The files its code may be in.
    Files(Vec<String>),
    /// Where its file is cannot be told from the declaring file alone.
    Unplaced(Doubt),
}

/// Why test code of a file may be judged as the file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Doubt {
    /// The source is not valid UTF-8.
    Unreadable,
    /// An item whose end was not found (the braces do not match).
    Unbalanced,
    /// A test-only `mod name;` names a file the project does not have.
    ModuleInAnotherFile,
    /// An inner `#![cfg(test)]`, or a test-only `mod name;`, inside an inline module.
    InsideModule,
    /// A `#[path]` puts the file of a test-only `mod name;` somewhere else.
    ModulePath,
}

impl Doubt {
    /// The line of the report for the `files` that share it.
    pub fn reason(self, files: &str) -> String {
        match self {
            Doubt::Unreadable => format!(
                "{files}: not valid UTF-8, so their `#[cfg(test)]` code is judged as the file's, not as `tests`"
            ),
            Doubt::Unbalanced => format!(
                "{files}: easement could not find the end of a `#[cfg(test)]` item (the braces do not match), so it is judged as the file's, not as `tests`"
            ),
            Doubt::InsideModule => format!(
                "{files}: a `#![cfg(test)]`, or a test-only `mod name;`, inside an inline module is not placed, so it is judged as the file's, not as `tests`"
            ),
            Doubt::ModulePath => format!(
                "{files}: a `#[path]` puts the file of a test-only `mod name;` somewhere easement does not look, so it is judged as its component's, not as `tests`"
            ),
            Doubt::ModuleInAnotherFile => format!(
                "{files}: a test-only `mod name;` names a file that is not in the project, so it is not judged as `tests`; the file is expected next to the module or under a directory of its name"
            ),
        }
    }
}
