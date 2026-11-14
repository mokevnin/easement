use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::check::SymbolRef;
use crate::config::ScanPolicy;
use crate::indexer;
use crate::lang::{self, Dep, Import, Target, TypeDef};
use crate::relpath::{glob_set, slash, walker};
use crate::scip;
use crate::suppress::{self, Suppression};

/// What was read from a file, valid while its size and modification time stay the same.
struct Parsed {
    modified: Option<SystemTime>,
    len: u64,
    imports: Vec<Import>,
    suppressions: Vec<Suppression>,
}

/// Parsed files kept between scans, so watch mode re-reads only what changed,
/// and how the indexes are obtained. Resolution is never cached: it depends on
/// the other files.
pub struct Cache {
    parsed: HashMap<PathBuf, Parsed>,
    pub indexing: indexer::Indexing,
}

impl Cache {
    /// A cache that obtains the indexes as `indexing` says.
    pub fn new(indexing: indexer::Indexing) -> Self {
        Self {
            parsed: HashMap::new(),
            indexing,
        }
    }
}

pub struct Scan {
    /// Every file that is not excluded, source or not, relative to the root with `/` separators.
    pub all_files: Vec<String>,
    /// Every checked file, relative to the root with `/` separators.
    pub paths: Vec<String>,
    pub deps: Vec<Dep>,
    /// Imports in files the checks do not read (`target_only`, excluded):
    /// not judged, but they still reach a file.
    pub unread_deps: Vec<Dep>,
    /// The `target_only` and excluded files the walk saw.
    pub unread: Vec<String>,
    pub symbols: Vec<SymbolRef>,
    /// Uses of names in files of a language whose uses are not verified: no rule
    /// judges them, and a symbol rule over them is an evidence gap.
    pub unverified_symbols: Vec<SymbolRef>,
    /// The types checked files define, from the index.
    pub types: Vec<TypeDef>,
    pub suppressions: Vec<Suppression>,
    /// Source files of an indexed language that the indexer did not cover,
    /// for example files outside every `tsconfig.json`.
    pub uncovered: Vec<String>,
    /// Checked files that use a name of a package the indexer left unnamed,
    /// which no rule on names judged.
    pub unnamed_uses: Vec<String>,
    /// Imports that checked files of the index write that the index took to no
    /// source target and whose text names none either: their dependencies are
    /// missing (scip-go on an import cycle, a package that does not type-check, a
    /// TypeScript import a wildcard declaration catches).
    pub unresolved_imports: Vec<Unresolved>,
    /// Files whose test code a finder of parts could not tell from the file's.
    pub parts_doubts: Vec<(String, lang::parts::Doubt)>,
    /// Manifests, or dependencies of one, that `packages` could not judge as declared.
    pub packages_problems: Vec<crate::check::Unjudged>,
    /// Names declared in more than one file, so an import of one could not be
    /// told for certain.
    pub declarations_problems: Vec<crate::check::Unjudged>,
    /// The qualifier and name of the symbols the index knows under a name that
    /// `forbid_symbol` lists.
    pub known_symbols: Vec<(String, String)>,
    /// Whether `known_symbols` is whole: false when the index left some unnamed.
    pub known_symbols_whole: bool,
    /// Indexers that ran.
    pub indexed: usize,
    /// Indexes taken from the cache instead of running their indexer.
    pub from_cache: usize,
    /// With `reach: symbols`, the references of the index by the definition that holds
    /// them.
    pub flow: Option<crate::flow::Flow>,
}

impl Scan {
    /// Whether the index holds all of the project: every source file covered by an
    /// indexer, every import it resolved and every name it saw named. Only then can a name it lacks be told
    /// to be missing.
    pub fn indexed_whole(&self) -> bool {
        self.uncovered.is_empty()
            && self.unresolved_imports.is_empty()
            && self.unnamed_uses.is_empty()
            && self.known_symbols_whole
            && self.paths.iter().all(|f| indexer::indexed(f))
    }
}

/// An import a file writes whose dependency is not in the graph, and why.
#[derive(Debug, Clone)]
pub struct Unresolved {
    /// The file that writes it.
    pub file: String,
    pub import: Import,
    pub cause: lang::Cause,
    /// What the language of the file says of the cause; `None` when it says nothing.
    pub explained: Option<&'static lang::registry::Explained>,
}

impl Unresolved {
    /// Where it is written, with its specifier.
    pub fn place(&self) -> crate::check::Place {
        crate::check::Place {
            file: self.file.clone(),
            line: self.import.line,
            specifier: Some(self.import.specifier.clone()),
        }
    }
}

/// What the comparison of an import with the index found.
enum Compared {
    /// The index takes it to a source target: its dependency is the index's.
    Resolved,
    /// The index does not, but its text names a file of the project or a package.
    Spelled(Box<Dep>),
    /// Neither does its text: its dependency is not in the graph.
    Unresolved(Unresolved),
}

/// `import`, written in `file`, against what the index holds on it (`resolved`).
/// When the index takes it to no source target, a path its text spells that names
/// a file that `exists` exactly, or a package it spells, is its dependency; else it
/// is unresolved, for the cause its resolution and its form tell.
fn compare(
    file: &str,
    import: Import,
    written: &lang::registry::WrittenImports,
    resolved: impl Fn(&Import) -> scip::Resolution,
    exists: impl Fn(&str) -> bool,
) -> Compared {
    use lang::registry::Spelled;
    use lang::{Cause, Form};
    // A target computed at run time is no one file, whatever the index holds.
    let unspelled = match import.form {
        Form::Computed => Cause::Computed,
        form => {
            let resolution = resolved(&import);
            let target = match written.spelled.map(|spelled| spelled(&import.specifier)) {
                _ if resolution == scip::Resolution::Target => return Compared::Resolved,
                Some(Spelled::Path(path)) => {
                    crate::relpath::resolve(crate::relpath::parent(file), &path)
                        .filter(|path| exists(path))
                        .map(|path| Target::Internal(lang::Unit::from(path.as_str())))
                }
                Some(Spelled::Package(package)) => Some(Target::External(package)),
                Some(Spelled::Neither) | None => None,
            };
            if let Some(target) = target {
                let specifier = target.name();
                return Compared::Spelled(Box::new(Dep::module(
                    file,
                    import.line,
                    specifier,
                    target,
                )));
            }
            match (resolution, form) {
                (scip::Resolution::Wildcard, _) => Cause::Wildcard,
                (_, Form::SideEffect) => Cause::SideEffect,
                (_, Form::Require) => Cause::Require,
                _ => Cause::NoTarget,
            }
        }
    };
    Compared::Unresolved(Unresolved {
        file: file.to_string(),
        import,
        cause: unspelled,
        explained: written
            .explained
            .iter()
            .find(|explained| explained.cause == unspelled),
    })
}

/// The project's files (`/`-separated, relative to `root`) without the excluded ones.
pub fn project_files(root: &Path, policy: &ScanPolicy) -> Result<Vec<String>> {
    let exclude = glob_set(&policy.exclude, "exclude")?;
    let mut files = Vec::new();
    for entry in walker(root).build() {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Some(rel) = lang::relative(root, entry.path())
            && !exclude.is_match(&rel)
        {
            files.push(slash(&rel));
        }
    }
    Ok(files)
}

/// Reads the project as `policy`, the scan-relevant part of the policy, asks.
pub fn scan(root: &Path, policy: &ScanPolicy, cache: &mut Cache) -> Result<Scan> {
    let exclude = glob_set(&policy.exclude, "exclude")?;
    let target_only = glob_set(&policy.target_only, "target_only")?;
    // One walk serves the adapters and the files: an adapter resolves to anything
    // the walk sees but a directory, the excluded files and the links included; only
    // a file is read as a source.
    let mut walked = Vec::new();
    let mut targets = Vec::new();
    for entry in walker(root).build() {
        let entry = entry?;
        if entry.file_type().is_none_or(|t| t.is_dir()) {
            continue;
        }
        let Some(rel) = lang::relative(root, entry.path()) else {
            continue;
        };
        targets.push(rel.clone());
        if entry.file_type().is_some_and(|t| t.is_file()) {
            walked.push((entry, rel));
        }
    }
    let adapters = lang::adapters(root, &targets)?;
    let mut paths = Vec::new();
    let mut all_files = Vec::new();
    // Excluded files the walk sees (not the ones .gitignore hides).
    let mut excluded = BTreeSet::new();
    let mut deps = Vec::new();
    let mut ambiguities: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> =
        std::collections::BTreeMap::new();
    let mut suppressions = Vec::new();
    let mut seen = HashSet::new();
    for (entry, rel) in walked {
        let path = entry.path();
        let file = slash(&rel);
        if exclude.is_match(&rel) {
            excluded.insert(file);
            continue;
        }
        all_files.push(file.clone());
        if indexer::indexed(&file) {
            continue; // read from the index below
        }
        if target_only.is_match(&rel) {
            continue; // a target of imports, never read as a source
        }
        let Some(adapter) = adapters.iter().find(|a| a.handles(path)) else {
            continue;
        };
        let meta = entry.metadata()?;
        let modified = meta.modified().ok();
        let fresh = cache
            .parsed
            .get(&rel)
            .is_some_and(|p| p.len == meta.len() && p.modified == modified && modified.is_some());
        if !fresh {
            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue, // binary or unreadable
            };
            let parsed = Parsed {
                modified,
                len: meta.len(),
                imports: adapter.imports(path, &source).with_context(|| {
                    format!("{} adapter failed on {}", adapter.name(), rel.display())
                })?,
                suppressions: suppress::parse(&file, &source),
            };
            cache.parsed.insert(rel.clone(), parsed);
        }
        let parsed = &cache.parsed[&rel];
        seen.insert(rel.clone());
        paths.push(file.clone());
        suppressions.extend(parsed.suppressions.iter().cloned());
        for import in &parsed.imports {
            let lang::Resolution {
                target,
                candidates,
                ambiguity,
            } = adapter.resolve(path, &import.specifier);
            let mut dep = Dep::module(rel.clone(), import.line, import.specifier.clone(), target);
            if let Some(name) = ambiguity {
                let entry = ambiguities
                    .entry(name)
                    .or_insert_with(|| (candidates.clone(), Vec::new()));
                entry.1.push(file.clone());
            }
            dep.candidates = candidates;
            deps.push(dep);
        }
    }
    cache.parsed.retain(|path, _| seen.contains(path));

    // Sorted, as the set is.
    let excluded: Vec<String> = excluded.into_iter().collect();
    let indexed = indexer::run(
        root,
        &all_files,
        &excluded,
        &cache.indexing,
        &policy.indexers,
    )?;
    let project: HashSet<&str> = all_files.iter().map(String::as_str).collect();
    // Excluded files are never read as sources, but what they define stays
    // resolvable, so an import of one is seen and fails as `excluded`.
    let (mut documents, hidden): (Vec<scip::Document>, Vec<scip::Document>) = indexed
        .documents
        .0
        .into_iter()
        .filter(|d| project.contains(d.path.as_str()) || excluded.binary_search(&d.path).is_ok())
        .partition(|d| project.contains(d.path.as_str()));
    let mut parts_doubts: Vec<(String, lang::parts::Doubt)> = Vec::new();
    // (the file that declares them, the files a test-only module may be in)
    let mut declared_files: Vec<(String, Vec<String>)> = Vec::new();
    // The other modules each file declares, test code too if the file turns out to be.
    let mut modules: HashMap<String, Vec<lang::parts::Module>> = HashMap::new();
    for document in &mut documents {
        let Some(find) = lang::registry::of(&document.path).and_then(|language| language.parts)
        else {
            continue;
        };
        match std::fs::read_to_string(root.join(&document.path)) {
            Ok(source) => {
                let found = find(&document.path, &source);
                document.parts = found.spans;
                parts_doubts.extend(found.doubts.into_iter().map(|d| (document.path.clone(), d)));
                declared_files.extend(found.files.into_iter().map(|f| (document.path.clone(), f)));
                modules.insert(document.path.clone(), found.modules);
            }
            Err(_) => parts_doubts.push((document.path.clone(), lang::parts::Doubt::Unreadable)),
        }
    }
    // A module that only a test build has is test code as a whole, in its own file, and
    // so is every module that file declares, all the way down.
    let mut pending = declared_files;
    let mut test_files: HashSet<String> = HashSet::new();
    while let Some((declaring, candidates)) = pending.pop() {
        let Some(file) = candidates
            .into_iter()
            .find(|c| project.contains(c.as_str()))
        else {
            parts_doubts.push((declaring, lang::parts::Doubt::ModuleInAnotherFile));
            continue;
        };
        if !test_files.insert(file.clone()) {
            continue;
        }
        if let Some(document) = documents.iter_mut().find(|d| d.path == file) {
            document.parts = vec![lang::parts::Span::test(1..=usize::MAX)];
        }
        for module in modules.remove(&file).unwrap_or_default() {
            match module {
                lang::parts::Module::Files(candidates) => pending.push((file.clone(), candidates)),
                lang::parts::Module::Unplaced(doubt) => parts_doubts.push((file.clone(), doubt)),
            }
        }
    }
    // Every pass over the index reads each symbol once and shares who defines what.
    let names = scip::Names::of(&documents, &hidden);
    let index = scip::Indexed::of(&names);
    let mut types: Vec<TypeDef> = Vec::new();
    let mut unresolved_imports = Vec::new();
    // Dependencies read from the text of imports the index took to no source target.
    let mut spelled_deps: Vec<Dep> = Vec::new();
    // A file the text of an import may name: one of the project, excluded or not.
    let exists = |file: &str| {
        project.contains(file) || excluded.binary_search_by(|f| f.as_str().cmp(file)).is_ok()
    };
    for document in documents
        .iter()
        .filter(|d| !target_only.is_match(d.path.as_str()))
    {
        paths.push(document.path.clone());
        if let Ok(source) = std::fs::read_to_string(root.join(&document.path)) {
            suppressions.extend(suppress::parse(&document.path, &source));
            if let Some(written) = lang::registry::of(&document.path)
                .and_then(|language| language.written_imports.as_ref())
                // A declaration file describes code that lives elsewhere.
                .filter(|_| !lang::registry::is_declaration(&document.path))
            {
                let lines: Vec<&str> = source.lines().collect();
                for import in (written.list)(&document.path, &source) {
                    let resolved = |import: &Import| index.resolution(document, &lines, import);
                    match compare(&document.path, import, written, resolved, exists) {
                        Compared::Resolved => {}
                        Compared::Spelled(dep) => spelled_deps.push(*dep),
                        Compared::Unresolved(unresolved) => unresolved_imports.push(unresolved),
                    }
                }
            }
        }
        types.extend(document.types.iter().map(|(name, line)| TypeDef {
            file: document.path.clone(),
            line: *line,
            name: name.clone(),
        }));
    }
    let flow = (policy.reach == crate::config::Reach::Symbols).then(|| index.flow());
    let facts = index.facts(policy.uses_symbols, &policy.watched_symbols);
    let is_source = |file: &Path| {
        let file = slash(file);
        project.contains(file.as_str()) && !target_only.is_match(&file)
    };
    let (sources, mut unread_deps): (Vec<Dep>, Vec<Dep>) =
        facts.deps.into_iter().partition(|dep| is_source(&dep.file));
    deps.extend(sources);
    // One dependency of a file on a target: the index's, or else the first line that
    // spells it.
    let mut known: HashSet<(PathBuf, Target)> = deps
        .iter()
        .map(|dep| (dep.file.clone(), dep.target.clone()))
        .collect();
    spelled_deps.sort_by_key(|dep| dep.line);
    deps.extend(
        spelled_deps
            .into_iter()
            .filter(|dep| known.insert((dep.file.clone(), dep.target.clone()))),
    );
    let mut packages_problems = Vec::new();
    if policy.packages {
        let declared =
            crate::packages::declared_by(root, &all_files, |file| !target_only.is_match(file));
        paths.extend(declared.manifests);
        deps.extend(declared.deps);
        packages_problems = declared.problems;
    }
    // What excluded files import: reading them is a pass over every definition, for nothing
    // when nothing is excluded.
    if !hidden.is_empty() {
        unread_deps.extend(index.excluded_facts().deps);
    }
    let mut unread = excluded;
    unread.extend(
        all_files
            .iter()
            .filter(|f| target_only.is_match(f.as_str()))
            .cloned(),
    );
    unread.sort();
    let unnamed_uses = facts
        .unnamed
        .iter()
        .filter(|file| is_source(Path::new(file.as_str())))
        .cloned()
        .collect();
    // Uses of names count only for a language whose uses are verified; a rule
    // over the others is an evidence gap (`check::symbols_unverified`).
    let (symbols, unverified_symbols): (Vec<SymbolRef>, Vec<SymbolRef>) = facts
        .symbols
        .into_iter()
        .filter(|symbol| is_source(&symbol.file))
        // A use of a name an excluded file defines adds nothing: its import
        // already fails as `excluded`.
        .filter(|symbol| {
            !matches!(&symbol.target, Some(Target::Internal(unit)) if exclude.is_match(unit.path()))
        })
        .partition(|symbol| {
            symbol
                .file
                .to_str()
                .and_then(lang::registry::of)
                .is_some_and(|language| language.symbols)
        });
    Ok(Scan {
        all_files,
        paths,
        deps,
        unread_deps,
        unread,
        types,
        symbols,
        unverified_symbols,
        suppressions,
        uncovered: indexed.uncovered,
        unnamed_uses,
        unresolved_imports,
        parts_doubts,
        packages_problems,
        declarations_problems: {
            let mut out = Vec::new();
            for (name, (files, importers)) in ambiguities {
                let mut all = files.clone();
                all.extend(importers);
                out.push(crate::check::Unjudged::new(format!(
                    "{} places declare `{}`: {}: an import of it is taken for the copy in the importing file's component, otherwise the first by path",
                    files.len(),
                    name,
                    files.join(" and ")
                )).in_files(all));
            }
            out
        },
        known_symbols: facts.known,
        known_symbols_whole: facts.known_whole,
        indexed: indexed.ran,
        from_cache: indexed.cached,
        flow,
    })
}
