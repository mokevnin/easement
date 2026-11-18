//! `Cargo.toml`: the dependency tables, the features, and the workspace a crate takes
//! dependencies from.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::manifest::{Declared, Declares, Listed, Reach, Source, line_at, path_source};
use crate::check::Unjudged;
use crate::lang::parts::{Kind, Part};
use crate::packages::manifest::ManifestFile;
use crate::relpath::{self, parent};

/// The name of its file.
pub const FILE_NAME: &str = "Cargo.toml";

/// The table of dependencies only the crate's tests, examples and benchmarks build with.
const DEV: &str = "dev-dependencies";

/// The table of dependencies only its build script builds with.
const BUILD: &str = "build-dependencies";

/// The parts a dependency of a `Cargo.toml` can belong to, as `part:` names them:
/// a feature (see [`feature_part_name`]) and the tables but the plain one.
pub(crate) const PART_NAMES: &[&str] = &["feature/<name>", DEV, BUILD];

/// The name of the part the feature `feature` is.
fn feature_part_name(feature: &str) -> String {
    format!("feature/{feature}")
}

/// The part a feature is, `default` being the package itself.
fn feature_part(feature: &str) -> Option<Part> {
    (feature != "default").then(|| Part {
        name: feature_part_name(feature),
        kind: None,
    })
}

/// Where a dependency of a `Cargo.toml` comes from, as the manifest writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    /// `path = "..."`, relative to the manifest.
    Path(String),
    /// `workspace = true`: what the workspace manifest declares under the name.
    Workspace,
    Registry,
    /// The manifest itself: an item of a feature that names another feature of it.
    Own,
}

/// One dependency as a `Cargo.toml` writes it, before its workspace is read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    line: usize,
    name: String,
    package: String,
    origin: Origin,
    part: Option<Part>,
    /// The feature of the package it turns on, when not just the package.
    feature: Option<String>,
    /// `optional = true`: a feature turns it on.
    optional: bool,
}

/// The dependencies the `Cargo.toml` `file` declares, a `workspace = true` one as
/// the closest workspace manifest above it declares it. A workspace manifest that
/// cannot be read leaves such a dependency an external crate, and says so.
pub(crate) fn declared(file: &ManifestFile) -> Option<Listed> {
    let entries = entries(file.text)?;
    let mut workspaces = Workspaces::new(file.read);
    let mut listed = Listed::default();
    for entry in entries {
        let mut package = entry.package.clone();
        let source = match &entry.origin {
            Origin::Path(path) => path_source(file.path, path),
            Origin::Registry => Source::Registry,
            Origin::Own => Source::Path(parent(file.path).to_string()),
            Origin::Workspace => match workspaces.dependency(parent(file.path), &entry.name) {
                Ok(Some(found)) => {
                    if let Some(real) = found.package {
                        package = real;
                    }
                    found
                        .path
                        .and_then(|path| relpath::resolve(&found.dir, &path))
                        .map_or(Source::Registry, Source::Path)
                }
                Ok(None) => Source::Registry,
                Err(workspace) => {
                    listed.problems.push(
                        Unjudged::new(format!(
                            "{}: `{}` is `workspace = true`, but {workspace} cannot be read, so it is taken for an external crate",
                            file.path, entry.name
                        ))
                        .in_files([file.path.to_string()]),
                    );
                    Source::Registry
                }
            },
        };
        listed.declared.push(Declared {
            line: entry.line,
            name: entry.name,
            package,
            source,
            part: entry.part,
            reaches: entry.feature.map(|feature| Reach {
                part: feature_part(&feature),
            }),
        });
    }
    Some(listed)
}

/// What a workspace manifest declares under a name.
#[derive(Clone)]
struct WorkspaceEntry {
    /// The directory of the workspace manifest.
    dir: String,
    path: Option<String>,
    package: Option<String>,
}

/// What a `Cargo.toml` says to a crate below it that takes a dependency from the
/// workspace.
enum Said {
    /// It cannot be read, or it holds no `[workspace]`: the walk goes on above it.
    Nothing,
    /// Its `[workspace]` text cannot be read as TOML.
    Unreadable,
    /// The dependencies its `[workspace]` declares, by name.
    Workspace(HashMap<String, WorkspaceEntry>),
}

/// The workspace manifests above one crate, each read and parsed once however many
/// of its dependencies take from it.
struct Workspaces<'r> {
    read: &'r dyn Fn(&str) -> Option<String>,
    by_manifest: HashMap<String, Said>,
}

impl<'r> Workspaces<'r> {
    /// `read` gives the text of a file of the project.
    fn new(read: &'r dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            read,
            by_manifest: HashMap::new(),
        }
    }

    /// What the closest workspace manifest above `dir` declares under `name`. `Err`
    /// names a manifest that holds `[workspace]` text it cannot read as TOML.
    fn dependency(&mut self, dir: &str, name: &str) -> Result<Option<WorkspaceEntry>, String> {
        let mut here = Some(dir);
        while let Some(current) = here {
            let manifest = relpath::join(current, FILE_NAME);
            let read = self.read;
            let said = self
                .by_manifest
                .entry(manifest)
                .or_insert_with_key(|manifest| {
                    read(manifest).map_or(Said::Nothing, |text| said_by(&text, current))
                });
            match said {
                Said::Nothing => {}
                Said::Unreadable => return Err(relpath::join(current, FILE_NAME)),
                Said::Workspace(dependencies) => return Ok(dependencies.get(name).cloned()),
            }
            here = (!current.is_empty()).then(|| parent(current));
        }
        Ok(None)
    }
}

/// What the `Cargo.toml` in `dir` holding `text` says to a crate below it.
fn said_by(text: &str, dir: &str) -> Said {
    match toml::from_str::<toml::Table>(text) {
        Ok(table) => match table.get("workspace") {
            Some(workspace) => Said::Workspace(
                workspace
                    .get("dependencies")
                    .and_then(toml::Value::as_table)
                    .into_iter()
                    .flatten()
                    .map(|(name, spec)| {
                        let text_of = |key: &str| {
                            spec.get(key)
                                .and_then(toml::Value::as_str)
                                .map(str::to_string)
                        };
                        let entry = WorkspaceEntry {
                            dir: dir.to_string(),
                            path: text_of("path"),
                            package: text_of("package"),
                        };
                        (name.clone(), entry)
                    })
                    .collect(),
            ),
            None => Said::Nothing,
        },
        Err(_) if text.contains("[workspace") => Said::Unreadable,
        Err(_) => Said::Nothing,
    }
}

const CARGO_TABLES: [&str; 3] = ["dependencies", DEV, BUILD];

/// The dependencies a `Cargo.toml` writes, in its plain tables and in those of each
/// `[target.<cfg>]`, with the features; `None` for a manifest that is no TOML.
fn entries(text: &str) -> Option<Vec<Entry>> {
    let document = DeTable::parse(text).ok()?;
    let manifest = document.get_ref();
    let line = |span: Range<usize>| line_at(text, span.start);
    // The tables of dependencies: the plain ones and those of each `[target.<cfg>]`.
    let mut tables: Vec<(&str, &DeTable)> = CARGO_TABLES
        .iter()
        .filter_map(|kind| Some((*kind, table(manifest, kind)?)))
        .collect();
    for (_, target) in table(manifest, "target").into_iter().flatten() {
        if let Some(target) = target.get_ref().as_table() {
            tables.extend(
                CARGO_TABLES
                    .iter()
                    .filter_map(|kind| Some((*kind, table(target, kind)?))),
            );
        }
    }
    let mut found: Vec<Entry> = tables
        .iter()
        .flat_map(|(kind, dependencies)| {
            dependencies.iter().flat_map(|(name, spec)| {
                cargo_declared(kind, name.get_ref(), spec.get_ref(), line(name.span()))
            })
        })
        .collect();
    found.sort_by_key(|d| d.line);
    // `[features]`: the items of each feature, with their lines.
    let mut items: Vec<(usize, String, String)> = table(manifest, "features")
        .into_iter()
        .flatten()
        .flat_map(|(feature, list)| {
            list.get_ref()
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |item| {
                    let text = item.get_ref().as_str()?;
                    Some((
                        line(item.span()),
                        feature.get_ref().to_string(),
                        text.to_string(),
                    ))
                })
        })
        .collect();
    items.sort_by_key(|(line, ..)| *line);
    Some(with_features(found, &items))
}

/// The table under `key` of `parent`.
fn table<'a, 'i>(parent: &'a DeTable<'i>, key: &str) -> Option<&'a DeTable<'i>> {
    parent
        .iter()
        .find(|(name, _): &(&Spanned<_>, _)| name.get_ref() == key)
        .and_then(|(_, value)| value.get_ref().as_table())
}

/// What a feature item turns on.
enum Item<'a> {
    /// `dep:x`: the dependency.
    Dependency(&'a str),
    /// `x/f`, or `x?/f` when weak: the dependency, and its feature.
    Reaches {
        name: &'a str,
        feature: &'a str,
        weak: bool,
    },
    /// `other`: a feature of the manifest.
    Feature(&'a str),
}

fn item(text: &str) -> Item<'_> {
    if let Some(name) = text.strip_prefix("dep:") {
        return Item::Dependency(name);
    }
    match text.split_once('/') {
        Some((name, feature)) => match name.strip_suffix('?') {
            Some(name) => Item::Reaches {
                name,
                feature,
                weak: true,
            },
            None => Item::Reaches {
                name,
                feature,
                weak: false,
            },
        },
        None => Item::Feature(text),
    }
}

/// The dependencies of the tables, put where the features say they belong: an optional
/// one is a dependency of what turns it on, and of its implicit feature when nothing
/// names it with `dep:`; and the items of the features, which are edges of their own.
fn with_features(declared: Vec<Entry>, items: &[(usize, String, String)]) -> Vec<Entry> {
    let named = |name: &str| {
        items
            .iter()
            .any(|(_, _, text)| matches!(item(text), Item::Dependency(n) if n == name))
    };
    // The features whose items reach a dependency.
    let owners = |name: &str| {
        let mut owners: Vec<&str> = Vec::new();
        for (_, feature, text) in items {
            let hit = match item(text) {
                Item::Dependency(n) => n == name,
                Item::Reaches { name: n, weak, .. } => n == name && !weak,
                Item::Feature(_) => false,
            };
            if hit && !owners.contains(&feature.as_str()) {
                owners.push(feature);
            }
        }
        owners
    };
    let mut out: Vec<Entry> = Vec::new();
    for d in &declared {
        if !d.optional {
            out.push(d.clone());
        } else if !named(&d.name) {
            // The implicit feature `name` turns it on.
            out.push(Entry {
                part: feature_part(&d.name),
                ..d.clone()
            });
        } else if d.feature.is_some() {
            for owner in owners(&d.name) {
                out.push(Entry {
                    part: feature_part(owner),
                    ..d.clone()
                });
            }
        }
    }
    // What an item names is a dependency of the crate, never a dev-dependency.
    let table_dependency = |name: &str| {
        declared.iter().find(|d| {
            d.name == name
                && d.feature.is_none()
                && !d
                    .part
                    .as_ref()
                    .is_some_and(|part| part.kind == Some(Kind::Test))
        })
    };
    for (line, feature, text) in items {
        let from = feature_part(feature);
        let base = |name: &str| {
            table_dependency(name).map(|d| Entry {
                line: *line,
                part: None,
                feature: None,
                optional: false,
                ..d.clone()
            })
        };
        let mut edge = |name: &str, feature: Option<&str>| {
            // A name no table declares is a mistake Cargo reports; it is no edge.
            if let Some(base) = base(name) {
                out.push(Entry {
                    part: from.clone(),
                    feature: feature.map(str::to_string),
                    ..base
                });
            }
        };
        match item(text) {
            Item::Dependency(name) => edge(name, None),
            Item::Reaches {
                name,
                feature,
                weak,
            } => {
                if !weak {
                    edge(name, None);
                }
                edge(name, Some(feature));
            }
            Item::Feature(name) => out.push(Entry {
                line: *line,
                name: name.to_string(),
                package: name.to_string(),
                origin: Origin::Own,
                part: from.clone(),
                feature: Some(name.to_string()),
                optional: false,
            }),
        }
    }
    out.sort_by_key(|d| d.line);
    out
}

/// The dependency `name` of the table `kind`, declared by `spec` on `line`.
fn cargo_declared(kind: &str, name: &str, spec: &DeValue, line: usize) -> Vec<Entry> {
    let text_of = |key: &str| spec.get(key).and_then(|value| value.get_ref().as_str());
    let flag = |key: &str| spec.get(key).and_then(|value| value.get_ref().as_bool()) == Some(true);
    let package = text_of("package").unwrap_or(name).to_string();
    let optional = flag("optional");
    let origin = if flag("workspace") {
        Origin::Workspace
    } else if let Some(path) = text_of("path") {
        Origin::Path(path.to_string())
    } else {
        Origin::Registry
    };
    let own = Entry {
        line,
        name: name.to_string(),
        package,
        origin,
        part: part_of(kind),
        feature: None,
        optional,
    };
    // `features = ["f"]` also reaches the feature `f` of the package.
    let features: Vec<Entry> = spec
        .get("features")
        .and_then(|features| features.get_ref().as_array())
        .into_iter()
        .flatten()
        .filter_map(|feature| feature.get_ref().as_str())
        .map(|f| Entry {
            feature: Some(f.to_string()),
            ..own.clone()
        })
        .collect();
    std::iter::once(own).chain(features).collect()
}

/// The part of a manifest a table of dependencies is: none for the plain one.
fn part_of(kind: &str) -> Option<Part> {
    match kind {
        DEV => Some(Part {
            name: kind.to_string(),
            kind: Some(Kind::Test),
        }),
        BUILD => Some(Part {
            name: kind.to_string(),
            kind: None,
        }),
        _ => None,
    }
}

/// The crate a Rust path belongs to (already written with `_`).
pub(crate) fn package(module: &str) -> String {
    module.split("::").next().unwrap_or(module).to_string()
}

/// What the files of a crate may import: its own name and the dependencies its
/// `Cargo.toml` declares, by name and by the real crate a renamed one is, all written
/// with `_`, read as `packages` reads them. `[workspace.dependencies]` only sets what
/// a member takes with `workspace = true`, so it adds nothing here.
pub(crate) fn importable(text: &str) -> Declares {
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return Declares::Unreadable;
    };
    let own = table
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string);
    let names = own
        .into_iter()
        .chain(
            entries(text)
                .unwrap_or_default()
                .into_iter()
                .filter(|dependency| dependency.origin != Origin::Own)
                .flat_map(|dependency| [dependency.name, dependency.package]),
        )
        .map(|name| name.replace('-', "_"))
        .collect();
    Declares::Package(names)
}

/// How Rust spells the crate `package` in a path: with `_` for each `-`.
pub(crate) fn spelled_in_import(package: &str) -> String {
    package.replace('-', "_")
}

/// The crates Rust source, of any file, names as the root of a path or in an
/// `extern crate`.
pub(crate) fn named_in_source(_file: &str, source: &str) -> BTreeSet<String> {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|[^\w:])(\w+)::|\bextern\s+crate\s+(\w+)\b").expect("valid regex")
    });
    NAMED
        .captures_iter(source)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|name| name.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Target;
    use crate::lang::registry::CARGO_TOML;
    use crate::packages::declared::declared_by;
    use crate::packages::declared::tests::importable;

    /// What the `Cargo.toml` at `path` holding `text` lists, in a project whose other
    /// files are `project`.
    fn listed_in(path: &str, text: &str, project: &[(&str, &str)]) -> Listed {
        let read = |file: &str| {
            project
                .iter()
                .find(|(p, _)| *p == file)
                .map(|(_, t)| t.to_string())
        };
        let file = ManifestFile {
            path,
            text,
            read: &read,
        };
        CARGO_TOML.declared(&file).expect("valid TOML")
    }

    /// (line, name, package, source, the part it is in) of each dependency of a crate
    /// at `crates/chunk-store`, alone in its project.
    fn found(text: &str) -> Vec<(usize, String, String, Source, Option<String>)> {
        listed_in("crates/chunk-store/Cargo.toml", text, &[])
            .declared
            .into_iter()
            .map(|d| {
                (
                    d.line,
                    d.name,
                    d.package,
                    d.source,
                    d.part.map(|part| part.name),
                )
            })
            .collect()
    }

    #[test]
    fn a_cargo_manifest_declares_a_dependency_on_the_line_of_its_name() {
        let text = concat!(
            "[package]\n",                                            // 1
            "name = \"chunk-store\"\n",                               // 2
            "\n",                                                     // 3
            "[dependencies]\n",                                       // 4
            "storage-api = { workspace = true }\n",                   // 5
            "serde_json = \"1\"\n",                                   // 6
            "local = { path = \"../local\" }\n",                      // 7
            "json = { package = \"serde_json\", version = \"1\" }\n", // 8
            "\n",                                                     // 9
            "[dev-dependencies]\n",                                   // 10
            "testkit = { workspace = true }\n",                       // 11
            "\n",                                                     // 12
            "[build-dependencies]\n",                                 // 13
            "cc = \"1\"\n",                                           // 14
            "\n",                                                     // 15
            "[target.'cfg(unix)'.dependencies]\n",                    // 16
            "libc = \"0.2\"\n",                                       // 17
            "\n",                                                     // 18
            "[dependencies.regex]\n",                                 // 19
            "version = \"1\"\n",                                      // 20
        );
        let s = |x: &str| x.to_string();
        assert_eq!(
            found(text),
            [
                // No workspace manifest declares it: an external crate.
                (
                    5,
                    s("storage-api"),
                    s("storage-api"),
                    Source::Registry,
                    None
                ),
                (6, s("serde_json"), s("serde_json"), Source::Registry, None),
                (
                    7,
                    s("local"),
                    s("local"),
                    Source::Path(s("crates/local")),
                    None
                ),
                (8, s("json"), s("serde_json"), Source::Registry, None),
                (
                    11,
                    s("testkit"),
                    s("testkit"),
                    Source::Registry,
                    Some(s("dev-dependencies"))
                ),
                (
                    14,
                    s("cc"),
                    s("cc"),
                    Source::Registry,
                    Some(s("build-dependencies"))
                ),
                (17, s("libc"), s("libc"), Source::Registry, None),
                (19, s("regex"), s("regex"), Source::Registry, None),
            ]
        );
        let kinds: Vec<_> = listed_in("crates/chunk-store/Cargo.toml", text, &[])
            .declared
            .into_iter()
            .map(|d| d.part.and_then(|part| part.kind))
            .collect();
        assert_eq!(kinds[4], Some(Kind::Test));
        assert_eq!(kinds[5], None);
    }

    #[test]
    fn a_path_that_leaves_the_project_is_no_package_of_it() {
        let text = "[dependencies]\noutside = { path = \"../../../elsewhere\" }\n";
        assert_eq!(
            found(text),
            [(
                2,
                "outside".to_string(),
                "outside".to_string(),
                Source::Registry,
                None
            )]
        );
    }

    #[test]
    fn a_manifest_that_is_no_toml_is_unread_not_empty() {
        let read = |_: &str| None;
        let file = |text| ManifestFile {
            path: "Cargo.toml",
            text,
            read: &read,
        };
        assert_eq!(CARGO_TOML.declared(&file("[dependencies\nx = ")), None);
        assert_eq!(
            CARGO_TOML.declared(&file("[package]\nname = \"a\"\n")),
            Some(Listed::default())
        );
    }

    #[test]
    fn the_same_name_in_two_tables_is_two_dependencies() {
        let text =
            "[dependencies]\nlibc = \"0.2\"\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n";
        let lines: Vec<_> = found(text).iter().map(|d| d.0).collect();
        assert_eq!(lines, [2, 5]);
    }

    #[test]
    fn the_lines_of_a_multi_line_table_are_no_dependencies() {
        let text = "[dependencies]\nfoo = {\n    version = \"1\",\n    path = \"../foo\",\n}\nversion = \"2\"\n";
        let names: Vec<_> = found(text).iter().map(|d| (d.0, d.1.clone())).collect();
        assert_eq!(names, [(2, "foo".to_string()), (6, "version".to_string())]);
    }

    const WORKSPACE: (&str, &str) = (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nstorage-api = { path = \"crates/storage-api\" }\nserde = \"1\"\nhttp = { package = \"real-http\", version = \"1\" }\n",
    );

    /// (name, package, source) of each dependency of `crates/chunk-store`.
    fn from_workspace(text: &str, project: &[(&str, &str)]) -> Vec<(String, String, Source)> {
        listed_in("crates/chunk-store/Cargo.toml", text, project)
            .declared
            .into_iter()
            .map(|d| (d.name, d.package, d.source))
            .collect()
    }

    #[test]
    fn a_workspace_dependency_is_what_the_workspace_manifest_declares() {
        let text = "[dependencies]\nstorage-api = { workspace = true }\nserde = { workspace = true }\nhttp = { workspace = true }\nmissing = { workspace = true }\n";
        let s = |x: &str| x.to_string();
        assert_eq!(
            from_workspace(text, &[WORKSPACE]),
            [
                // Its path is the workspace's, from the workspace's directory.
                (
                    s("storage-api"),
                    s("storage-api"),
                    Source::Path(s("crates/storage-api"))
                ),
                (s("serde"), s("serde"), Source::Registry),
                // A renamed one is the package the workspace says.
                (s("http"), s("real-http"), Source::Registry),
                (s("missing"), s("missing"), Source::Registry),
            ]
        );
    }

    /// The closest manifest above the crate with a `[workspace]` is its workspace;
    /// one without is passed over.
    #[test]
    fn the_closest_workspace_above_the_crate_declares_it() {
        let text = "[dependencies]\nserde = { workspace = true }\n";
        let inner = (
            "crates/Cargo.toml",
            "[workspace]\n\n[workspace.dependencies]\nserde = { path = \"vendored/serde\" }\n",
        );
        let plain = (
            "crates/chunk-store/Cargo.toml",
            "[package]\nname = \"chunk-store\"\n",
        );
        assert_eq!(
            from_workspace(text, &[WORKSPACE, inner, plain]),
            [(
                "serde".to_string(),
                "serde".to_string(),
                Source::Path("crates/vendored/serde".to_string())
            )]
        );
    }

    #[test]
    fn a_workspace_manifest_that_cannot_be_read_is_said() {
        let listed = listed_in(
            "crates/a/Cargo.toml",
            "[dependencies]\napi = { workspace = true }\n",
            &[("Cargo.toml", "[workspace\n")],
        );
        assert_eq!(listed.declared[0].source, Source::Registry);
        let [problem] = listed.problems.as_slice() else {
            panic!("{:?}", listed.problems);
        };
        assert_eq!(
            problem.reason,
            "crates/a/Cargo.toml: `api` is `workspace = true`, but Cargo.toml cannot be read, so it is taken for an external crate"
        );
        assert_eq!(problem.files, ["crates/a/Cargo.toml"]);
    }

    /// (line, name, the part it leaves, the part of the target it reaches, source)
    type Row = (
        usize,
        String,
        Option<String>,
        Option<Option<String>>,
        Source,
    );

    /// The dependencies of a crate at the root of the project, alone in it.
    fn edges_of(text: &str) -> Vec<Row> {
        listed_in("Cargo.toml", text, &[])
            .declared
            .into_iter()
            .map(|d| {
                (
                    d.line,
                    d.name,
                    d.part.map(|p| p.name),
                    d.reaches.map(|reach| reach.part.map(|p| p.name)),
                    d.source,
                )
            })
            .collect()
    }

    #[test]
    fn features_declare_what_they_turn_on_and_an_optional_dependency_belongs_to_it() {
        let text = concat!(
            "[features]\n",                                                 // 1
            "default = [\"fake\"]\n",                                       // 2
            "fake = [\"storage-api/fake\", \"dep:serde\", \"other\"]\n",    // 3
            "other = [\"tls?/rustls\"]\n",                                  // 4
            "\n",                                                           // 5
            "[dependencies]\n",                                             // 6
            "storage-api = { workspace = true, features = [\"extra\"] }\n", // 7
            "serde = { version = \"1\", optional = true }\n",               // 8
            "tls = { version = \"1\", optional = true }\n",                 // 9
            "gzip = { version = \"1\", optional = true }\n",                // 10
        );
        let s = |x: &str| Some(x.to_string());
        let reaches = |x: &str| Some(Some(x.to_string()));
        let n = |x: &str| x.to_string();
        let own = || Source::Path(String::new());
        assert_eq!(
            edges_of(text),
            [
                // `default` is the package itself; `fake` is a feature of it.
                (2, n("fake"), None, reaches("feature/fake"), own()),
                (
                    3,
                    n("storage-api"),
                    s("feature/fake"),
                    None,
                    Source::Registry
                ),
                (
                    3,
                    n("storage-api"),
                    s("feature/fake"),
                    reaches("feature/fake"),
                    Source::Registry
                ),
                (3, n("serde"), s("feature/fake"), None, Source::Registry),
                (
                    3,
                    n("other"),
                    s("feature/fake"),
                    reaches("feature/other"),
                    own()
                ),
                // A weak item reaches the feature of `tls`, not `tls`.
                (
                    4,
                    n("tls"),
                    s("feature/other"),
                    reaches("feature/rustls"),
                    Source::Registry
                ),
                (7, n("storage-api"), None, None, Source::Registry),
                (
                    7,
                    n("storage-api"),
                    None,
                    reaches("feature/extra"),
                    Source::Registry
                ),
                // `serde` is named by `dep:serde`, so it has no feature of its own;
                // `tls` and `gzip` have the implicit one.
                (9, n("tls"), s("feature/tls"), None, Source::Registry),
                (10, n("gzip"), s("feature/gzip"), None, Source::Registry),
            ]
        );
    }

    /// `default` is no part: a feature item that names it reaches the package itself.
    #[test]
    fn the_default_feature_is_the_whole_package() {
        let rows = edges_of("[features]\ndefault = []\nfull = [\"default\"]\n");
        assert_eq!(
            rows,
            [(
                3,
                "default".to_string(),
                Some("feature/full".to_string()),
                Some(None),
                Source::Path(String::new())
            )]
        );
    }

    #[test]
    fn a_literal_string_is_an_item_too() {
        let rows = edges_of(
            "[features]\nfake = ['dep:serde']\n\n[dependencies]\nserde = { version = \"1\", optional = true }\n",
        );
        assert_eq!(
            rows,
            [(
                2,
                "serde".to_string(),
                Some("feature/fake".to_string()),
                None,
                Source::Registry
            )]
        );
    }

    #[test]
    fn an_item_takes_what_the_dependencies_declare_not_the_dev_ones() {
        let text = concat!(
            "[dev-dependencies]\n",
            "x = { path = \"x\" }\n",
            "\n",
            "[dependencies]\n",
            "x = \"1\"\n",
            "\n",
            "[features]\n",
            "f = [\"x/g\"]\n",
        );
        let items: Vec<_> = edges_of(text).into_iter().filter(|r| r.0 == 8).collect();
        let f = Some("feature/f".to_string());
        assert_eq!(
            items,
            [
                (8, "x".to_string(), f.clone(), None, Source::Registry),
                (
                    8,
                    "x".to_string(),
                    f,
                    Some(Some("feature/g".to_string())),
                    Source::Registry
                ),
            ]
        );
    }

    #[test]
    fn an_item_that_names_no_dependency_declares_none() {
        let rows = edges_of("[features]\nf = [\"typo/g\", \"dep:typo\"]\n");
        assert!(rows.is_empty(), "{rows:?}");
    }

    /// The name a part of a feature is goes by is the one `part:` is told of.
    #[test]
    fn a_feature_is_the_part_its_name_says() {
        assert_eq!(PART_NAMES[0], feature_part_name("<name>"));
        assert_eq!(
            feature_part("fake").map(|p| p.name),
            Some("feature/fake".into())
        );
        assert_eq!(feature_part("default"), None);
    }

    #[test]
    fn reads_what_a_crate_lets_its_files_import() {
        let cargo = importable(
            &CARGO_TOML,
            "[package]\nname = \"my-app\"\n\n[dependencies]\nserde = \"1\"\nserde-json = { version = \"1\" }\njson5 = { package = \"json-five\", version = \"1\" }\n\n[dev-dependencies.insta]\nversion = \"1\"\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n",
        );
        for name in [
            "my_app",
            "serde",
            "serde_json",
            "insta",
            "libc",
            "json5",
            "json_five",
        ] {
            assert!(cargo.contains(name), "{name}");
        }
        assert_eq!(
            CARGO_TOML.spelled_in_import("serde-json".into()),
            "serde_json"
        );
    }

    /// `[workspace.dependencies]` sets the versions members take with
    /// `workspace = true`; the package at the root may not import one it does not
    /// list itself, as `cargo build` says. A feature item names no crate.
    #[test]
    fn a_workspace_dependency_is_importable_only_where_a_package_lists_it() {
        let root = importable(
            &CARGO_TOML,
            "[package]\nname = \"ws\"\n\n[workspace]\nmembers = [\"a\"]\n\n[workspace.dependencies]\nserde = \"1\"\n\n[dependencies]\nlog = \"0.4\"\n\n[features]\nextra = []\nall = [\"extra\"]\n",
        );
        assert_eq!(root, ["log", "ws"].map(str::to_string).into());
        let member = importable(
            &CARGO_TOML,
            "[package]\nname = \"a\"\n\n[dependencies]\nserde = { workspace = true }\n",
        );
        assert_eq!(member, ["a", "serde"].map(str::to_string).into());
    }

    /// (line, the part it leaves, what it reaches and the part of it) for each dependency.
    fn edge_list(files: &[(&str, &str)]) -> Vec<(usize, Option<String>, String, Option<String>)> {
        let dir = tempfile::TempDir::new().unwrap();
        let mut names = Vec::new();
        for (path, text) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
            names.push(path.to_string());
        }
        let found = declared_by(dir.path(), &names, |_| true);
        assert!(found.problems.is_empty(), "{:?}", found.problems);
        found
            .deps
            .into_iter()
            .filter(|dep| dep.file.to_str() == Some("crates/telegram/Cargo.toml"))
            .map(|dep| {
                let (key, part) = match &dep.target {
                    Target::Internal(unit) => (unit.key(), unit.part_name().map(str::to_string)),
                    Target::External(module) => (module.clone(), None),
                };
                (
                    dep.line,
                    dep.from.part_name().map(str::to_string),
                    key,
                    part,
                )
            })
            .collect()
    }

    #[test]
    fn a_feature_item_reaches_the_feature_of_another_manifest_of_the_project() {
        let s = |x: &str| Some(x.to_string());
        let n = |x: &str| x.to_string();
        let got = edge_list(&[
            (
                "Cargo.toml",
                "[workspace]\n\n[workspace.dependencies]\nstorage-api = { path = \"crates/storage-api\" }\n",
            ),
            (
                "crates/storage-api/Cargo.toml",
                "[package]\nname = \"storage-api\"\n",
            ),
            (
                "crates/telegram/Cargo.toml",
                concat!(
                    "[features]\n",                                      // 1
                    "default = [\"fake\"]\n",                            // 2
                    "fake = [\"storage-api/fake\", \"serde/derive\"]\n", // 3
                    "\n",                                                // 4
                    "[dependencies]\n",                                  // 5
                    "storage-api = { workspace = true }\n",              // 6
                    "serde = \"1\"\n",                                   // 7
                ),
            ),
        ]);
        let kit = n("crates/storage-api/Cargo.toml");
        let own = n("crates/telegram/Cargo.toml");
        assert_eq!(
            got,
            [
                // `default` is the package itself, and `fake` a feature of the manifest.
                (2, None, own, s("feature/fake")),
                (3, s("feature/fake"), kit.clone(), None),
                (3, s("feature/fake"), kit.clone(), s("feature/fake")),
                // An external package has no part of the project to reach.
                (3, s("feature/fake"), n("serde"), None),
                (6, None, kit, None),
                (7, None, n("serde"), None),
            ]
        );
    }

    #[test]
    fn default_as_a_target_is_the_package_itself() {
        let own = "crates/telegram/Cargo.toml".to_string();
        let got = edge_list(&[(
            "crates/telegram/Cargo.toml",
            "[features]\ndefault = []\nfull = [\"default\"]\n",
        )]);
        assert_eq!(got, [(3, Some("feature/full".to_string()), own, None)]);
    }
}
