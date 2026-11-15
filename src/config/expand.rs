//! Globs over component names (`'ee-*'`) and named sets (`'@drivers'`) in the
//! lists of components and the keys of `rules`. They expand when the policy
//! loads, so every rule judges the names as if they were written out; where
//! each expanded name is written is kept, for provenance, the policy delta
//! and the patch.

use super::*;

/// How an item of a list of components is written.
enum Spelled<'a> {
    Component(&'a str),
    Glob(&'a str),
    Set(&'a str),
}

impl<'a> Spelled<'a> {
    fn of(text: &'a str) -> Self {
        if let Some(set) = text.strip_prefix('@') {
            Spelled::Set(set)
        } else if is_glob(text) {
            Spelled::Glob(text)
        } else {
            Spelled::Component(text)
        }
    }
}

/// Whether `name` is written as a glob over component names or a set, not as a component.
pub fn is_pattern(name: &str) -> bool {
    name.starts_with('@') || is_glob(name)
}

fn is_glob(name: &str) -> bool {
    name.contains(['*', '?', '[', '{'])
}

/// `key` as a field path writes it: quoted when it is a glob or a set (`rules.'api-*'`).
pub fn field_key(key: &str) -> String {
    if is_pattern(key) {
        format!("'{key}'")
    } else {
        key.to_string()
    }
}

/// What the globs and sets of a policy expanded to.
#[derive(Debug, Default, Clone)]
pub struct Expansion {
    /// By component, the key of `rules` that covers it, when that key is a glob or a set.
    rule_keys: BTreeMap<String, String>,
    /// By expanded list (`rules.web.may_depend_on`, `components.db.allow_importers`,
    /// `common`, `forbid_symbol[0].allow`, `components.db.surfaces.admin.importers`),
    /// where each of its names is written.
    lists: BTreeMap<String, WrittenNames>,
    /// The globs and sets that cover no component, and the sets nothing names.
    pub unmatched: Vec<Unmatched>,
}

/// Where a list writes an expanded name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The index, in the list as written, of the first item that names it.
    pub index: usize,
    /// The glob or set that first item is, `ee-*` or `@drivers`; none when it is the
    /// name itself.
    pub through: Option<String>,
    /// The first glob or set of the list that covers it, though an item may also
    /// name it: removing that item alone would not drop it.
    pub covered_by: Option<String>,
}

/// Where a list writes each of its names, by name.
type WrittenNames = BTreeMap<String, Written>;

/// A glob or a set that covers no component, or a set nothing names: it fails as
/// `unmatched` at its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmatched {
    /// The keys to what writes it: the list (`rules.web.may_depend_on`, an index of
    /// digits for an item of a list), or the mapping whose key it is (`rules`, `sets`).
    pub path: Vec<String>,
    /// The glob or the set as written, or the name of the set.
    pub item: String,
    /// Whether `item` is a key of the mapping at `path`, not an item of the list there.
    pub is_key: bool,
    pub message: String,
}

impl Config {
    /// The key of `rules` that writes the rule of `component`: its own name, or the
    /// glob or set that covers it.
    pub fn rule_key<'a>(&'a self, component: &'a str) -> &'a str {
        self.expansion
            .rule_keys
            .get(component)
            .map_or(component, String::as_str)
    }

    /// Where the expanded list `list` (`rules.web.may_depend_on`) writes `name`.
    pub fn written(&self, list: &str, name: &str) -> Option<&Written> {
        self.expansion.lists.get(list)?.get(name)
    }

    /// The config with every glob and set of its lists of components, and of the keys
    /// of `rules`, replaced by the components it covers.
    pub(super) fn expand(mut self) -> Result<Self> {
        let mut expander = Expander::new(&self)?;
        self.rules = expander.rules(std::mem::take(&mut self.rules))?;
        self.common = expander.names(&["common"], "common", std::mem::take(&mut self.common))?;
        self.layers = expander.layers(std::mem::take(&mut self.layers))?;
        for (name, component) in &mut self.components {
            if let Some(importers) = component.allow_importers.take() {
                let at = ["components", name.as_str(), "allow_importers"];
                component.allow_importers = Some(expander.names(&at, &at.join("."), importers)?);
            }
            for (surface, declared) in &mut component.surfaces {
                let at = [
                    "components",
                    name.as_str(),
                    "surfaces",
                    surface.as_str(),
                    "importers",
                ];
                declared.importers =
                    expander.names(&at, &at.join("."), std::mem::take(&mut declared.importers))?;
            }
        }
        for (i, listed) in self.forbid_symbol.iter_mut().enumerate() {
            if let ForbiddenName::Detailed(item) = listed
                && let Some(allow) = item.allow.take()
            {
                let index = i.to_string();
                let at = ["forbid_symbol", &index, "allow"];
                item.allow =
                    Some(expander.names(&at, &format!("forbid_symbol[{i}].allow"), allow)?);
            }
        }
        self.expansion = expander.finish();
        Ok(self)
    }
}

/// The state of one expansion: the components and sets of the config, which sets
/// something names, and what has expanded so far.
struct Expander {
    components: Vec<String>,
    /// The components each set covers, by name.
    sets: BTreeMap<String, Vec<String>>,
    named_sets: BTreeSet<String>,
    expansion: Expansion,
}

/// The item of a list that names one component, with the item it was written as.
struct Expanded<T> {
    item: T,
    name: String,
}

impl Expander {
    /// The expander of `config`, with each of its sets expanded: a set lists names and
    /// globs, never another set, and every name it lists is a component.
    fn new(config: &Config) -> Result<Self> {
        let mut expander = Self {
            components: config.components.keys().cloned().collect(),
            sets: BTreeMap::new(),
            named_sets: BTreeSet::new(),
            expansion: Expansion::default(),
        };
        for (set, items) in &config.sets {
            let place = format!("sets.{set}");
            if items.is_empty() {
                bail!("{place} lists no component; list some or drop the set");
            }
            let mut members: Vec<String> = Vec::new();
            for item in items {
                let found = match Spelled::of(item) {
                    Spelled::Set(_) => bail!(
                        "{place} lists `{item}`, another set: sets are flat, so list its components instead"
                    ),
                    Spelled::Component(name) => {
                        if !expander.components.iter().any(|c| c == name) {
                            return Err(unknown_component(
                                name,
                                &format!(" in {place}"),
                                expander.components.iter().map(String::as_str),
                            ));
                        }
                        vec![name.to_string()]
                    }
                    Spelled::Glob(glob) => expander.glob(glob, &["sets", set], &place)?,
                };
                for name in found {
                    if !members.contains(&name) {
                        members.push(name);
                    }
                }
            }
            expander.sets.insert(set.clone(), members);
        }
        Ok(expander)
    }

    /// The components `glob` matches, in the order of their names; one that matches
    /// none is unmatched at its item of the list at `path`, written in `place`.
    fn glob(&mut self, glob: &str, path: &[&str], place: &str) -> Result<Vec<String>> {
        let matcher = globset::Glob::new(glob)
            .with_context(|| format!("bad glob `{glob}` in {place}"))?
            .compile_matcher();
        let found: Vec<String> = self
            .components
            .iter()
            .filter(|c| matcher.is_match(c.as_str()))
            .cloned()
            .collect();
        if found.is_empty() {
            self.unmatched(
                path,
                glob,
                false,
                format!("`{glob}` in {place} matches no component; fix the glob or drop it"),
            );
        }
        Ok(found)
    }

    fn unmatched(&mut self, path: &[&str], item: &str, is_key: bool, message: String) {
        let unmatched = Unmatched {
            path: path.iter().map(|k| k.to_string()).collect(),
            item: item.to_string(),
            is_key,
            message,
        };
        // A list under a rule key that covers several components expands once for each.
        if !self.expansion.unmatched.contains(&unmatched) {
            self.expansion.unmatched.push(unmatched);
        }
    }

    /// The components `item`, written in the list at `path` and called `place` in
    /// errors, names: itself, the ones a glob matches, or the members of a set.
    fn members(&mut self, item: &str, path: &[&str], place: &str) -> Result<Vec<String>> {
        match Spelled::of(item) {
            // `validate` names one that is no component.
            Spelled::Component(name) => Ok(vec![name.to_string()]),
            Spelled::Glob(glob) => self.glob(glob, path, place),
            Spelled::Set(set) => {
                let Some(members) = self.sets.get(set) else {
                    let guess = closest(set, self.sets.keys().map(String::as_str))
                        .map(|s| format!("; did you mean `@{s}`?"))
                        .unwrap_or_default();
                    bail!("unknown set `{item}` in {place}{guess}");
                };
                self.named_sets.insert(set.to_string());
                Ok(members.clone())
            }
        }
    }

    /// `items` with each glob or set replaced by an item for every component it
    /// covers, made by `rebuild` of the item and the name; a name written twice is
    /// kept where it is first written. Also where each name is written.
    fn list<T>(
        &mut self,
        path: &[&str],
        place: &str,
        items: Vec<T>,
        name: impl Fn(&T) -> &str,
        rebuild: impl Fn(&T, String) -> T,
    ) -> Result<(Vec<T>, WrittenNames)> {
        let mut expanded: Vec<Expanded<T>> = Vec::new();
        let mut written = WrittenNames::new();
        for (index, item) in items.iter().enumerate() {
            let text = name(item);
            let through = is_pattern(text).then(|| text.to_string());
            for member in self.members(text, path, place)? {
                let entry = written.entry(member.clone()).or_insert_with(|| Written {
                    index,
                    through: through.clone(),
                    covered_by: None,
                });
                if entry.covered_by.is_none() {
                    entry.covered_by = through.clone();
                }
                if !expanded.iter().any(|e| e.name == member) {
                    expanded.push(Expanded {
                        item: rebuild(item, member.clone()),
                        name: member,
                    });
                }
            }
        }
        Ok((expanded.into_iter().map(|e| e.item).collect(), written))
    }

    /// A list of plain names, as `common` and `allow_importers` are, keeping where
    /// each is written under `list`.
    fn names(&mut self, path: &[&str], list: &str, items: Vec<String>) -> Result<Vec<String>> {
        let (names, written) = self.list(path, list, items, String::as_str, |_, m| m)?;
        self.expansion.lists.insert(list.to_string(), written);
        Ok(names)
    }

    /// The rules by component: a key that is a glob or a set gives its rule to each
    /// component it covers, which no other key may cover too.
    fn rules(&mut self, rules: BTreeMap<String, Rule>) -> Result<BTreeMap<String, Rule>> {
        let mut by_component: BTreeMap<String, (String, Rule)> = BTreeMap::new();
        for (key, rule) in rules {
            let place = format!("rules.{}", field_key(&key));
            let members = match Spelled::of(&key) {
                Spelled::Component(_) => vec![key.clone()],
                Spelled::Glob(glob) => {
                    let found = self.glob_quietly(glob, &place)?;
                    if found.is_empty() {
                        self.unmatched(
                            &["rules"],
                            &key,
                            true,
                            format!(
                                "the rule key `{key}` matches no component, so its rule judges nothing; fix the glob or drop the rule"
                            ),
                        );
                    }
                    found
                }
                Spelled::Set(_) => self.members(&key, &["rules"], "the keys of rules")?,
            };
            let rule = self.rule(&key, rule)?;
            for member in members {
                if let Some((other, _)) = by_component.get(&member) {
                    bail!(
                        "component `{member}` has two rules, `rules.{}` and `rules.{}`: a component has one rule, so narrow a glob or a set, or merge the two",
                        field_key(other),
                        field_key(&key)
                    );
                }
                if member != key {
                    self.expansion.rule_keys.insert(member.clone(), key.clone());
                }
                for field in ["may_depend_on", "must_not_depend_on", "must_not_reach"] {
                    if let Some(written) = rule.1.get(field) {
                        self.expansion
                            .lists
                            .insert(format!("rules.{member}.{field}"), written.clone());
                    }
                }
                by_component.insert(member, (key.clone(), rule.0.clone()));
            }
        }
        Ok(by_component
            .into_iter()
            .map(|(member, (_, rule))| (member, rule))
            .collect())
    }

    /// The components `glob` matches, leaving to the caller what to say when none does.
    fn glob_quietly(&mut self, glob: &str, place: &str) -> Result<Vec<String>> {
        let matcher = globset::Glob::new(glob)
            .with_context(|| format!("bad glob `{glob}` in {place}"))?
            .compile_matcher();
        Ok(self
            .components
            .iter()
            .filter(|c| matcher.is_match(c.as_str()))
            .cloned()
            .collect())
    }

    /// The rule under `key` with its lists of components expanded, and where each list
    /// writes each name.
    fn rule(
        &mut self,
        key: &str,
        mut rule: Rule,
    ) -> Result<(Rule, BTreeMap<&'static str, WrittenNames>)> {
        let mut written = BTreeMap::new();
        for (field, list) in [
            ("may_depend_on", &mut rule.may_depend_on),
            ("must_not_depend_on", &mut rule.must_not_depend_on),
            ("must_not_reach", &mut rule.must_not_reach),
        ] {
            let path = ["rules", key, field];
            let place = format!("rules.{}.{field}", field_key(key));
            let (items, names) = self.list(
                &path,
                &place,
                std::mem::take(list),
                Listed::name,
                |item, m| match item {
                    Listed::Plain(_) => Listed::Plain(m),
                    Listed::Reasoned(Reasoned { because, .. }) => Listed::Reasoned(Reasoned {
                        name: m,
                        because: because.clone(),
                    }),
                },
            )?;
            *list = items;
            written.insert(field, names);
        }
        Ok((rule, written))
    }

    /// The layers with each glob or set replaced by the components it covers: a row
    /// that covers several holds siblings, one that covers none is dropped. A closed
    /// layer is one component, so its name covers exactly one.
    fn layers(&mut self, layers: Vec<Layer>) -> Result<Vec<Layer>> {
        let mut expanded = Vec::new();
        for layer in layers {
            let closed = layer.closed();
            let written: Vec<String> = layer.names().into_iter().map(str::to_string).collect();
            let (mut names, _) = self.list(
                &["layers"],
                "layers",
                written.clone(),
                String::as_str,
                |_, m| m,
            )?;
            if closed && names.len() > 1 {
                bail!(
                    "layers: the closed layer `{}` covers {}, but a closed layer is one component; name it alone",
                    written.join(", "),
                    names.join(", ")
                );
            }
            expanded.push(match (names.len(), closed) {
                (0, _) => continue,
                (1, true) => Layer::Named(NamedLayer {
                    name: names.remove(0),
                    closed: true,
                }),
                (1, false) => Layer::One(names.remove(0)),
                _ => Layer::Siblings(names),
            });
        }
        Ok(expanded)
    }

    /// The expansion, with every set that nothing names unmatched.
    fn finish(mut self) -> Expansion {
        let unnamed: Vec<String> = self
            .sets
            .keys()
            .filter(|set| !self.named_sets.contains(*set))
            .cloned()
            .collect();
        for set in unnamed {
            self.unmatched(
                &["sets"],
                &set,
                true,
                format!(
                    "the set `{set}` is named by no list or rule key, so it changes nothing; use `@{set}` or drop the set"
                ),
            );
        }
        self.expansion
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Result<Config> {
        serde_norway::from_str::<Config>(yaml)?.checked()
    }

    const COMPONENTS: &str = "version: 1
components:
  api-site: { in: a }
  api-external: { in: b }
  runner: { in: c }
  usecase: { in: d }
  wiring: { in: e }
  ee-licensekey: { in: f }
  ee-sso: { in: g }
";

    fn with(rest: &str) -> Result<Config> {
        parse(&format!("{COMPONENTS}{rest}"))
    }

    fn names(listed: &[Listed]) -> Vec<&str> {
        listed.iter().map(Listed::name).collect()
    }

    #[test]
    fn globs_and_sets_expand_to_the_components_they_cover() {
        let config = with(
            "sets:
  drivers: ['api-*', runner]
common: ['ee-*']
rules:
  usecase:
    may_depend_on: [wiring, '@drivers', runner]
    must_not_depend_on: [{ name: 'ee-*', because: no enterprise code }]
  'api-*':
    may_depend_on: [usecase]
",
        )
        .unwrap();
        assert_eq!(config.common, ["ee-licensekey", "ee-sso"]);
        let usecase = &config.rules["usecase"];
        assert_eq!(
            names(&usecase.may_depend_on),
            ["wiring", "api-external", "api-site", "runner"]
        );
        assert_eq!(
            usecase
                .must_not_depend_on
                .iter()
                .map(|l| (l.name(), l.because()))
                .collect::<Vec<_>>(),
            [
                ("ee-licensekey", Some("no enterprise code")),
                ("ee-sso", Some("no enterprise code"))
            ]
        );
        for api in ["api-site", "api-external"] {
            assert_eq!(names(&config.rules[api].may_depend_on), ["usecase"]);
            assert_eq!(config.rule_key(api), "api-*");
        }
        assert!(!config.rules.contains_key("api-*"));
        assert_eq!(config.rule_key("usecase"), "usecase");
        // Where each name is written: the first item that names it.
        let at = |name| config.written("rules.usecase.may_depend_on", name).unwrap();
        assert_eq!(
            at("runner"),
            &Written {
                index: 1,
                through: Some("@drivers".to_string()),
                covered_by: Some("@drivers".to_string()),
            }
        );
        assert_eq!(at("wiring").through, None);
        assert_eq!(at("wiring").covered_by, None);
        assert_eq!(
            config
                .written("rules.api-site.may_depend_on", "usecase")
                .map(|w| w.index),
            Some(0)
        );
        assert!(config.expansion.unmatched.is_empty());
    }

    #[test]
    fn importers_allow_and_layers_take_globs_and_sets() {
        let config = with(
            "sets:
  drivers: ['api-*', runner]
layers: ['@drivers', usecase, ['ee-*', wiring]]
forbid_symbol: [{ name: lic.Key, allow: ['ee-*'] }]
",
        )
        .unwrap();
        let rows: Vec<Vec<&str>> = config.layers.iter().map(Layer::names).collect();
        assert_eq!(
            rows,
            [
                vec!["api-external", "api-site", "runner"],
                vec!["usecase"],
                vec!["ee-licensekey", "ee-sso", "wiring"]
            ]
        );
        assert!(config.grant("runner", "usecase").is_some());
        assert!(config.grant("api-site", "runner").is_none());
        assert!(config.forbid_symbol[0].allows("ee-sso"));
        let config = parse(
            "version: 1
components:
  ee-a: { in: a, allow_importers: [wiring, 'ee-*'], surfaces: { admin: { in: a/admin, importers: ['ee-*'] } } }
  ee-b: { in: b }
  wiring: { in: w }
",
        )
        .unwrap();
        let ee = &config.components["ee-a"];
        assert_eq!(
            ee.allow_importers.as_deref().unwrap(),
            ["wiring", "ee-a", "ee-b"]
        );
        assert_eq!(ee.surfaces["admin"].importers, ["ee-a", "ee-b"]);
        assert_eq!(
            config
                .written("components.ee-a.allow_importers", "ee-b")
                .unwrap()
                .through
                .as_deref(),
            Some("ee-*")
        );
    }

    #[test]
    fn a_component_two_rule_keys_cover_fails_naming_both() {
        for (rules, said) in [
            (
                "'api-*': {}\n  api-site: {}\n",
                "component `api-site` has two rules, `rules.'api-*'` and `rules.api-site`",
            ),
            (
                "'api-*': {}\n  '*-site': {}\n",
                "component `api-site` has two rules, `rules.'*-site'` and `rules.'api-*'`",
            ),
            (
                "'@drivers': {}\n  runner: {}\n",
                "component `runner` has two rules, `rules.'@drivers'` and `rules.runner`",
            ),
        ] {
            let err = with(&format!(
                "sets:\n  drivers: ['api-*', runner]\nrules:\n  {rules}"
            ))
            .unwrap_err();
            assert!(err.to_string().contains(said), "{err}");
        }
    }

    #[test]
    fn what_covers_nothing_is_kept_to_fail_as_unmatched() {
        let config = with(
            "sets:
  drivers: ['cli-*', runner]
  idle: [runner]
common: ['web-*']
rules:
  'job-*': { may_depend_on: [usecase] }
  usecase: { may_depend_on: ['@drivers', 'x-*'] }
",
        )
        .unwrap();
        let found: Vec<(String, &str, bool)> = config
            .expansion
            .unmatched
            .iter()
            .map(|u| (u.path.join("."), u.item.as_str(), u.is_key))
            .collect();
        assert_eq!(
            found,
            [
                ("sets.drivers".to_string(), "cli-*", false),
                ("rules".to_string(), "job-*", true),
                ("rules.usecase.may_depend_on".to_string(), "x-*", false),
                ("common".to_string(), "web-*", false),
                ("sets".to_string(), "idle", true),
            ]
        );
        assert_eq!(names(&config.rules["usecase"].may_depend_on), ["runner"]);
        assert!(config.common.is_empty());
    }

    #[test]
    fn sets_are_flat_and_name_what_exists() {
        for (rest, said) in [
            (
                "sets:\n  a: [runner]\n  b: ['@a']\ncommon: ['@b']\n",
                "sets.b lists `@a`, another set: sets are flat",
            ),
            (
                "sets:\n  a: [runer]\ncommon: ['@a']\n",
                "unknown component `runer` in sets.a; did you mean `runner`?",
            ),
            (
                "sets:\n  drivers: [runner]\ncommon: ['@driver']\n",
                "unknown set `@driver` in common; did you mean `@drivers`?",
            ),
            ("sets:\n  a: []\n", "sets.a lists no component"),
            (
                "rules:\n  usecase: { must_depend_on: ['ee-*'] }\n",
                "`ee-*` in rules.usecase.must_depend_on: a glob or a set of components works only in",
            ),
            (
                "layers: [{ name: 'ee-*', closed: true }]\n",
                "the closed layer `ee-*` covers ee-licensekey, ee-sso",
            ),
            ("common: ['ee-[']\n", "bad glob `ee-[` in common"),
        ] {
            let err = format!("{:#}", with(rest).unwrap_err());
            assert!(err.contains(said), "{rest}: {err}");
        }
        let err = parse("version: 1\ncomponents:\n  'a*': { in: a }\n").unwrap_err();
        assert!(
            err.to_string().contains("reads as a glob or a set"),
            "{err}"
        );
    }

    /// A list that lets no one in is judged as written: a glob that matches nothing
    /// leaves it empty, which `unmatched` reports instead.
    #[test]
    fn a_list_judged_as_written_may_expand_to_nothing() {
        let config = with(
            "forbid_symbol: [{ name: lic.Key, allow: ['x-*'] }]
layers: [['x-*', 'y-*']]
",
        )
        .unwrap();
        assert_eq!(config.forbid_symbol[0].allow(), Some(&[][..]));
        assert!(config.layers.is_empty());
        let config = parse(
            "version: 1
components:
  a: { in: a, surfaces: { admin: { in: a/admin, importers: ['x-*'] } } }
",
        )
        .unwrap();
        assert!(
            config.components["a"].surfaces["admin"]
                .importers
                .is_empty()
        );
        assert_eq!(config.expansion.unmatched.len(), 1);
    }
}
