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
    /// Test-only code on `lines` (`1..=usize::MAX` for a whole file), in the part
    /// its language names `name`.
    pub fn test(name: &str, lines: RangeInclusive<usize>) -> Self {
        Self {
            lines,
            part: Part {
                name: name.to_string(),
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
    /// The other files the source declares as test code alone: for each, the files
    /// its code may be in, of which the one that exists is test code as a whole.
    pub test_files: Vec<Vec<String>>,
    /// The other files the source declares as its own code: test code as well when
    /// the source turns out to be.
    pub modules: Vec<Module>,
}

/// Code a source declares that is written in another file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Module {
    /// The files its code may be in.
    Files(Vec<String>),
    /// Where its file is cannot be told from the declaring source alone.
    Unplaced(Doubt),
}

/// Why test code of a file may be judged as the file's, as its language says it:
/// what follows the files in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Doubt {
    pub why: &'static str,
}

impl Doubt {
    /// The line of the report for the `files` that share it.
    pub fn reason(self, files: &str) -> String {
        format!("{files}: {}", self.why)
    }
}
