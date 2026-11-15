//! `Cargo.toml`: the dependency tables, the features, and the workspace.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::{Declared, Declares, Source, line_at, manifest_in};
use crate::lang::parts::{Kind, Part};
use crate::lang::registry::Manifest;
use crate::relpath::parent;

/// What a workspace manifest declares under a name.
#[derive(Clone)]
pub(super) struct WorkspaceEntry {
    /// The directory of the workspace manifest.
    pub(super) dir: String,
    pub(super) path: Option<String>,
    pub(super) package: Option<String>,
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

/// The workspace manifests of one run: each `Cargo.toml` is read and parsed once,
/// however many crates below it take dependencies from it. It lives for one run, so
/// a manifest edited since is read afresh on the next.
pub(super) struct Workspaces<'r> {
    read: &'r dyn Fn(&str) -> Option<String>,
    by_manifest: HashMap<String, Said>,
}

impl<'r> Workspaces<'r> {
    /// `read` gives the text of a file of the project.
    pub(super) fn new(read: &'r dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            read,
            by_manifest: HashMap::new(),
        }
    }

    /// What the closest workspace manifest above `dir` declares under `name`. `Err`
    /// names a manifest that holds `[workspace]` text it cannot read as TOML.
    pub(super) fn dependency(
        &mut self,
        dir: &str,
        name: &str,
    ) -> Result<Option<WorkspaceEntry>, String> {
        let mut here = Some(dir);
        while let Some(current) = here {
            let manifest = manifest_in(Manifest::Cargo, current);
            let read = self.read;
            let said = self
                .by_manifest
                .entry(manifest)
                .or_insert_with_key(|manifest| {
                    read(manifest).map_or(Said::Nothing, |text| said_by(&text, current))
                });
            match said {
                Said::Nothing => {}
                Said::Unreadable => return Err(manifest_in(Manifest::Cargo, current)),
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

const CARGO_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// A table of dependencies of `Cargo.toml`: which one, and under which `cfg` of
/// `[target.<cfg>.…]` when one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Table<'a> {
    kind: &'a str,
    target: Option<String>,
}

pub(super) fn declared(text: &str) -> Option<Vec<Declared>> {
    let document = DeTable::parse(text).ok()?;
    let manifest = document.get_ref();
    let line = |span: Range<usize>| line_at(text, span.start);
    // The tables of dependencies: the plain ones and those of each `[target.<cfg>]`.
    let mut tables: Vec<(Table, &DeTable)> = CARGO_TABLES
        .iter()
        .filter_map(|kind| Some((Table { kind, target: None }, table(manifest, kind)?)))
        .collect();
    for (cfg, target) in table(manifest, "target").into_iter().flatten() {
        if let Some(target) = target.get_ref().as_table() {
            tables.extend(CARGO_TABLES.iter().filter_map(|kind| {
                let here = Table {
                    kind,
                    target: Some(cfg.get_ref().to_string()),
                };
                Some((here, table(target, kind)?))
            }));
        }
    }
    let mut found: Vec<Declared> = tables
        .iter()
        .flat_map(|(here, dependencies)| {
            dependencies.iter().flat_map(|(name, spec)| {
                cargo_declared(here, name.get_ref(), spec.get_ref(), line(name.span()))
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

/// The part a feature is, `default` being the package itself.
fn feature_part(feature: &str) -> Option<Part> {
    (feature != "default").then(|| Part {
        name: format!("feature/{feature}"),
        kind: None,
    })
}

/// The dependencies of the tables, put where the features say they belong: an optional
/// one is a dependency of what turns it on, and of its implicit feature when nothing
/// names it with `dep:`; and the items of the features, which are edges of their own.
fn with_features(declared: Vec<Declared>, items: &[(usize, String, String)]) -> Vec<Declared> {
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
    let mut out: Vec<Declared> = Vec::new();
    for d in &declared {
        if !d.optional {
            out.push(d.clone());
        } else if !named(&d.name) {
            // The implicit feature `name` turns it on.
            out.push(Declared {
                part: feature_part(&d.name),
                ..d.clone()
            });
        } else if d.feature.is_some() {
            for owner in owners(&d.name) {
                out.push(Declared {
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
            table_dependency(name).map(|d| Declared {
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
                out.push(Declared {
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
            Item::Feature(name) => out.push(Declared {
                line: *line,
                name: name.to_string(),
                package: name.to_string(),
                source: Source::Own,
                part: from.clone(),
                target: None,
                feature: Some(name.to_string()),
                optional: false,
            }),
        }
    }
    out.sort_by_key(|d| d.line);
    out
}

/// The dependency `name` of `table`, declared by `spec` on `line`.
fn cargo_declared(table: &Table, name: &str, spec: &DeValue, line: usize) -> Vec<Declared> {
    let text_of = |key: &str| spec.get(key).and_then(|value| value.get_ref().as_str());
    let flag = |key: &str| spec.get(key).and_then(|value| value.get_ref().as_bool()) == Some(true);
    let package = text_of("package").unwrap_or(name).to_string();
    let optional = flag("optional");
    let source = if flag("workspace") {
        Source::Workspace
    } else if let Some(path) = text_of("path") {
        Source::Path(path.to_string())
    } else {
        Source::Registry
    };
    let own = Declared {
        line,
        name: name.to_string(),
        package,
        source,
        part: part_of(table.kind),
        target: table.target.clone(),
        feature: None,
        optional,
    };
    // `features = ["f"]` also reaches the feature `f` of the package.
    let features: Vec<Declared> = spec
        .get("features")
        .and_then(|features| features.get_ref().as_array())
        .into_iter()
        .flatten()
        .filter_map(|feature| feature.get_ref().as_str())
        .map(|f| Declared {
            feature: Some(f.to_string()),
            ..own.clone()
        })
        .collect();
    std::iter::once(own).chain(features).collect()
}

/// The part of a manifest a table of dependencies is: none for the plain one.
fn part_of(kind: &str) -> Option<Part> {
    match kind {
        "dev-dependencies" => Some(Part {
            name: kind.to_string(),
            kind: Some(Kind::Test),
        }),
        "build-dependencies" => Some(Part {
            name: kind.to_string(),
            kind: None,
        }),
        _ => None,
    }
}

/// The crate a Rust path belongs to (already written with `_`).
pub(super) fn package(module: &str) -> String {
    module.split("::").next().unwrap_or(module).to_string()
}

/// What the files of a crate may import: its own name and the dependencies its
/// `Cargo.toml` declares, by name and by the real crate a renamed one is, all written
/// with `_`, read as `packages` reads them. `[workspace.dependencies]` only sets what
/// a member takes with `workspace = true`, so it adds nothing here.
pub(super) fn importable(text: &str) -> Declares {
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
            declared(text)
                .unwrap_or_default()
                .into_iter()
                .filter(|dependency| dependency.source != Source::Own)
                .flat_map(|dependency| [dependency.name, dependency.package]),
        )
        .map(|name| name.replace('-', "_"))
        .collect();
    Declares::Package(names)
}

/// The crates Rust source names as the root of a path or in an `extern crate`.
pub(super) fn named_in_source(source: &str) -> BTreeSet<String> {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|[^\w:])(\w+)::|\bextern\s+crate\s+(\w+)\b").expect("valid regex")
    });
    NAMED
        .captures_iter(source)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|name| name.as_str().to_string())
        .collect()
}
