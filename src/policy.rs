//! The policy as loaded: the config merged from the files it extends, every
//! policy input with its content, and the input that defined each component
//! and rule. It is read from the working tree, or as it was at a Git revision.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_norway::{Mapping, Value};

use crate::config::{Config, field_key};
use crate::relpath::real;

/// What a policy input is to the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// The configuration the policy is loaded from.
    Config,
    /// A configuration that `extends` names, directly or through another one.
    Extended,
    /// The diagram that `diagram` names.
    Diagram,
}

/// A file whose content is part of the policy, as it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInput {
    pub kind: InputKind,
    /// Absolute, where the file is in the working tree.
    pub path: PathBuf,
    pub content: String,
    /// Read from disk although a revision was asked, because the file is
    /// outside the repository: its content at that revision is unknown.
    pub past_unknown: bool,
}

/// The policy and the inputs it was loaded from.
#[derive(Debug)]
pub struct Policy {
    pub config: Config,
    /// The configuration first, then every file it extends in the order they
    /// are read, then the diagram if it could be read.
    pub inputs: Vec<PolicyInput>,
    /// The input that last defined an entry, by (`components` or `rules`, name).
    origins: Origins,
    /// Where each of `config.examples` is written.
    examples: Vec<ExampleOrigin>,
    /// Why the diagram the config names could not be read; raised only by the
    /// rule that needs it, so other commands work without it.
    diagram_error: Option<String>,
}

/// Where a policy example is written: the index of its input in
/// `Policy::inputs`, and its own index among the examples of that input.
#[derive(Debug, Clone, Copy)]
struct ExampleOrigin {
    input: usize,
    item: usize,
}

/// The input that last defined an entry, by (key, name): `components` and
/// `rules` by entry name, and the top-level keys under an empty key.
type Origins = BTreeMap<(String, String), usize>;

impl Policy {
    /// Loads the policy from the configuration at `path`, as it is in the
    /// working tree or, with `revision`, as it was at that Git revision.
    pub fn load(path: &Path, revision: Option<&str>) -> Result<Self> {
        let mut loader = Loader {
            reader: Reader::new(path, revision)?,
            stack: Vec::new(),
            inputs: Vec::new(),
            origins: Origins::new(),
            examples: Vec::new(),
        };
        let value = loader.load(path, InputKind::Config)?;
        let Loader {
            reader,
            mut inputs,
            mut origins,
            examples,
            ..
        } = loader;
        let config = Config::from_merged(Value::Mapping(value), path)?;
        if config.tests.is_some()
            && let Some(&at) = origins.get(&(String::new(), "tests".to_string()))
        {
            // `tests: auto` makes the component.
            origins.insert(
                ("components".to_string(), crate::config::TESTS.to_string()),
                at,
            );
        }
        let mut diagram_error = None;
        if let Some(diagram) = &config.diagram {
            // Relative to the root, the directory of the configuration.
            let root = inputs[0].path.parent().unwrap_or(Path::new("/"));
            let path = real(&root.join(diagram))?;
            match reader.read(&path) {
                Ok((content, past_unknown)) => inputs.push(PolicyInput {
                    kind: InputKind::Diagram,
                    path,
                    content,
                    past_unknown,
                }),
                Err(err) => {
                    diagram_error = Some(format!(
                        "{:#}",
                        err.context(format!("cannot read the diagram {}", diagram.display()))
                    ));
                }
            }
        }
        Ok(Self {
            config,
            inputs,
            origins,
            examples,
            diagram_error,
        })
    }

    /// The configuration the policy is loaded from.
    pub fn config_input(&self) -> &PolicyInput {
        &self.inputs[0]
    }

    /// The diagram the config names, if it names one; an error when it cannot
    /// be read.
    pub fn diagram(&self) -> Result<Option<&PolicyInput>> {
        if let Some(error) = &self.diagram_error {
            return Err(anyhow!("{error}"));
        }
        Ok(self.inputs.iter().find(|i| i.kind == InputKind::Diagram))
    }

    /// The input that writes the `index`th of `config.examples`, and the line,
    /// counted from 1, of its item there.
    pub fn example_at(&self, index: usize) -> Option<(&PolicyInput, usize)> {
        let origin = self.examples.get(index)?;
        let input = self.inputs.get(origin.input)?;
        Some((input, item_line(&input.content, "examples", origin.item)))
    }

    /// The input that last defined `name` under `key`, `components`, `rules`
    /// or `sets`: an extending file replaces a whole entry of that name. Under
    /// `common`, the input that first listed `name`; under an empty key, the
    /// last that wrote the top-level key `name`.
    pub fn defined_by(&self, key: &str, name: &str) -> Option<&PolicyInput> {
        let at = self.origins.get(&(key.to_string(), name.to_string()))?;
        self.inputs.get(*at)
    }

    /// The line, counted from 1, where the configuration writes `value` under the
    /// keys `path` (`components.web.in`, `rules.web.forbid_symbol`, `entry`, a key of
    /// digits being the index of an item, as in `forbid_symbol.0.allow_in`); 1 when
    /// a file it extends defines the entry that holds it, as reports then name the
    /// configuration.
    pub fn config_line(&self, path: &[&str], value: &str) -> usize {
        if !self.written_in_config(path) {
            return 1;
        }
        written_line(&self.config_input().content, path, value)
    }

    /// The line, counted from 1, where the configuration writes the key `key` of the
    /// mapping at `path` (`rules`, `sets`); 1 when a file it extends defines it.
    pub fn config_key_line(&self, path: &[&str], key: &str) -> usize {
        let full: Vec<&str> = path.iter().copied().chain([key]).collect();
        if !self.written_in_config(&full) {
            return 1;
        }
        written_key_line(&self.config_input().content, path, key)
    }

    /// Whether the configuration itself, not a file it extends, defines the entry
    /// that holds what is written at `path`.
    fn written_in_config(&self, path: &[&str]) -> bool {
        let (key, name) = match path {
            [section @ ("components" | "rules" | "sets"), name, ..] => (*section, *name),
            [key, ..] => ("", *key),
            [] => return false,
        };
        self.defined_by(key, name)
            .is_some_and(|by| by.kind == InputKind::Config)
    }
}

/// Where the policy writes a clause: its normalized field path
/// (`rules.web.must_not_depend_on[1]`), and the entry whose defining input
/// holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub field: String,
    /// `components` or `rules` and the name of the entry, `common` and the
    /// name it lists, or an empty key and a top-level key.
    entry: (&'static str, String),
    /// The globs and sets that bring the clause to the component it is about:
    /// the key of its rule, then the item of its list.
    through: Vec<String>,
}

impl Site {
    /// A clause of the entry `name` under `section`, `components` or `rules`.
    pub fn of_entry(section: &'static str, name: &str, field: String) -> Self {
        Self {
            field,
            entry: (section, name.to_string()),
            through: Vec::new(),
        }
    }

    /// The rule of `component`, or the clause of it at `rest` (`.because`), under
    /// the key of `rules` that writes it: a glob or a set may cover the component.
    pub fn of_rule(config: &Config, component: &str, rest: &str) -> Self {
        let key = config.rule_key(component);
        Self {
            field: format!("rules.{}{rest}", field_key(key)),
            entry: ("rules", key.to_string()),
            through: (key != component)
                .then(|| key.to_string())
                .into_iter()
                .collect(),
        }
    }

    /// The item that names `name` in the list `list` (`may_depend_on`) of the rule
    /// of `component`: the name itself, or the glob or set it comes through.
    pub fn of_rule_item(config: &Config, component: &str, list: &str, name: &str) -> Option<Self> {
        let written = config.written(&format!("rules.{component}.{list}"), name)?;
        let mut site = Self::of_rule(config, component, &format!(".{list}[{}]", written.index));
        // `'ee-*': { may_depend_on: ['ee-*'] }` brings both through one glob.
        if let Some(through) = &written.through
            && !site.through.contains(through)
        {
            site.through.push(through.clone());
        }
        Some(site)
    }

    /// `layers`, where it puts `from` and `to`, with the globs and sets of the rows
    /// that name them.
    pub fn of_layers(config: &Config, from: &str, to: &str) -> Self {
        let mut site = Self::of_key("layers");
        for name in [from, to] {
            if let Some(through) = config
                .written("layers", name)
                .and_then(|w| w.through.clone())
                && !site.through.contains(&through)
            {
                site.through.push(through);
            }
        }
        site
    }

    /// The item that names `name` in the top-level list `key` (`no_cycles`): the
    /// name itself, or the glob or set it comes through.
    pub fn of_top_item(config: &Config, key: &str, name: &str) -> Option<Self> {
        let written = config.written(key, name)?;
        let mut site = Self::in_key(key, format!("{key}[{}]", written.index));
        site.through.extend(written.through.clone());
        Some(site)
    }

    /// The globs and sets that bring the clause to the component it is about.
    pub fn through(&self) -> &[String] {
        &self.through
    }

    /// The top-level `key`, a clause in itself.
    pub fn of_key(key: &str) -> Self {
        Self::in_key(key, key.to_string())
    }

    /// A part of the top-level `key` at `field`, such as an item of its list
    /// (`forbid_symbol[2]`): the input that last wrote the key holds it.
    pub fn in_key(key: &str, field: String) -> Self {
        Self {
            field,
            entry: ("", key.to_string()),
            through: Vec::new(),
        }
    }

    /// The item of `common` that names `name`, itself or the glob or set it comes
    /// through, at its index in the merged list.
    pub fn of_common(config: &Config, name: &str) -> Option<Self> {
        let written = config.written("common", name)?;
        Some(Self {
            field: format!("common[{}]", written.index),
            entry: (
                "common",
                written.through.clone().unwrap_or_else(|| name.to_string()),
            ),
            through: written.through.clone().into_iter().collect(),
        })
    }
}

/// Where the policy writes a clause, as reports give it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    /// The normalized field path in the merged policy.
    pub field: String,
    /// The policy input that defined it, relative to the root of the policy,
    /// or absolute when it is outside the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// The globs and sets, as written, that bring the clause to the component it is
    /// about: `api-*` for a rule under `rules.'api-*'`, then `@drivers` for an item
    /// of its list; empty when the policy names the component.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub through: Vec<String>,
}

impl Policy {
    /// Where `site` is written: its field path and the input that defined it.
    pub fn provenance(&self, site: &Site) -> Provenance {
        let (section, name) = &site.entry;
        Provenance {
            field: site.field.clone(),
            input: self.defined_by(section, name).map(|i| self.input_name(i)),
            through: site.through.clone(),
        }
    }

    /// How reports name `input`: relative to the root of the policy, the
    /// directory of the configuration, or absolute outside it.
    pub fn input_name(&self, input: &PolicyInput) -> String {
        let root = self.inputs[0].path.parent().unwrap_or(Path::new("/"));
        crate::relpath::slash(input.path.strip_prefix(root).unwrap_or(&input.path))
    }
}

/// Where the inputs are read from: the working tree, or a Git revision.
struct Reader {
    revision: Option<Revision>,
}

struct Revision {
    rev: String,
    /// The top of the repository, as a real path.
    top: PathBuf,
}

impl Reader {
    fn new(config: &Path, revision: Option<&str>) -> Result<Self> {
        let Some(rev) = revision else {
            return Ok(Self { revision: None });
        };
        let dir = real(config)?
            .parent()
            .map(Path::to_path_buf)
            .context("config has no parent directory")?;
        let top = crate::diff::git(&dir, &["rev-parse", "--show-toplevel"]).with_context(|| {
            format!("cannot read the policy at `{rev}`: not in a Git repository")
        })?;
        Ok(Self {
            revision: Some(Revision {
                rev: rev.to_string(),
                top: real(Path::new(top.trim()))?,
            }),
        })
    }

    /// The path that tells two inputs apart, so that a loop of `extends` is
    /// found: the real path in the working tree, the normalized one at a
    /// revision, where the file may no longer exist.
    fn identify(&self, path: &Path) -> Result<PathBuf> {
        match &self.revision {
            None => dunce::canonicalize(path)
                .with_context(|| format!("cannot read config {}", path.display())),
            Some(_) => real(path),
        }
    }

    /// The content of the file at `path` (absolute and normalized), and
    /// whether it was read from disk although its past was asked.
    fn read(&self, path: &Path) -> Result<(String, bool)> {
        let Some(Revision { rev, top }) = &self.revision else {
            return Ok((std::fs::read_to_string(path)?, false));
        };
        let Ok(inside) = path.strip_prefix(top) else {
            return Ok((std::fs::read_to_string(path)?, true));
        };
        let inside = crate::relpath::slash(inside);
        let text = crate::diff::file_at(top, rev, &inside)?
            .with_context(|| format!("{inside} is not in {rev}"))?;
        Ok((text, false))
    }
}

/// Reads the configurations of a policy, keeping each file it reads and the
/// input that last defined each entry.
struct Loader {
    reader: Reader,
    /// The files being read, the outermost first: one read again extends itself.
    stack: Vec<PathBuf>,
    inputs: Vec<PolicyInput>,
    origins: Origins,
    /// The origin of each example, in the order the merge joins them.
    examples: Vec<ExampleOrigin>,
}

impl Loader {
    /// Reads a config file and merges the files it `extends` underneath it.
    ///
    /// `extends` is a path or a list of paths, relative to the file that names them.
    /// Patterns in every file stay relative to the root of the config being loaded.
    /// Keys that start with `x-` are dropped, so they can hold anchors for `<<`.
    /// `components`, `rules` and `sets` merge by name, the later definition replacing the
    /// earlier one; `common`, `exclude` and `target_only` are concatenated, and so is
    /// `examples`, the bases' first and without dropping repeats; any other key is
    /// replaced by the extending file.
    fn load(&mut self, path: &Path, kind: InputKind) -> Result<Mapping> {
        let identity = self.reader.identify(path)?;
        if self.stack.contains(&identity) {
            bail!("config {} extends itself", path.display());
        }
        let (text, past_unknown) = self
            .reader
            .read(&identity)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let mut value = serde_norway::from_str::<Value>(&text)
            .with_context(|| format!("invalid config {}", path.display()))?;
        // `<<: *anchor` shares a part of a rule between components; a key that
        // replaces a merged list must not lift what the anchor shares unseen.
        refuse_dropped_merges(&value, "")
            .with_context(|| format!("invalid config {}", path.display()))?;
        apply_merges(&mut value).with_context(|| format!("invalid config {}", path.display()))?;
        let Value::Mapping(mut own) = value else {
            bail!("config {} is not a mapping", path.display());
        };
        let at = self.inputs.len();
        self.inputs.push(PolicyInput {
            kind,
            path: identity.clone(),
            content: text,
            past_unknown,
        });
        // `x-` keys are only places to define anchors.
        own.retain(|key, _| !key.as_str().is_some_and(|k| k.starts_with("x-")));
        let bases = match own.remove("extends") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(one)) => vec![one],
            Some(other) => serde_norway::from_value::<Vec<String>>(other).with_context(|| {
                format!("`extends` in {} must be a path or a list", path.display())
            })?,
        };
        // At a revision `..` is resolved by name, so it starts from the real
        // directory, as the working tree resolves it after the links.
        let dir = match self.reader.revision {
            None => path.parent(),
            Some(_) => identity.parent(),
        }
        .unwrap_or(Path::new("."))
        .to_path_buf();
        self.stack.push(identity);
        let mut merged = Mapping::new();
        for base in bases {
            let base = self.load(&dir.join(base), InputKind::Extended)?;
            merge(&mut merged, base);
        }
        self.stack.pop();
        // The bases pushed theirs while they were read: these follow them, as
        // `merge` appends them.
        if let Some(Value::Sequence(items)) = own.get("examples") {
            self.examples
                .extend((0..items.len()).map(|item| ExampleOrigin { input: at, item }));
        }
        for (key, value) in &own {
            let key = key.as_str().unwrap_or_default();
            self.origins.insert((String::new(), key.to_string()), at);
            match (key, value) {
                ("components" | "rules" | "sets", Value::Mapping(entries)) => {
                    for name in entries.keys().filter_map(Value::as_str) {
                        self.origins.insert((key.to_string(), name.to_string()), at);
                    }
                }
                // Merged lists keep an item where it was first written.
                ("common", Value::Sequence(items)) => {
                    for name in items.iter().filter_map(Value::as_str) {
                        self.origins
                            .entry((key.to_string(), name.to_string()))
                            .or_insert(at);
                    }
                }
                _ => {}
            }
        }
        merge(&mut merged, own);
        Ok(merged)
    }
}

fn merge(into: &mut Mapping, from: Mapping) {
    for (key, value) in from {
        let name = key.as_str().unwrap_or_default();
        match (into.get_mut(&key), value) {
            (Some(Value::Mapping(old)), Value::Mapping(new))
                if matches!(name, "components" | "rules" | "sets") =>
            {
                for (k, v) in new {
                    old.insert(k, v);
                }
            }
            (Some(Value::Sequence(old)), Value::Sequence(new))
                if matches!(name, "common" | "exclude" | "target_only") =>
            {
                for v in new {
                    if !old.contains(&v) {
                        old.push(v);
                    }
                }
            }
            (Some(Value::Sequence(old)), Value::Sequence(new)) if name == "examples" => {
                old.extend(new);
            }
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

/// Fails on a key of a mapping with `<<` whose kept value drops items that a
/// value it overrides brings in. YAML keeps the key written next to `<<`, else
/// the value of the first merged mapping that has it, and silently discards
/// the others: a list replaced, not joined, would lift every shared item. A
/// list must keep each item of the lists it overrides and a mapping each key,
/// at any depth, and a null (`~`, or no value) keeps none of them; a scalar
/// or another change of type is left to the schema. `path` is where `value`
/// is written, `a.b[0]`.
fn refuse_dropped_merges(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Mapping(mapping) => {
            if let Some(merge) = mapping.get("<<") {
                refuse_dropped_keys(value, merge, path)?;
            }
            // `<<` too: a mapping written inline under it is checked there.
            mapping
                .iter()
                .try_for_each(|(key, child)| refuse_dropped_merges(child, &joined(path, key)))
        }
        Value::Sequence(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(i, item)| refuse_dropped_merges(item, &format!("{path}[{i}]"))),
        Value::Tagged(tagged) => refuse_dropped_merges(&tagged.value, path),
        _ => Ok(()),
    }
}

/// Fails when the value `apply_merges` keeps for a key of `mapping`, written at
/// `path`, drops items of the written or merged value of that key it overrides.
fn refuse_dropped_keys(mapping: &Value, merge: &Value, path: &str) -> Result<()> {
    // A `<<` that cannot be merged is left to `apply_merges` to report.
    let sources = match merge {
        Value::Mapping(_) => vec![merged(merge)],
        Value::Sequence(items) => items.iter().map(merged).collect(),
        _ => return Ok(()),
    };
    let (Some(sources), Some(Value::Mapping(kept))) = (
        sources.into_iter().collect::<Option<Vec<_>>>(),
        merged(mapping),
    ) else {
        return Ok(());
    };
    for (key, kept) in &kept {
        // `mapping` merged, so whatever is written in it merges too.
        let written = mapping.get(key).and_then(merged);
        let at = joined(path, key);
        let overridden = written
            .iter()
            .chain(sources.iter().filter_map(|source| source.get(key)));
        for value in overridden {
            let Some(Dropped { at, items }) = dropped(kept, value, &at) else {
                continue;
            };
            let whose = if written.is_some() {
                "written next to `<<`"
            } else {
                "from the first merged mapping that has it"
            };
            let key = rendered(key);
            bail!(
                "`{at}` {whose} drops {} that a `<<` merge brings in: the key kept replaces the merged one, it does not join it. \
                 To drop them on purpose, do not merge `{key}` there: split the anchor into smaller ones and write `{key}` out in full",
                items.join(", "),
            );
        }
    }
    Ok(())
}

/// `value` as it reads once its own `<<` merges are applied; none when they
/// cannot be, which the whole config then fails on.
fn merged(value: &Value) -> Option<Value> {
    let mut value = value.clone();
    apply_merges(&mut value).ok()?;
    Some(value)
}

/// Applies the `<<` merges of `value`, the innermost first, so a merged
/// mapping that merges another brings in what that one does.
/// `Value::apply_merge` alone merges the outermost first and copies the `<<`
/// of a merged mapping as a key, unmerged.
fn apply_merges(value: &mut Value) -> serde_norway::Result<()> {
    match value {
        Value::Mapping(mapping) => {
            for child in mapping.values_mut() {
                apply_merges(child)?;
            }
            if mapping.contains_key("<<") {
                // Every value under it is merged: this merges the mapping alone.
                value.apply_merge()?;
            }
            Ok(())
        }
        Value::Sequence(items) => items.iter_mut().try_for_each(apply_merges),
        Value::Tagged(tagged) => apply_merges(&mut tagged.value),
        _ => Ok(()),
    }
}

/// Items that a kept value lacks of a value it overrides.
struct Dropped {
    /// Where the kept value lacks them, `rules.web.forbid_symbol`.
    at: String,
    /// The items of a list, or the keys of a mapping, each on one line.
    items: Vec<String>,
}

/// What `kept` lacks of `lost`, both at `path`: items of a list, keys of a
/// mapping, the first such place at any depth. A null lacks them all.
fn dropped(kept: &Value, lost: &Value, path: &str) -> Option<Dropped> {
    let missing: Vec<String> = match (kept, lost) {
        (Value::Null, Value::Sequence(lost)) => lost.iter().map(rendered).collect(),
        (Value::Null, Value::Mapping(lost)) => lost.keys().map(rendered).collect(),
        (Value::Sequence(kept), Value::Sequence(lost)) => lost
            .iter()
            .filter(|item| !kept.contains(item))
            .map(rendered)
            .collect(),
        (Value::Mapping(kept), Value::Mapping(lost)) => {
            let missing: Vec<String> = lost
                .keys()
                .filter(|key| !kept.contains_key(*key))
                .map(rendered)
                .collect();
            if missing.is_empty() {
                return lost
                    .iter()
                    .find_map(|(key, lost)| dropped(kept.get(key)?, lost, &joined(path, key)));
            }
            missing
        }
        _ => return None,
    };
    (!missing.is_empty()).then(|| Dropped {
        at: path.to_string(),
        items: missing,
    })
}

/// The path to `key` under `path`.
fn joined(path: &str, key: &Value) -> String {
    let key = rendered(key);
    if path.is_empty() {
        key
    } else {
        format!("{path}.{key}")
    }
}

/// `value` written on one line: a string as it reads, anything else in flow style.
fn rendered(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| format!("{other:?}")),
    }
}

/// The line, counted from 1, of the `index`th item of the top-level list `key`; the
/// line of `key:` when the tree holds no such item, 1 when the text has no `key`.
fn item_line(text: &str, key: &str, index: usize) -> usize {
    let Ok(document) = yaml_edit::Document::from_str(text) else {
        return 1;
    };
    let item = document
        .get_sequence(key)
        .and_then(|list| list.get(index))
        .map(|item| item.byte_range().start);
    let at = item.or_else(|| key_start(&document, key));
    at.map_or(1, |at| line_at(text, at))
}

/// The line, counted from 1, where `text` writes `value` under the keys `path`: the
/// scalar itself, an item of the list, or the `name` of an item (`src/a/` is written
/// for `src/a`); 1 when it does not.
fn written_line(text: &str, path: &[&str], value: &str) -> usize {
    let Ok(document) = yaml_edit::Document::from_str(text) else {
        return 1;
    };
    let Some((first, rest)) = path.split_first() else {
        return 1;
    };
    // A key of digits is the index of an item of a list.
    let node = rest.iter().try_fold(document.get(*first), |node, key| {
        let node = node?;
        match (node.as_sequence(), key.parse::<usize>()) {
            (Some(list), Ok(index)) => Some(list.get(index)),
            _ => Some(node.as_mapping()?.get(*key)),
        }
    });
    node.flatten()
        .and_then(|node| holding(&node, value))
        .map_or(1, |at| line_at(text, at))
}

/// The line, counted from 1, where `text` writes the key `key` of the mapping at the
/// keys `path`; 1 when it does not.
fn written_key_line(text: &str, path: &[&str], key: &str) -> usize {
    let Ok(document) = yaml_edit::Document::from_str(text) else {
        return 1;
    };
    let Some((first, rest)) = path.split_first() else {
        return 1;
    };
    let mapping = rest
        .iter()
        .try_fold(document.get(*first), |node, k| {
            Some(node?.as_mapping()?.get(*k))
        })
        .flatten();
    mapping
        .as_ref()
        .and_then(|node| node.as_mapping())
        .and_then(|mapping| {
            mapping
                .keys()
                .find(|k| k.as_scalar().is_some_and(|k| k.as_string() == key))
        })
        .map_or(1, |k| line_at(text, k.byte_range().start))
}

/// Where the scalar of `node` that reads `value` starts.
fn holding(node: &yaml_edit::YamlNode, value: &str) -> Option<u32> {
    if let Some(scalar) = node.as_scalar() {
        let same = scalar.as_string().trim_end_matches('/') == value.trim_end_matches('/');
        return same.then(|| node.byte_range().start);
    }
    if let Some(list) = node.as_sequence() {
        return (0..list.len())
            .filter_map(|i| list.get(i))
            .find_map(|item| holding(&item, value));
    }
    holding(&node.as_mapping()?.get("name")?, value)
}

/// Where the top-level `key` is written.
fn key_start(document: &yaml_edit::Document, key: &str) -> Option<u32> {
    document
        .keys()
        .find(|k| k.as_scalar().is_some_and(|k| k.as_string() == key))
        .map(|k| k.byte_range().start)
}

/// The line, counted from 1, of the byte `at` of `text`.
fn line_at(text: &str, at: u32) -> usize {
    yaml_edit::byte_offset_to_line_column(text, at as usize).line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        crate::diff::git(dir, args).unwrap();
    }

    fn commit(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]);
        git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
    }

    /// `extends` joins the examples, the bases' first, and each keeps the
    /// file and line that write it, whether a block item spans lines or the
    /// list is a flow sequence.
    #[test]
    fn examples_join_through_extends_and_keep_where_they_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let example = |to: &str| format!("{{ from: a/x, to: {to}, expect: allowed }}");
        write(
            dir.path(),
            "deep.yml",
            &format!(
                "version: 1\nexamples: [{}, {}]\n",
                example("d1"),
                example("d2")
            ),
        );
        write(
            dir.path(),
            "base.yml",
            &format!(
                "extends: deep.yml\ncomponents:\n  a: {{ in: a }}\n# examples: [not, these]\nexamples:\n\n  # the base's own\n  - from: a/x\n    to: b1\n    expect: allowed\n  - {}\nrules: {{}}\n",
                example("b2")
            ),
        );
        write(
            dir.path(),
            "easement.yml",
            &format!(
                "extends: base.yml\nexamples:\n- {}\n- {}\n",
                example("c1"),
                example("d1")
            ),
        );
        let policy = Policy::load(&dir.path().join("easement.yml"), None).unwrap();
        let found: Vec<(String, &str, usize)> = policy
            .config
            .examples
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let (input, line) = policy.example_at(i).unwrap();
                (
                    e.to.clone().unwrap(),
                    input.path.file_name().unwrap().to_str().unwrap(),
                    line,
                )
            })
            .collect();
        let at = |to: &str, file, line| (to.to_string(), file, line);
        assert_eq!(
            found,
            [
                // A flow sequence writes its items on the line of its key.
                at("d1", "deep.yml", 2),
                at("d2", "deep.yml", 2),
                at("b1", "base.yml", 8),
                at("b2", "base.yml", 11),
                at("c1", "easement.yml", 3),
                // The same example twice is kept twice: each has its own line.
                at("d1", "easement.yml", 4),
            ]
        );
    }

    /// The policy at `easement.yml` in a new directory that holds `text`.
    fn load_text(text: &str) -> Result<Policy> {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "easement.yml", text);
        Policy::load(&dir.path().join("easement.yml"), None)
    }

    /// The error that loading `text` fails with, with its causes.
    fn load_error(text: &str) -> String {
        format!("{:#}", load_text(text).unwrap_err())
    }

    /// A policy that shares bans through the anchor `bans` and gives `web` the rule `rule`.
    fn with_bans(rule: &str) -> String {
        format!(
            "version: 1
components:
  web: {{ in: web }}
  api: {{ in: api }}
x-bans: &bans
  forbid_symbol: [a.A, b.B]
  because: shared
  max_fan_out: 3
rules:
  web: {rule}
"
        )
    }

    /// A rule of `web` that drops both shared bans.
    const DROPPING: &str = "{ <<: *bans, forbid_symbol: [extra.E] }";

    #[test]
    fn a_list_next_to_a_merge_that_drops_merged_items_is_refused() {
        let err = load_error(&with_bans(DROPPING));
        assert!(err.contains("easement.yml"), "{err}");
        assert!(
            err.contains("`rules.web.forbid_symbol` written next to `<<` drops a.A, b.B"),
            "{err}"
        );
        assert!(!err.contains("extra.E"), "{err}");
        // Under an `x-` key too, though easement drops that key once merged.
        let err = load_error(&format!(
            "{}x-web: {DROPPING}\n",
            with_bans("{ <<: *bans }")
        ));
        assert!(
            err.contains("`x-web.forbid_symbol` written next to `<<` drops a.A, b.B"),
            "{err}"
        );
        assert!(
            err.contains("do not merge `forbid_symbol` there: split the anchor"),
            "{err}"
        );
    }

    #[test]
    fn a_list_next_to_a_merge_that_keeps_every_merged_item_loads() {
        let policy = load_text(&with_bans(
            "{ <<: *bans, forbid_symbol: [a.A, extra.E, b.B] }",
        ))
        .unwrap();
        assert_eq!(policy.config.rules["web"].forbid_symbol.len(), 3);
        assert!(load_text(&with_bans("{ <<: *bans, forbid_symbol: [b.B, a.A] }")).is_ok());
        // An item written again with another reason is not the item merged.
        let err = load_error(&with_bans(
            "{ <<: *bans, forbid_symbol: [{ name: a.A, because: other }, b.B] }",
        ));
        assert!(err.contains("drops a.A that"), "{err}");
    }

    #[test]
    fn a_scalar_or_another_type_next_to_a_merge_is_not_a_dropped_item() {
        let policy = load_text(&with_bans("{ <<: *bans, because: own, max_fan_out: 1 }")).unwrap();
        assert_eq!(policy.config.reason("web"), Some("own"));
        assert_eq!(policy.config.rules["web"].max_fan_out, Some(1));
        // A scalar over a list is left to the schema.
        let err = load_error(&with_bans("{ <<: *bans, forbid_symbol: a.A }"));
        assert!(err.contains("rules.web.forbid_symbol"), "{err}");
        assert!(!err.contains("merge brings in"), "{err}");
    }

    /// `~`, or a key with no value, keeps nothing: over a merged list it would
    /// lift an allowlist as `may_use_external` reads none as no limit.
    #[test]
    fn a_null_next_to_a_merge_drops_every_merged_item() {
        let text = |rule: &str| {
            format!(
                "version: 1
components:
  web: {{ in: web }}
x-npm: &npm
  may_use_external: [lodash, react]
rules:
  web: {rule}
"
            )
        };
        for rule in [
            "{ <<: *npm, may_use_external: ~ }",
            "\n    <<: *npm\n    may_use_external:",
        ] {
            let err = load_error(&text(rule));
            assert!(
                err.contains(
                    "`rules.web.may_use_external` written next to `<<` drops lodash, react"
                ),
                "{err}"
            );
        }
        let err = load_error(
            "version: 1
x-web: &web
  in: web
  surfaces: { admin: { in: web/admin, importers: [api] } }
components:
  api: { in: api }
  web: { <<: *web, surfaces: ~ }
",
        );
        assert!(
            err.contains("`components.web.surfaces` written next to `<<` drops admin"),
            "{err}"
        );
    }

    #[test]
    fn of_several_merged_mappings_the_first_must_keep_what_the_others_bring() {
        let text = |b: &str| {
            format!(
                "version: 1
components:
  web: {{ in: web }}
x-a: &a
  forbid_external: [left-pad]
x-b: &b
  forbid_external: {b}
  because: b
rules:
  web: {{ <<: [*a, *b] }}
"
            )
        };
        let policy = load_text(&text("[left-pad]")).unwrap();
        assert_eq!(policy.config.reason("web"), Some("b"));
        let err = load_error(&text("[left-pad, moment]"));
        assert!(
            err.contains(
                "`rules.web.forbid_external` from the first merged mapping that has it drops moment"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_mapping_next_to_a_merge_that_drops_a_merged_key_is_refused() {
        let text = |surfaces: &str| {
            format!(
                "version: 1
x-web: &web
  in: web
  surfaces:
    admin: {{ in: web/admin, importers: [api] }}
    hooks: {{ in: web/hooks, importers: [api] }}
components:
  api: {{ in: api }}
  web: {{ <<: *web, surfaces: {surfaces} }}
"
            )
        };
        let err = load_error(&text("{ admin: { in: web/admin, importers: [api] } }"));
        assert!(
            err.contains("`components.web.surfaces` written next to `<<` drops hooks"),
            "{err}"
        );
        // At a deeper key, a list drops an item in the same way.
        let err = load_error(&text(
            "{ admin: { in: web/admin, importers: [] }, hooks: { in: web/hooks, importers: [api] } }",
        ));
        assert!(
            err.contains(
                "`components.web.surfaces.admin.importers` written next to `<<` drops api"
            ),
            "{err}"
        );
        // A scalar at a deeper key is an override.
        let policy = load_text(&text(
            "{ admin: { in: web/admin2, importers: [api] }, hooks: { in: web/hooks, importers: [api, web] } }",
        ))
        .unwrap();
        assert_eq!(policy.config.components["web"].surfaces.len(), 2);
    }

    /// A mapping written under `<<` itself, not through an alias, is checked
    /// where it is written.
    #[test]
    fn a_merge_inside_the_value_of_a_merge_is_checked_too() {
        let err = load_error(&with_bans(&format!("{{ <<: {DROPPING} }}")));
        assert!(
            err.contains("`rules.web.<<.forbid_symbol` written next to `<<` drops a.A, b.B"),
            "{err}"
        );
    }

    #[test]
    fn an_anchor_that_merges_another_brings_in_what_that_one_does() {
        let more = |rule: &str| {
            with_bans(rule).replace(
                "rules:\n",
                "x-more: &more\n  <<: *bans\n  because: more\nrules:\n",
            )
        };
        let policy = load_text(&more("{ <<: *more, max_fan_out: 1 }")).unwrap();
        let web = &policy.config.rules["web"];
        assert_eq!(web.forbid_symbol.len(), 2);
        assert_eq!(web.max_fan_out, Some(1));
        assert_eq!(policy.config.reason("web"), Some("more"));
        let policy = load_text(&with_bans("{ <<: { <<: *bans, because: own } }")).unwrap();
        assert_eq!(policy.config.rules["web"].forbid_symbol.len(), 2);
        assert_eq!(policy.config.reason("web"), Some("own"));
        // What the inner anchor brings is kept like what the outer one does.
        let err = load_error(&more("{ <<: *more, forbid_symbol: [a.A] }"));
        assert!(
            err.contains("`rules.web.forbid_symbol` written next to `<<` drops b.B"),
            "{err}"
        );
    }

    #[test]
    fn a_merge_that_drops_items_in_an_extended_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "base.yml", &with_bans(DROPPING));
        write(dir.path(), "easement.yml", "extends: base.yml\n");
        let err = format!(
            "{:#}",
            Policy::load(&dir.path().join("easement.yml"), None).unwrap_err()
        );
        assert!(err.contains("invalid config"), "{err}");
        assert!(err.contains("base.yml"), "{err}");
        assert!(err.contains("rules.web.forbid_symbol"), "{err}");
    }

    #[test]
    fn a_merge_that_drops_items_at_a_revision_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        write(repo, "easement.yml", &with_bans(DROPPING));
        git(repo, &["init", "-q"]);
        commit(repo, "drop the shared bans");
        write(repo, "easement.yml", &with_bans("{ <<: *bans }"));
        commit(repo, "keep them");

        let config = repo.join("easement.yml");
        let err = format!("{:#}", Policy::load(&config, Some("HEAD~1")).unwrap_err());
        assert!(err.contains("rules.web.forbid_symbol"), "{err}");
        assert!(Policy::load(&config, Some("HEAD")).is_ok());
        assert!(Policy::load(&config, None).is_ok());
    }

    #[test]
    fn written_line_finds_a_value_where_its_keys_write_it() {
        let text = r#"version: 1
rules:
  b: { may_depend_on: [a], forbid_symbol: [x.Y] }
components:
  a:
    in:
      - src/ab
      - src/a   # was src/x
  "c": { in: src/c }
  d: { in: [src/d/], part: web }
  b:
    # old home was src/old
    in: src/old
entry:
  - cmd/**
forbid_symbol:
  - { name: d.T, because: banned }
  - name: e.T
    allow_in:
      - src/ab
      - src/a
  - { name: f.T, allow_in: [src/a] }
"#;
        let line = |path: &[&str], value| written_line(text, path, value);
        assert_eq!(line(&["components", "b", "in"], "src/old"), 13);
        assert_eq!(line(&["components", "d", "in"], "src/d"), 10);
        assert_eq!(line(&["components", "a", "in"], "src/ab"), 7);
        assert_eq!(line(&["components", "a", "in"], "src/a"), 8);
        assert_eq!(line(&["components", "c", "in"], "src/c"), 9);
        assert_eq!(line(&["components", "d", "part"], "web"), 10);
        assert_eq!(line(&["rules", "b", "forbid_symbol"], "x.Y"), 3);
        assert_eq!(line(&["entry"], "cmd/**"), 15);
        assert_eq!(line(&["forbid_symbol"], "d.T"), 17);
        // An item of a list by its index: two items may write the same path.
        assert_eq!(line(&["forbid_symbol", "1", "allow_in"], "src/a"), 21);
        assert_eq!(line(&["forbid_symbol", "2", "allow_in"], "src/a"), 22);
        assert_eq!(line(&["forbid_symbol", "3", "allow_in"], "src/a"), 1);
        // What the text does not write there, such as a component from `extends`.
        assert_eq!(line(&["components", "legacy", "in"], "src/gone"), 1);
        assert_eq!(line(&["components", "a", "in"], "src/x"), 1);
        assert_eq!(written_line("not: [yaml", &["not"], "yaml"), 1);
    }

    #[test]
    fn extends_merges_by_name_and_detects_loops() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "base.yml",
            "version: 1
components:
  a: { in: a }
  b: { in: b }
common: [b]
exclude: [gen]
target_only: ['**/*.gen.ts']
tests: auto
rules:
  a: { may_depend_on: [b], because: base }
",
        );
        write(
            dir.path(),
            "easement.yml",
            "extends: base.yml
components:
  c: { in: c }
  b: { in: other/b }
common: [c, b]
exclude: [vendor]
target_only: ['**/*.pb.go']
rules:
  a: { because: own }
",
        );
        let policy = Policy::load(&dir.path().join("easement.yml"), None).unwrap();
        let c = &policy.config;
        assert_eq!(c.components.len(), 4);
        assert_eq!(c.components["b"].patterns.as_vec(), ["other/b"]);
        assert_eq!(c.common, ["b", "c"]);
        assert_eq!(c.exclude, ["gen", "vendor"]);
        assert_eq!(c.target_only, ["**/*.gen.ts", "**/*.pb.go"]);
        // The rule of the extending file replaces the base one as a whole.
        assert_eq!(c.reason("a"), Some("own"));
        assert!(c.rules["a"].may_depend_on.is_empty());
        let kinds: Vec<(InputKind, &str)> = policy
            .inputs
            .iter()
            .map(|i| (i.kind, i.path.file_name().unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(
            kinds,
            [
                (InputKind::Config, "easement.yml"),
                (InputKind::Extended, "base.yml")
            ]
        );
        let from = |key: &str, name: &str| {
            let input = policy.defined_by(key, name).unwrap();
            input
                .path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(from("components", "a"), "base.yml");
        assert_eq!(from("components", "b"), "easement.yml");
        assert_eq!(from("rules", "a"), "easement.yml");
        // `tests: auto` makes its component where it is set.
        assert_eq!(from("components", "tests"), "base.yml");
        // An item of a merged list is where it was first written, though the
        // extending file writes the key last.
        let common =
            |name: &str| policy.provenance(&Site::of_common(&policy.config, name).unwrap());
        assert_eq!(
            common("b"),
            Provenance {
                field: "common[0]".to_string(),
                input: Some("base.yml".to_string()),
                through: Vec::new(),
            }
        );
        assert_eq!(common("c").input.as_deref(), Some("easement.yml"));

        write(dir.path(), "loop.yml", "extends: loop.yml\nversion: 1\n");
        let err = Policy::load(&dir.path().join("loop.yml"), None).unwrap_err();
        assert!(err.to_string().contains("extends itself"));
    }

    /// `sets` merge by name as `components` do, and an unmatched glob of a set
    /// is found at its line in the file that writes it, or at line 1 when a file
    /// the configuration extends does.
    #[test]
    fn sets_merge_by_name_through_extends() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "base.yml",
            "version: 1\ncomponents:\n  api-a: { in: a }\n  api-b: { in: b }\n  cli: { in: c }\nsets:\n  drivers: [cli]\n  apis: ['api-*', 'gone-*']\n",
        );
        write(
            dir.path(),
            "easement.yml",
            "extends: base.yml\nsets:\n  drivers: [cli, 'api-*', 'web-*']\ncommon: ['@drivers', '@apis']\n",
        );
        let policy = Policy::load(&dir.path().join("easement.yml"), None).unwrap();
        assert_eq!(policy.config.common, ["cli", "api-a", "api-b"]);
        let lines: Vec<(&str, usize)> = policy
            .config
            .expansion
            .unmatched
            .iter()
            .map(|u| {
                let path: Vec<&str> = u.path.iter().map(String::as_str).collect();
                (u.item.as_str(), policy.config_line(&path, &u.item))
            })
            .collect();
        assert_eq!(lines, [("gone-*", 1), ("web-*", 3)]);
    }

    #[test]
    fn a_policy_at_a_revision_has_the_entries_of_its_extended_files_then() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let shared = tmp.path().join("shared");
        write(
            &shared,
            "outside.yml",
            "components:\n  far: { in: far, owner: outside }\n",
        );
        write(
            &repo,
            "policy/base.yml",
            "version: 1\ncomponents:\n  a: { in: a, owner: old }\n  b: { in: b }\nrules:\n  a: { may_depend_on: [b] }\n",
        );
        write(
            &repo,
            "easement.yml",
            "extends: [policy/base.yml, ../shared/outside.yml]\ncomponents:\n  c: { in: c }\n",
        );
        git(&repo, &["init", "-q"]);
        commit(&repo, "first");
        write(
            &repo,
            "policy/base.yml",
            "version: 1\ncomponents:\n  a: { in: a, owner: new }\n  b: { in: b }\nrules:\n  a: { because: alone }\n",
        );
        commit(&repo, "second");

        let config = repo.join("easement.yml");
        let old = Policy::load(&config, Some("HEAD~1")).unwrap();
        assert_eq!(old.config.components["a"].owner.as_deref(), Some("old"));
        assert_eq!(old.config.rules["a"].may_depend_on.len(), 1);
        let file = |input: &PolicyInput| {
            crate::relpath::slash(input.path.strip_prefix(real(tmp.path()).unwrap()).unwrap())
        };
        let defined = |key: &str, name: &str| file(old.defined_by(key, name).unwrap());
        assert_eq!(defined("components", "a"), "repo/policy/base.yml");
        assert_eq!(defined("rules", "a"), "repo/policy/base.yml");
        assert_eq!(defined("components", "c"), "repo/easement.yml");
        assert_eq!(defined("components", "far"), "shared/outside.yml");
        let past_unknown: Vec<(String, bool)> = old
            .inputs
            .iter()
            .map(|i| (file(i), i.past_unknown))
            .collect();
        assert_eq!(
            past_unknown,
            [
                ("repo/easement.yml".to_string(), false),
                ("repo/policy/base.yml".to_string(), false),
                ("shared/outside.yml".to_string(), true),
            ]
        );

        // Reached through a link, `..` still leaves the real directory.
        #[cfg(unix)]
        {
            std::fs::create_dir(tmp.path().join("elsewhere")).unwrap();
            let link = tmp.path().join("elsewhere/link");
            std::os::unix::fs::symlink(&repo, &link).unwrap();
            let linked = Policy::load(&link.join("easement.yml"), Some("HEAD~1")).unwrap();
            assert_eq!(linked.inputs, old.inputs);
        }

        let now = Policy::load(&config, None).unwrap();
        assert_eq!(now.config.components["a"].owner.as_deref(), Some("new"));
        assert_eq!(now.config.reason("a"), Some("alone"));
        assert!(now.inputs.iter().all(|i| !i.past_unknown));
    }

    #[test]
    fn a_policy_at_a_revision_is_judged_against_the_diagram_then() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        write(
            repo,
            "easement.yml",
            "version: 1\ncomponents:\n  web: { in: web }\n  db: { in: db }\ndiagram: docs/arch.md\n",
        );
        write(
            repo,
            "docs/arch.md",
            "```mermaid\nflowchart LR\n  web --> db\n```\n",
        );
        git(repo, &["init", "-q"]);
        commit(repo, "first");
        write(
            repo,
            "docs/arch.md",
            "```mermaid\nflowchart LR\n  web\n  db\n```\n",
        );
        commit(repo, "drop the arrow");

        let edges = [crate::check::Edge {
            from: "web".to_string(),
            to: "db".to_string(),
            file: "web/a.ts".to_string(),
            line: 1,
        }];
        let config = repo.join("easement.yml");
        let old = Policy::load(&config, Some("HEAD~1")).unwrap();
        assert!(crate::diagram::check(&old, &edges).unwrap().is_empty());
        let now = Policy::load(&config, None).unwrap();
        let violations = crate::diagram::check(&now, &edges).unwrap();
        assert_eq!(violations.len(), 1);
        assert!(
            violations[0]
                .message
                .starts_with("web -> db is not in the diagram")
        );
    }

    #[test]
    fn a_diagram_that_cannot_be_read_fails_only_where_it_is_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: a }\ndiagram: docs/arch.md\n",
        );
        let policy = Policy::load(&dir.path().join("easement.yml"), None).unwrap();
        let err = format!("{:#}", policy.diagram().unwrap_err());
        assert!(
            err.starts_with("cannot read the diagram docs/arch.md: "),
            "{err}"
        );
        assert_eq!(policy.inputs.len(), 1);
    }
}
