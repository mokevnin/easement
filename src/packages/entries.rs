//! What the manifests of the project make entries of, for `unused`: the files a
//! manifest names as what its package runs or exports, and the files a framework
//! the package depends on loads by convention. A framework is a row of the table
//! of the kind of manifest that names it; the nearest manifest owns a file, so a
//! framework's conventions hold in its own package and not in one nested in it.

use std::collections::{BTreeMap, BTreeSet};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::Declares;
use crate::check::{self, Unjudged};
use crate::lang::registry::{self, Manifest};
use crate::relpath::{self, parent};

/// A framework that loads files of a package by convention.
pub struct Framework {
    pub name: &'static str,
    /// The dependency, in `dependencies` or `devDependencies`, that tells it.
    pub dependency: &'static str,
    /// The files it loads, as globs relative to the directory of the package.
    pub entries: &'static [&'static str],
    /// Its config files, as globs relative to the directory of the package.
    pub configs: &'static [&'static str],
    /// The settings of a config file that move its conventions.
    pub moves: &'static [Setting],
}

/// A setting of a framework's config file: its name as the framework's docs
/// spell it, and the key that writes it, found in the text as a whole word. A
/// key found where it means something else only leaves files unjudged.
pub struct Setting {
    pub name: &'static str,
    pub key: &'static str,
}

/// What the manifests make entries of.
#[derive(Debug, Default)]
pub struct Entries {
    pub files: BTreeSet<String>,
    /// The frameworks found, by package.
    pub frameworks: Vec<check::RecognisedFramework>,
    /// Packages whose files were all taken as entries, since what they load
    /// could not be told; each lists those files.
    pub untrusted: Vec<Unjudged>,
}

const CLOSE_MOVED: &str = "`unused` does not follow a convention a config file moves: review those files by hand; it judges them again once the config file no longer names the setting";

/// The entries the manifests among `files` make, reading each file `read`
/// returns the text of (`None` when it cannot be read). `others` are files the
/// checks do not read, whose config files still count.
pub fn entries(
    files: &[String],
    others: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Entries {
    let known: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let any: BTreeSet<&str> = known
        .iter()
        .copied()
        .chain(others.iter().map(String::as_str))
        .collect();
    let mut texts: BTreeMap<&str, Option<String>> = BTreeMap::new();
    let mut packages: BTreeMap<&Manifest, BTreeSet<&str>> = BTreeMap::new();
    for manifest in files {
        let Some(kind) = registry::manifest_of(manifest) else {
            continue;
        };
        let text = read(manifest);
        // A marker (`{"type": "module"}`) owns no file: the package is above it.
        if text.as_deref().map(|t| kind.declares(t)) != Some(Declares::Marker) {
            packages.entry(kind).or_default().insert(parent(manifest));
        }
        texts.insert(manifest, text);
    }
    let mut found = Entries::default();
    for (manifest, text) in texts {
        let Some(kind) = registry::manifest_of(manifest) else {
            continue;
        };
        let dir = parent(manifest);
        let Some(text) = text else {
            // What it exports is not known: every file under it may be.
            found.untrusted_all(
                files,
                dir,
                format!(
                    "`unused` could not read {manifest}, so it took every file under it as used"
                ),
                "make the manifest readable, then check again",
            );
            continue;
        };
        let Some(named) = kind.entries(&text, dir, &known) else {
            found.untrusted_all(
                files,
                dir,
                format!("`unused` could not parse {manifest}, so it took every file under it as used: what the package exports and the frameworks it uses are not known"),
                "fix the manifest, then check again",
            );
            continue;
        };
        found.files.extend(named);
        let owners = packages.get(&kind).cloned().unwrap_or_default();
        let owned = |list: &BTreeSet<&str>| -> Vec<String> {
            list.iter()
                .filter(|f| owner(f, &owners) == Some(dir))
                .map(|f| f.to_string())
                .collect()
        };
        for framework in kind.frameworks(&text) {
            found.frameworks.push(check::RecognisedFramework {
                name: framework.name.into(),
                package: manifest.into(),
                dependency: framework.dependency.into(),
            });
            let mine = owned(&known);
            match moved(framework, dir, &owned(&any), read) {
                Some(why) => {
                    found.files.extend(mine.iter().cloned());
                    found.untrusted.push(
                        Unjudged::new(format!(
                            "`unused` did not judge the files of {manifest}, which uses {}: {why}, so it took every file of the package as used",
                            framework.name
                        ))
                        .in_files(mine)
                        .closed_by(CLOSE_MOVED),
                    );
                }
                None => {
                    let globs = glob_set(framework.entries);
                    found.files.extend(
                        mine.into_iter()
                            .filter(|f| globs.is_match(relative(f, dir))),
                    );
                }
            }
        }
    }
    found
}

impl Entries {
    /// Every file under `dir` taken as an entry, with `reason`.
    fn untrusted_all(&mut self, files: &[String], dir: &str, reason: String, close: &str) {
        let under: Vec<String> = files
            .iter()
            .filter(|f| relpath::is_under(f, dir))
            .cloned()
            .collect();
        self.files.extend(under.iter().cloned());
        self.untrusted
            .push(Unjudged::new(reason).in_files(under).closed_by(close));
    }
}

/// Why the conventions of `framework` in the package in `dir` may be moved: a
/// config file of it among `files` sets a setting that moves them, or cannot
/// be read. `None` when they hold.
fn moved(
    framework: &Framework,
    dir: &str,
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let configs = glob_set(framework.configs);
    for config in files.iter().filter(|f| configs.is_match(relative(f, dir))) {
        let Some(text) = read(config) else {
            return Some(format!(
                "its config file {config} could not be read, and may move the files it loads"
            ));
        };
        if let Some(setting) = framework
            .moves
            .iter()
            .find(|setting| has_word(&text, setting.key))
        {
            return Some(format!(
                "{config} may set `{}`, which moves the files it loads",
                setting.name
            ));
        }
    }
    None
}

/// The directory of the package `file` belongs to: the nearest of `packages`
/// at or above it.
fn owner<'a>(file: &str, packages: &BTreeSet<&'a str>) -> Option<&'a str> {
    let mut dir = parent(file);
    loop {
        if let Some(found) = packages.get(dir) {
            return Some(found);
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent(dir);
    }
}

/// `file` relative to `dir`, which holds it.
fn relative<'a>(file: &'a str, dir: &str) -> &'a str {
    if dir.is_empty() {
        file
    } else {
        file.strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(file)
    }
}

/// Globs of a table, where `*` stays within one directory.
fn glob_set(patterns: &[&str]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .expect("a glob of a framework table is valid"),
        );
    }
    builder
        .build()
        .expect("a glob set of a framework table builds")
}

/// Whether `text` holds `word` with no letter, digit, `_` or `$` on either side.
fn has_word(text: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + word.len()..].chars().next().is_some_and(part)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVELTEKIT: &str = r#"{"devDependencies": {"@sveltejs/kit": "^2"}}"#;

    fn found(project: &[(&str, &str)]) -> Entries {
        let files: Vec<String> = project.iter().map(|(f, _)| f.to_string()).collect();
        let texts: BTreeMap<String, String> = project
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect();
        entries(&files, &[], &|f| texts.get(f).cloned())
    }

    #[test]
    fn a_framework_makes_entries_of_its_conventional_files_in_its_package_alone() {
        let got = found(&[
            ("package.json", r#"{"name": "root"}"#),
            ("apps/web/package.json", SVELTEKIT),
            ("apps/web/src/routes/+page.ts", ""),
            ("apps/web/src/routes/blog/[slug]/+page.server.ts", ""),
            ("apps/web/src/hooks.server.ts", ""),
            ("apps/web/src/params/id.ts", ""),
            ("apps/web/svelte.config.js", "export default { kit: {} };"),
            ("apps/web/src/lib/x.ts", ""),
            // A marker owns nothing: its files are the web package's.
            (
                "apps/web/src/routes/esm/package.json",
                r#"{"type": "module"}"#,
            ),
            ("apps/web/src/routes/esm/+server.ts", ""),
            // A nested package is its own, and does not depend on the framework.
            ("apps/web/tools/package.json", r#"{"name": "tools"}"#),
            ("apps/web/tools/src/routes/+page.ts", ""),
            // A sibling with the same layout does not either.
            ("apps/api/package.json", r#"{"name": "api"}"#),
            ("apps/api/src/routes/+page.ts", ""),
        ]);
        let files: Vec<&str> = found_files(&got);
        assert_eq!(
            files,
            [
                "apps/web/src/hooks.server.ts",
                "apps/web/src/params/id.ts",
                "apps/web/src/routes/+page.ts",
                "apps/web/src/routes/blog/[slug]/+page.server.ts",
                "apps/web/src/routes/esm/+server.ts",
                "apps/web/svelte.config.js",
            ]
        );
        assert_eq!(
            got.frameworks,
            [check::RecognisedFramework {
                name: "SvelteKit".into(),
                package: "apps/web/package.json".into(),
                dependency: "@sveltejs/kit".into(),
            }]
        );
        assert!(got.untrusted.is_empty());
    }

    fn found_files(got: &Entries) -> Vec<&str> {
        got.files.iter().map(String::as_str).collect()
    }

    #[test]
    fn next_routes_are_entries_at_the_root_and_under_src() {
        let got = found(&[
            ("package.json", r#"{"dependencies": {"next": "15"}}"#),
            ("app/page.tsx", ""),
            ("app/blog/[id]/layout.tsx", ""),
            ("app/api/route.ts", ""),
            ("app/sitemap.ts", ""),
            ("app/components/button.tsx", ""),
            ("src/app/not-found.tsx", ""),
            ("pages/index.tsx", ""),
            ("src/pages/api/users.ts", ""),
            ("middleware.ts", ""),
            ("src/instrumentation.ts", ""),
            ("next.config.mjs", "export default {};"),
            ("lib/x.ts", ""),
        ]);
        assert_eq!(
            found_files(&got),
            [
                "app/api/route.ts",
                "app/blog/[id]/layout.tsx",
                "app/page.tsx",
                "app/sitemap.ts",
                "middleware.ts",
                "next.config.mjs",
                "pages/index.tsx",
                "src/app/not-found.tsx",
                "src/instrumentation.ts",
                "src/pages/api/users.ts",
            ]
        );
    }

    #[test]
    fn a_peer_dependency_recognises_no_framework() {
        let got = found(&[
            ("package.json", r#"{"peerDependencies": {"next": "15"}}"#),
            ("app/page.tsx", ""),
        ]);
        assert!(got.frameworks.is_empty() && got.files.is_empty());
    }

    #[test]
    fn a_moved_convention_or_an_unread_config_leaves_the_package_unjudged() {
        let moved = found(&[
            ("package.json", SVELTEKIT),
            (
                "svelte.config.js",
                "export default { kit: { files: { routes: 'pages' } } };",
            ),
            ("pages/+page.ts", ""),
            ("src/lib/x.ts", ""),
        ]);
        assert_eq!(
            found_files(&moved),
            [
                "package.json",
                "pages/+page.ts",
                "src/lib/x.ts",
                "svelte.config.js"
            ]
        );
        let [gap] = &moved.untrusted[..] else {
            panic!("{:?}", moved.untrusted)
        };
        assert_eq!(
            gap.reason,
            "`unused` did not judge the files of package.json, which uses SvelteKit: svelte.config.js may set `kit.files`, which moves the files it loads, so it took every file of the package as used"
        );
        assert_eq!(gap.files.len(), 4);

        let next = found(&[
            ("package.json", r#"{"dependencies": {"next": "15"}}"#),
            (
                "next.config.js",
                "module.exports = { pageExtensions: ['page.tsx'] };",
            ),
        ]);
        assert!(
            next.untrusted[0]
                .reason
                .contains("next.config.js may set `pageExtensions`")
        );

        let files = ["package.json".to_string(), "next.config.ts".to_string()];
        let unread = entries(&files, &[], &|f| {
            (f == "package.json").then(|| r#"{"dependencies": {"next": "15"}}"#.to_string())
        });
        assert!(
            unread.untrusted[0]
                .reason
                .contains("its config file next.config.ts could not be read"),
            "{:?}",
            unread.untrusted
        );
    }

    #[test]
    fn a_manifest_that_cannot_be_parsed_or_read_leaves_every_file_under_it_unjudged() {
        let got = found(&[
            ("package.json", r#"{"name": "root"}"#),
            ("web/package.json", "{ \"dependencies\": "),
            ("web/src/routes/+page.ts", ""),
            ("lib/x.ts", ""),
        ]);
        assert_eq!(
            found_files(&got),
            ["web/package.json", "web/src/routes/+page.ts"]
        );
        assert_eq!(
            got.untrusted[0].reason,
            "`unused` could not parse web/package.json, so it took every file under it as used: what the package exports and the frameworks it uses are not known"
        );
        let files = ["package.json".to_string(), "x.ts".to_string()];
        let unread = entries(&files, &[], &|_| None);
        assert_eq!(found_files(&unread), ["package.json", "x.ts"]);
        assert!(
            unread.untrusted[0]
                .reason
                .contains("could not read package.json")
        );
    }

    #[test]
    fn a_key_is_found_as_a_whole_word() {
        assert!(has_word("kit: { files: {} }", "files"));
        assert!(has_word("files", "files"));
        assert!(!has_word("const profiles = 1; filesystem", "files"));
    }
}
