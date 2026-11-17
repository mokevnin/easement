//! What `--diff` judges: the lines changed since the merge-base, what the
//! change to the policy causes over the same code (the policy delta), and the
//! parts judged in full because neither can say what the change caused.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::check::{HonouredSuppression, Key, RuleId, Violation};
use crate::lang::registry;
use crate::policy::{Policy, PolicyInput};

/// Lines added or changed since the merge-base with a base revision, including
/// uncommitted and untracked files, and the points where lines were deleted. Paths
/// are relative to the project root.
#[derive(Debug, Default)]
pub struct Changes {
    pub base: String,
    pub merge_base: String,
    files: HashMap<String, Lines>,
    /// Files the change deletes.
    pub deleted: Vec<String>,
}

#[derive(Debug)]
enum Lines {
    All,
    Ranges {
        changed: Vec<(usize, usize)>,
        /// The lines after which a hunk deleted lines and added none (0 at the top).
        deleted_after: Vec<usize>,
    },
}

/// What a change did to the lines of one file.
#[derive(Debug, PartialEq, Eq)]
pub enum Touch<'a> {
    /// Every line is new: an untracked file.
    Whole,
    Lines {
        /// The ranges of lines added or changed, first and last.
        changed: &'a [(usize, usize)],
        /// The lines after which lines were deleted and none added: a deletion
        /// after line `p` falls between `p` and `p + 1`.
        deleted_after: &'a [usize],
    },
}

impl Changes {
    pub fn collect(root: &Path, base: &str) -> Result<Self> {
        let merge_base = merge_base(root, base)?;
        // Working tree against the merge-base: committed and uncommitted changes.
        // --relative limits the diff to the project root and makes paths relative to it.
        // A moved file is deleted and added: in its new place every line is new,
        // where a rename would leave only the lines edited on the way.
        let diff = git(
            root,
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--relative",
                "--unified=0",
                "--no-renames",
                "--no-color",
                "--no-ext-diff",
                "--no-prefix",
                &merge_base,
            ],
        )?;
        let mut changes = Self::parse(&diff)?;
        let untracked = git(
            root,
            &[
                "-c",
                "core.quotePath=false",
                "ls-files",
                "--others",
                "--exclude-standard",
            ],
        )?;
        for path in untracked.lines().filter(|l| !l.is_empty()) {
            changes.files.insert(path.to_string(), Lines::All);
        }
        changes.base = base.to_string();
        changes.merge_base = merge_base;
        Ok(changes)
    }

    fn parse(diff: &str) -> Result<Self> {
        let mut patch = unidiff::PatchSet::new();
        patch
            .parse(diff)
            .context("cannot read the output of `git diff`")?;
        let mut files: HashMap<String, Lines> = HashMap::new();
        let mut deleted = Vec::new();
        for file in patch.files() {
            // A target of /dev/null is a deleted file; a hunk that leaves no line
            // at 0 is only a file that lost its first lines.
            if file.target_file == "/dev/null" {
                deleted.push(file.source_file.clone());
                continue;
            }
            // Changed although no line of it may be added: a manifest loses a
            // dependency by a deleted line.
            let (added, deleted): (Vec<_>, Vec<_>) =
                file.hunks().iter().partition(|hunk| hunk.target_length > 0);
            let changed = added
                .iter()
                .map(|hunk| {
                    (
                        hunk.target_start,
                        hunk.target_start + hunk.target_length - 1,
                    )
                })
                .collect();
            // A hunk that adds nothing starts at the line before it.
            let deleted_after = deleted.iter().map(|hunk| hunk.target_start).collect();
            files.insert(
                file.target_file.clone(),
                Lines::Ranges {
                    changed,
                    deleted_after,
                },
            );
        }
        Ok(Self {
            files,
            deleted,
            ..Self::default()
        })
    }

    /// Every file the change touches.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Whether `line` of `file` is added or changed.
    pub fn contains(&self, file: &str, line: usize) -> bool {
        match self.files.get(file) {
            Some(Lines::All) => true,
            Some(Lines::Ranges { changed, .. }) => {
                changed.iter().any(|(a, b)| (*a..=*b).contains(&line))
            }
            None => false,
        }
    }

    /// What the change did to the lines of `file`; `None` when it left the file as it was.
    pub fn touch(&self, file: &str) -> Option<Touch<'_>> {
        Some(match self.files.get(file)? {
            Lines::All => Touch::Whole,
            Lines::Ranges {
                changed,
                deleted_after,
            } => Touch::Lines {
                changed,
                deleted_after,
            },
        })
    }
}

/// A diff run: the change, and what it is held responsible for.
#[derive(Debug)]
pub struct Diff {
    pub changes: Changes,
    pub attribution: Attribution,
}

impl Diff {
    /// Whether the violation belongs to the change: a rule about the config,
    /// an import on a changed line, the policy delta, or a part judged in full.
    pub fn keeps(&self, violation: &Violation) -> bool {
        let attribution = &self.attribution;
        attribution.whole()
            || violation.rule.is_config_rule()
            || violation
                .evidence
                .iter()
                .any(|at| self.changes.contains(&at.file, at.line))
            || attribution
                .base_keys
                .as_ref()
                .is_some_and(|base| !base.contains(&violation.key()))
            || (attribution.manifests_in_full() && reads_a_manifest(violation))
    }

    /// Whether the suppression belongs to the change: a violation it excused
    /// would, or the change wrote its directive.
    pub fn keeps_suppression(&self, suppression: &HonouredSuppression) -> bool {
        self.changes.contains(&suppression.file, suppression.line)
            || suppression.excused.iter().any(|v| self.keeps(v))
    }
}

/// What a diff run judges besides the changed lines, and why.
#[derive(Debug, Default, Serialize)]
pub struct Attribution {
    /// The policy inputs that differ from the merge-base, relative to the root.
    pub policy_changed: Vec<String>,
    /// What was judged in full instead of by the change, with why.
    pub in_full: Vec<InFull>,
    /// The violations the policy at the merge-base fails over the same code,
    /// when the policy changed and that could be judged.
    #[serde(skip)]
    base_keys: Option<BTreeSet<Key>>,
}

/// A part of the project judged in full in a diff run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InFull {
    /// The policy keys whose violations were judged in full; every rule when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    pub reason: String,
}

const MANIFEST_KEYS: [&str; 2] = ["manifests", "packages"];

impl Attribution {
    /// What `head`, the policy of the working tree, is held responsible for
    /// against the policy at the merge-base of `changes`. `judge` gives the
    /// violations a policy produces over the code this run scanned, those an
    /// inline suppression excuses among them: a violation the base policy
    /// produced is no effect of the change to the policy, excused or not.
    pub fn of(
        head: &Policy,
        root: &Path,
        changes: &Changes,
        judge: impl FnOnce(&Policy) -> Result<Vec<Violation>>,
    ) -> Self {
        let mut attribution = Self::default();
        let reads_manifests = head.config.manifests || head.config.packages;
        let manifest = changes
            .files()
            .chain(changes.deleted.iter().map(String::as_str))
            .filter(|f| registry::manifest_of(f).is_some())
            .min();
        if reads_manifests && let Some(manifest) = manifest {
            attribution.in_full.push(InFull {
                keys: MANIFEST_KEYS.map(str::to_string).to_vec(),
                reason: format!(
                    "{manifest} changed, so the `manifests` and `packages` rules were judged in full: they read the manifest, not a line of it"
                ),
            });
        }
        let whole = |reason: String| InFull {
            keys: Vec::new(),
            reason,
        };
        let base = match Policy::load(&head.config_input().path, Some(&changes.merge_base)) {
            Ok(base) => base,
            Err(err) => {
                attribution.in_full.push(whole(format!(
                    "the policy at the merge-base cannot be read, so what the change to it causes is unknown: {err:#}"
                )));
                return attribution;
            }
        };
        attribution.policy_changed = changed_inputs(&head.inputs, &base.inputs, root);
        // Its past is unknown, so it counts as changed.
        if let Some(outside) = base.inputs.iter().find(|i| i.past_unknown) {
            attribution
                .policy_changed
                .push(outside.path.display().to_string());
            attribution.in_full.push(whole(format!(
                "{} is outside the repository, so its content at the merge-base is unknown",
                outside.path.display()
            )));
            return attribution;
        }
        if attribution.policy_changed.is_empty() {
            return attribution;
        }
        let scanned = base
            .config
            .scan_policy()
            .differences(&head.config.scan_policy());
        if !scanned.is_empty() {
            attribution.in_full.push(whole(format!(
                "the change to the policy changes what is scanned ({}), so the code scanned now cannot stand in for the code the merge-base policy would have scanned",
                scanned
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
            return attribution;
        }
        match judge(&base) {
            // A warning fails nothing, so a rule taken out of `warn` causes what it fails.
            Ok(violations) => {
                attribution.base_keys = Some(
                    violations
                        .iter()
                        .filter(|v| !base.config.warns(v.rule))
                        .map(Violation::key)
                        .collect(),
                );
            }
            Err(err) => attribution.in_full.push(whole(format!(
                "the policy at the merge-base cannot be judged, so what the change to it causes is unknown: {err:#}"
            ))),
        }
        attribution
    }

    /// Whether the whole project was judged, not only the change.
    pub fn whole(&self) -> bool {
        self.in_full.iter().any(|part| part.keys.is_empty())
    }

    fn manifests_in_full(&self) -> bool {
        self.in_full.iter().any(|part| {
            part.keys
                .iter()
                .any(|k| MANIFEST_KEYS.contains(&k.as_str()))
        })
    }
}

/// Whether a manifest stands behind the violation: an import a manifest does
/// not declare, or a dependency a manifest declares.
fn reads_a_manifest(violation: &Violation) -> bool {
    violation.rule == RuleId::Undeclared
        || violation
            .evidence
            .iter()
            .any(|at| registry::manifest_of(&at.file).is_some())
}

/// The inputs of one policy whose content differs in the other or that the
/// other lacks, relative to `root` where they are under it.
pub fn changed_inputs(head: &[PolicyInput], base: &[PolicyInput], root: &Path) -> Vec<String> {
    let differs = |input: &PolicyInput, other: &[PolicyInput]| {
        !other
            .iter()
            .any(|o| o.path == input.path && o.content == input.content)
    };
    let mut changed: Vec<String> = head
        .iter()
        .filter(|i| differs(i, base))
        .chain(base.iter().filter(|i| differs(i, head)))
        .map(|i| input_path(i, root))
        .collect();
    changed.sort();
    changed.dedup();
    changed
}

/// The commit where HEAD left the base revision `base`.
pub fn merge_base(root: &Path, base: &str) -> Result<String> {
    let merge_base = git(root, &["merge-base", base, "HEAD"]).with_context(|| {
        format!(
            "cannot find the merge-base of `{base}` and HEAD; \
             in CI fetch the base branch too (actions/checkout: fetch-depth: 0)"
        )
    })?;
    Ok(merge_base.trim().to_string())
}

/// Where a policy input is: relative to `root` when under it, else absolute.
pub fn input_path(input: &PolicyInput, root: &Path) -> String {
    // Input paths are real, so the root is too.
    let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    input
        .path
        .strip_prefix(&root)
        .map_or_else(|_| input.path.display().to_string(), crate::relpath::slash)
}

/// The content of `path` at the Git revision `rev`, with `path` as `git show`
/// reads it in `dir` (`./x` relative to `dir`, else to the top of the
/// repository); none when the revision has no such file.
pub fn file_at(dir: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    let spec = format!("{rev}:{path}");
    if git(dir, &["cat-file", "-e", &spec]).is_err() {
        return Ok(None);
    }
    git(dir, &["show", &spec]).map(Some)
}

/// The output of `git` run in `dir` with `args`; an error with what Git said
/// when it fails. Git reads the repository of `dir`, whatever repository the
/// environment names: see [`repository_env`].
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    for name in repository_env() {
        command.env_remove(name);
    }
    let output = command
        .current_dir(dir)
        .args(args)
        .output()
        .context("cannot run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The variables that tie git to one repository (`GIT_DIR`, `GIT_WORK_TREE`,
/// `GIT_INDEX_FILE`, …), as git names them. A git hook and `git rebase --exec`
/// set them for the repository that runs easement, which need not be the one it
/// checks, so nothing easement runs inherits them: no git, no indexer.
pub(crate) fn repository_env() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_deleted_files() {
        let diff = "\
--- src/gone.ts
+++ /dev/null
@@ -1,2 +0,0 @@
-a
-b
--- src/kept.ts
+++ src/kept.ts
@@ -1 +1,2 @@
-x
+x
+y
";
        let changes = Changes::parse(diff).unwrap();
        assert_eq!(changes.deleted, ["src/gone.ts"]);
        assert!(changes.contains("src/kept.ts", 1));
    }

    #[test]
    fn a_file_with_only_deleted_lines_is_changed() {
        let diff = "\
--- package.json
+++ package.json
@@ -4 +3,0 @@
-    \"left-pad\": \"1.0.0\",
";
        let changes = Changes::parse(diff).unwrap();
        assert_eq!(changes.files().collect::<Vec<_>>(), ["package.json"]);
        assert!(!changes.contains("package.json", 3));
    }

    /// A file whose first lines alone are deleted is still there: one hunk
    /// that leaves no line at 0 is no deleted file, a target of /dev/null is.
    #[test]
    fn a_file_that_loses_its_first_lines_is_changed_not_deleted() {
        let diff = "\
--- src/top.ts
+++ src/top.ts
@@ -1 +0,0 @@
-import { gone } from \"./gone\";
";
        let changes = Changes::parse(diff).unwrap();
        assert!(changes.deleted.is_empty(), "{:?}", changes.deleted);
        assert_eq!(changes.files().collect::<Vec<_>>(), ["src/top.ts"]);
    }

    /// A hunk that only deletes lines changes none, but says where they were. A hunk
    /// that adds lines too is a change, however many more it deletes: the head
    /// alone cannot tell lines lost at a definition's edge from a definition lost.
    #[test]
    fn keeps_where_lines_were_deleted() {
        let diff = "\
--- src/a.ts
+++ src/a.ts
@@ -1 +0,0 @@
-top
@@ -3 +2 @@
-old
+new
@@ -5,3 +4 @@
-was
-was
-was
+is
@@ -10,2 +6,0 @@
-gone
-gone
";
        let changes = Changes::parse(diff).unwrap();
        assert_eq!(
            changes.touch("src/a.ts"),
            Some(Touch::Lines {
                changed: &[(2, 2), (4, 4)],
                deleted_after: &[0, 6],
            })
        );
        assert!(!changes.contains("src/a.ts", 6));
        assert_eq!(changes.touch("src/b.ts"), None);
    }

    #[test]
    fn a_path_with_a_space_is_the_path_git_wrote() {
        // git ends `---` and `+++` with a tab when the path holds a space.
        let diff = "diff --git dir a/x.ts dir a/x.ts\n--- dir a/x.ts\t\n+++ dir a/x.ts\t\n@@ -1 +1 @@\n-q\n+r\n";
        let changes = Changes::parse(diff).unwrap();
        assert!(changes.contains("dir a/x.ts", 1), "{:?}", changes.files);
    }

    #[test]
    fn parses_hunks() {
        let diff = "\
diff --git src/a.ts src/a.ts
--- src/a.ts
+++ src/a.ts
@@ -3 +3 @@ x
-old
+new
@@ -10,0 +11,3 @@
+a
+b
+c
@@ -20,2 +23,0 @@
-gone
diff --git src/old.ts src/old.ts
--- src/old.ts
+++ /dev/null
@@ -1 +0,0 @@
-x
";
        let changes = Changes::parse(diff).unwrap();
        assert!(changes.contains("src/a.ts", 3));
        assert!(!changes.contains("src/a.ts", 4));
        assert!(changes.contains("src/a.ts", 11));
        assert!(changes.contains("src/a.ts", 13));
        assert!(!changes.contains("src/a.ts", 14));
        assert!(!changes.contains("src/a.ts", 23));
        assert!(!changes.contains("src/old.ts", 1));
    }

    /// The lines of `text`, a `\n` after each but the last when `newline` is false.
    fn file(lines: &[String], newline: bool) -> String {
        let mut text = lines.join("\n");
        if newline && !lines.is_empty() {
            text.push('\n');
        }
        text
    }

    proptest::proptest! {
        // Each case runs git.
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Whatever git writes for an edit, the lines `contains` names in the new
        /// file are the ones git added, and the others are the old file's
        /// lines that git kept, in their order.
        #[test]
        fn a_changed_line_is_one_git_added(
            old in proptest::collection::vec("[abc]?", 0..12),
            new in proptest::collection::vec("[abc]?", 0..12),
            old_newline: bool,
            new_newline: bool,
        ) {
            let dir = tempfile::tempdir().unwrap();
            let (old, new) = (file(&old, old_newline), file(&new, new_newline));
            for (side, text) in [("old", &old), ("new", &new)] {
                std::fs::create_dir(dir.path().join(side)).unwrap();
                std::fs::write(dir.path().join(side).join("f"), text).unwrap();
            }
            // The lines as git counts them: a last line with no `\n` is one too.
            let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
            let output = std::process::Command::new("git")
                .args(["-c", "core.quotePath=false", "diff", "--no-index", "--unified=0",
                    "--no-renames", "--no-color", "--no-ext-diff", "--no-prefix", "old/f", "new/f"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            let diff = String::from_utf8(output.stdout).unwrap();
            let changes = Changes::parse(&diff).unwrap();
            proptest::prop_assert!(changes.deleted.is_empty(), "{:?}", changes.deleted);

            let mut added = vec![false; new.len()];
            let mut line = 0;
            for text in diff.lines() {
                if let Some(header) = text.strip_prefix("@@ ") {
                    // `+start,count`: the line before the hunk when it adds none.
                    let target = header.split(' ').nth(1).unwrap().trim_start_matches('+');
                    line = target.split(',').next().unwrap().parse::<usize>().unwrap();
                } else if text.starts_with('+') && !text.starts_with("+++") {
                    added[line - 1] = true;
                    line += 1;
                }
            }
            for (i, was_added) in added.iter().enumerate() {
                proptest::prop_assert_eq!(changes.contains("new/f", i + 1), *was_added, "line {}\n{}", i + 1, diff);
            }
            proptest::prop_assert!(!changes.contains("new/f", new.len() + 1), "{}", diff);

            let mut rest = old.iter();
            for (text, _) in new.iter().zip(&added).filter(|(_, added)| !**added) {
                proptest::prop_assert!(rest.any(|o| o == text), "kept line {:?} is not in the old file\n{}", text, diff);
            }
        }
    }
}
