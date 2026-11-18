//! The `undeclared` rule: an external module a file imports has to be a
//! dependency of the nearest manifest above it (`package.json`, `go.mod`,
//! `Cargo.toml`), or the build works only by accident.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::check::{Place, RuleId, Unjudged, Violation};
use crate::components::ComponentMap;
use crate::config::Config;
use crate::lang::Dep;
use crate::lang::Target;
use crate::lang::registry::{self, Manifest};
use crate::packages::{Declares, Named};
use crate::relpath;

pub struct Undeclared {
    pub violations: Vec<Violation>,
    pub reasons: Vec<Unjudged>,
}

/// With `manifests`, every import of an external module that the nearest
/// manifest above the importing file does not declare.
pub fn undeclared(
    config: &Config,
    map: &ComponentMap,
    root: &Path,
    deps: &[Dep],
    all_files: &[String],
) -> Undeclared {
    if !config.manifests {
        return Undeclared {
            violations: Vec::new(),
            reasons: Vec::new(),
        };
    }
    let known: BTreeSet<&str> = all_files.iter().map(String::as_str).collect();
    // Manifest -> what it declares; `None` when it declares no package.
    let mut manifests: BTreeMap<String, Option<BTreeSet<String>>> = BTreeMap::new();
    let mut unreadable: BTreeSet<String> = BTreeSet::new();
    // A source file -> the packages it names, `None` when it cannot be read.
    let mut sources: BTreeMap<String, Option<Named>> = BTreeMap::new();
    let mut unread_sources: BTreeSet<String> = BTreeSet::new();
    let mut violations = Vec::new();
    let mut unjudged = BTreeSet::new();
    let mut orphans = BTreeSet::new();
    // The files no manifest is above.
    let mut unowned = BTreeSet::new();
    for dep in deps {
        let Target::External(module) = &dep.target else {
            continue;
        };
        let file = crate::relpath::slash(&dep.file);
        // What a manifest declares is `packages`' to judge, not an import to look up.
        if registry::manifest_of(&file).is_some() {
            continue;
        }
        let Some(kind) = registry::of(&file).and_then(|language| language.manifest) else {
            unjudged.insert(relpath::extension(&file).to_string());
            continue;
        };
        if crate::lang::registry::is_std(&file, module) {
            continue;
        }
        let Some(manifest) =
            package_manifest(&file, kind, root, &known, &mut manifests, &mut unreadable)
        else {
            orphans.insert(kind.file_name);
            unowned.insert(file.clone());
            continue;
        };
        let Some(Some(names)) = manifests.get(&manifest) else {
            continue; // unreadable: reported once below
        };
        let package = kind.package(module);
        if kind.covers(names, &package) {
            continue;
        }
        // Read as text whatever its encoding: an import is ASCII.
        let named = sources.entry(file.clone()).or_insert_with(|| {
            std::fs::read(root.join(&file))
                .ok()
                .map(|bytes| kind.named_in_source(&file, &String::from_utf8_lossy(&bytes)))
        });
        let Some(named) = named else {
            unread_sources.insert(file);
            continue;
        };
        if !named.names(&package) {
            continue;
        }
        violations.push(Violation {
            evidence: vec![Place::new(file.clone(), dep.line)],
            line: dep.line,
            message: format!(
                "{package} is not declared in {manifest}; add it to the dependencies, or drop the dependency"
            ),
            from: map.component_of(&file).unwrap_or_default().to_string(),
            target: package,
            specifier: dep.specifier.clone(),
            file,
            ..Violation::of(RuleId::Undeclared)
        });
    }
    let mut reasons = Vec::new();
    if !unjudged.is_empty() {
        let exts: Vec<String> = unjudged.into_iter().collect();
        reasons.push(
            Unjudged::new(format!(
                "`manifests` cannot tell the package of a module in .{} files, so their dependencies were not checked",
                exts.join(", .")
            ))
            .in_extensions(exts),
        );
    }
    if !unreadable.is_empty() {
        let list: Vec<String> = unreadable.into_iter().collect();
        reasons.push(
            Unjudged::new(format!(
                "{} could not be read, so the dependencies of the files under it were not checked",
                list.join(", ")
            ))
            .in_files(list),
        );
    }
    if !unread_sources.is_empty() {
        let list: Vec<String> = unread_sources.into_iter().collect();
        reasons.push(
            Unjudged::new(format!(
                "{} could not be read, so whether it imports the packages its manifest lacks was not checked",
                list.join(", ")
            ))
            .in_files(list),
        );
    }
    if !orphans.is_empty() {
        let names: Vec<&str> = orphans.into_iter().collect();
        reasons.push(
            Unjudged::new(format!(
                "some files have no {} above them, so their dependencies were not checked",
                names.join(" or ")
            ))
            .in_files(unowned),
        );
    }
    Undeclared {
        violations,
        reasons,
    }
}

/// The manifest that governs `file`: the nearest one above it that declares
/// a package, passing over a `package.json` that only marks its directory.
/// An unreadable one stops the search and is recorded in `unreadable`.
fn package_manifest(
    file: &str,
    kind: &Manifest,
    root: &Path,
    known: &BTreeSet<&str>,
    manifests: &mut BTreeMap<String, Option<BTreeSet<String>>>,
    unreadable: &mut BTreeSet<String>,
) -> Option<String> {
    let mut below = file.to_string();
    loop {
        let manifest = nearest(&below, kind.file_name, known)?;
        let declares = manifests
            .entry(manifest.clone())
            .or_insert_with(|| {
                // One that cannot be read is not an empty one, which declares nothing.
                let declares = std::fs::read_to_string(root.join(&manifest))
                    .map_or(Declares::Unreadable, |text| kind.declares(&text));
                match declares {
                    Declares::Package(names) => Some(names),
                    Declares::Marker => None,
                    Declares::Unreadable => {
                        unreadable.insert(manifest.clone());
                        None
                    }
                }
            })
            .is_some();
        if declares || unreadable.contains(&manifest) {
            return Some(manifest);
        }
        // A marker: look above its directory.
        let dir = manifest.rsplit_once('/').map(|(d, _)| d)?;
        below = dir.to_string();
    }
}

/// The nearest file named `name` in the directory of `file` or above it.
fn nearest(file: &str, name: &str, known: &BTreeSet<&str>) -> Option<String> {
    let mut dir = relpath::parent(file);
    loop {
        let candidate = relpath::join(dir, name);
        if known.contains(candidate.as_str()) {
            return Some(candidate);
        }
        if dir.is_empty() {
            return None;
        }
        dir = relpath::parent(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_nearest_manifest() {
        let known: BTreeSet<&str> = ["package.json", "web/package.json"].into();
        assert_eq!(
            nearest("web/src/a.ts", "package.json", &known).as_deref(),
            Some("web/package.json")
        );
        assert_eq!(
            nearest("api/a.ts", "package.json", &known).as_deref(),
            Some("package.json")
        );
        assert_eq!(nearest("a.go", "go.mod", &known), None);
    }

    #[test]
    fn a_marker_package_json_is_passed_over() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src/esm")).unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"name": "app"}"#).unwrap();
        std::fs::write(
            dir.path().join("src/esm/package.json"),
            r#"{"type": "module"}"#,
        )
        .unwrap();
        let known: BTreeSet<&str> = ["package.json", "src/esm/package.json"].into();
        let (mut manifests, mut unreadable) = (BTreeMap::new(), BTreeSet::new());
        let found = package_manifest(
            "src/esm/a.ts",
            &registry::PACKAGE_JSON,
            dir.path(),
            &known,
            &mut manifests,
            &mut unreadable,
        );
        assert_eq!(found.as_deref(), Some("package.json"));
        assert!(unreadable.is_empty());
    }

    /// A manifest that cannot be read declares nothing that is known: it is
    /// unreadable, not empty, or every import under it would be undeclared.
    #[test]
    #[cfg(unix)]
    fn a_manifest_that_cannot_be_read_is_unreadable_not_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink("missing.toml", dir.path().join("Cargo.toml")).unwrap();
        let known: BTreeSet<&str> = ["Cargo.toml"].into();
        let (mut manifests, mut unreadable) = (BTreeMap::new(), BTreeSet::new());
        let found = package_manifest(
            "src/lib.rs",
            &registry::CARGO_TOML,
            dir.path(),
            &known,
            &mut manifests,
            &mut unreadable,
        );
        assert_eq!(found.as_deref(), Some("Cargo.toml"));
        assert_eq!(unreadable, BTreeSet::from(["Cargo.toml".to_string()]));
    }
}
