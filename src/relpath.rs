//! A project file is named by a `/`-separated path relative to the project root.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

/// The real path of `path`, whether or not it exists yet: the real path of the
/// deepest part that exists, the rest resolved by name. A relative path is
/// taken from the working directory.
pub fn real(path: &Path) -> Result<PathBuf> {
    soft_canonicalize::soft_canonicalize(path)
        .with_context(|| format!("cannot resolve the path {}", path.display()))
}

/// The extension of the file, `""` without one: `ts` of `src/a.d.ts`.
pub fn extension(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
}

/// The directory of the file, `""` at the root.
pub fn parent(path: &str) -> &str {
    Path::new(path)
        .parent()
        .and_then(|dir| dir.to_str())
        .unwrap_or("")
}

/// The name of the file, the last part of its path.
pub fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

/// `name` in the directory `dir`, `""` being the root.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// `rel`, a `/`-separated path written in `dir`, relative to the root: `.` and `..`
/// resolved by the letters alone. `None` if it climbs above the root.
pub fn resolve(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// Whether `path` is the directory `dir` or below it; every path is below the root `""`.
pub fn is_under(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// `/`-separated string form of a relative path, used for matching and output.
pub fn slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Matches each pattern and everything below it; `key` names the patterns in an error.
pub fn glob_set(patterns: &[String], key: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let p = p.trim_end_matches('/');
        builder.add(Glob::new(p).with_context(|| format!("bad {key} pattern `{p}`"))?);
        builder.add(Glob::new(&format!("{p}/**"))?);
    }
    Ok(builder.build()?)
}

/// Project walker: honours .gitignore (even outside a git checkout) and
/// always skips the third-party directories of a toolchain (`node_modules`).
pub fn walker(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder.require_git(false).filter_entry(|e| {
        !e.file_name()
            .to_str()
            .is_some_and(crate::lang::registry::is_third_party)
    });
    builder
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path is real through its links, `..` included, as `realpath` takes it:
    /// `link/../x` is beside what the link points to, and a part that does not
    /// exist yet is kept by name.
    #[cfg(unix)]
    #[test]
    fn a_real_path_follows_links_before_it_goes_up() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::os::unix::fs::symlink(root.join("a/b"), root.join("link")).unwrap();
        assert_eq!(
            real(&root.join("link/../x.yml")).unwrap(),
            root.join("a/x.yml")
        );
        assert_eq!(
            real(&dir.path().join("link/new/../y.yml")).unwrap(),
            root.join("a/b/y.yml")
        );
    }

    #[test]
    fn splits_a_project_path() {
        assert_eq!(extension("src/a.d.ts"), "ts");
        assert_eq!(extension("Makefile"), "");
        assert_eq!(extension("v1.2/Makefile"), "");
        assert_eq!(parent("src/web/a.ts"), "src/web");
        assert_eq!(parent("a.ts"), "");
        assert_eq!(file_name("src/web/a.ts"), "a.ts");
        assert_eq!(file_name("a.ts"), "a.ts");
    }

    #[test]
    fn joins_and_resolves_within_the_project() {
        assert_eq!(join("", "go.mod"), "go.mod");
        assert_eq!(join("svc/api", "go.mod"), "svc/api/go.mod");
        assert_eq!(resolve("web/src", "../lib/./a"), Some("web/lib/a".into()));
        assert_eq!(resolve("", "./a"), Some("a".into()));
        assert_eq!(resolve("web", "../.."), None);
        assert!(is_under("src/a.ts", ""));
        assert!(is_under("src", "src"));
        assert!(is_under("src/a.ts", "src"));
        assert!(!is_under("srcs/a.ts", "src"));
    }

    /// A project path: one to four names, a name of letters with an
    /// optional extension, none of them `.` or `..`.
    fn path() -> impl proptest::strategy::Strategy<Value = String> {
        proptest::collection::vec("[a-z]{1,3}(\\.[a-z]{1,2})?", 1..5)
            .prop_map(|names| names.join("/"))
    }

    /// The root `""` or a project path.
    fn dir() -> impl proptest::strategy::Strategy<Value = String> {
        proptest::prop_oneof![proptest::strategy::Just(String::new()), path()]
    }

    use proptest::strategy::Strategy as _;

    proptest::proptest! {
        #[test]
        fn a_path_is_its_directory_and_its_name(path in path()) {
            proptest::prop_assert_eq!(join(parent(&path), file_name(&path)), path);
        }

        /// Without `.` or `..`, `resolve` is `join`; each `..` takes one name
        /// off the directory, and one more than it has climbs above the root.
        #[test]
        fn resolve_climbs_one_directory_per_dot_dot(dir in dir(), rel in path(), up in 0usize..6) {
            proptest::prop_assert_eq!(resolve(&dir, &rel), Some(join(&dir, &rel)));
            let names: Vec<&str> = dir.split('/').filter(|n| !n.is_empty()).collect();
            let climbed = format!("{}{rel}", "../".repeat(up));
            let expected = (up <= names.len()).then(|| join(&names[..names.len() - up].join("/"), &rel));
            proptest::prop_assert_eq!(resolve(&dir, &climbed), expected);
            proptest::prop_assert_eq!(resolve(&dir, &format!("./{rel}/.")), Some(join(&dir, &rel)));
        }

        /// A path is under itself and the directories above it, never under a
        /// directory whose last name only begins its own.
        #[test]
        fn a_path_is_under_its_directories_only(dir in dir(), rest in path()) {
            let path = join(&dir, &rest);
            proptest::prop_assert!(is_under(&path, &dir));
            let longer = format!("{path}x");
            proptest::prop_assert!(is_under(&path, &path));
            proptest::prop_assert!(!is_under(&longer, &path));
            proptest::prop_assert!(!is_under(&dir, &path) || dir == path);
        }

        /// A pattern matches its own path and every path below it, and no path
        /// that only begins with its last name.
        #[test]
        fn a_glob_set_holds_a_path_and_what_is_below_it(pattern in path(), rest in path(), slash: bool) {
            let written = if slash { format!("{pattern}/") } else { pattern.clone() };
            let set = glob_set(&[written], "include").unwrap();
            proptest::prop_assert!(set.is_match(&pattern));
            proptest::prop_assert!(set.is_match(join(&pattern, &rest)));
            let (longer, below_longer) = (format!("{pattern}x"), format!("{pattern}x/{rest}"));
            proptest::prop_assert!(!set.is_match(longer));
            proptest::prop_assert!(!set.is_match(below_longer));
        }
    }
}
