//! Running the adapters of the languages no indexer reads.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::Adapter;
use super::registry;

/// The adapters of the languages that have a file among `files`, everything but
/// the directories that the walk of `root` saw (relative to it, in the order of the
/// walk): the excluded files and the links too, which stay targets that an import
/// resolves to. Each adapter reads
/// what it needs from this list, so the tree is walked once.
pub fn build(root: &Path, files: &[PathBuf]) -> Result<Vec<Box<dyn Adapter>>> {
    let mut adapters: Vec<Box<dyn Adapter>> = Vec::new();
    for &language in &registry::LANGUAGES {
        let Some(build) = language.adapter() else {
            continue;
        };
        if !files.iter().any(|f| registry::is(f, language)) {
            continue;
        }
        adapters.push(build(root, files)?);
    }
    Ok(adapters)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(files: &[&str]) -> Vec<&'static str> {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        build(Path::new("/nonexistent"), &files)
            .unwrap()
            .iter()
            .map(|a| a.name())
            .collect()
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
