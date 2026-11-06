use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

/// Parsed files kept between scans, so watch mode re-reads only what changed.
/// Resolution is never cached: it depends on the other files.
#[derive(Default)]
pub struct Cache {
    parsed: HashMap<PathBuf, Parsed>,
    /// Whether the indexers' output is read from and kept in the index cache.
    pub index_cache: indexer::CacheUse,
    /// How many languages are indexed at once.
    pub jobs: indexer::Jobs,
}

impl Cache {
    /// A cache that keeps the indexers' output between runs as `index_cache` says,
    /// and indexes `jobs` languages at once.
    pub fn new(index_cache: indexer::CacheUse, jobs: indexer::Jobs) -> Self {
        Self {
            index_cache,
            jobs,
            ..Self::default()
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
    /// Files whose test code a finder of parts could not tell from the file's.
    pub parts_doubts: Vec<(String, lang::parts::Doubt)>,
    /// Manifests, or dependencies of one, that `packages` could not judge as declared.
    pub packages_problems: Vec<crate::check::Unjudged>,
    /// Names that more than one component declares, so that an import of one
    /// could not be told for certain.
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
    /// indexer and every name it saw named. Only then can a name it lacks be told
    /// to be missing.
    pub fn indexed_whole(&self) -> bool {
        self.uncovered.is_empty()
            && self.unnamed_uses.is_empty()
            && self.known_symbols_whole
            && self.paths.iter().all(|f| indexer::indexed(f))
    }
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
pub fn scan(
    root: &Path,
    policy: &ScanPolicy,
    map: &crate::components::ComponentMap,
    cache: &mut Cache,
) -> Result<Scan> {
    let exclude = glob_set(&policy.exclude, "exclude")?;
    let target_only = glob_set(&policy.target_only, "target_only")?;
    let adapters = lang::adapters(root, map)?;
    let mut paths = Vec::new();
    let mut all_files = Vec::new();
    // Excluded files the walk sees (not the ones .gitignore hides).
    let mut excluded = BTreeSet::new();
    let mut deps = Vec::new();
    let mut suppressions = Vec::new();
    let mut seen = HashSet::new();
    // The name of an ambiguous declaration -> the files that declare it and
    // the files that import it.
    let mut ambiguities: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for entry in walker(root).build() {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(rel) = lang::relative(root, path) else {
            continue;
        };
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
            deps.push(Dep::module(
                rel.clone(),
                import.line,
                import.specifier.clone(),
                adapter.resolve(path, &import.specifier),
            ));
            // A name that more than one component declares: the import is taken
            // for the copy in the component of the file that imports it, and
            // the ambiguity is said, for the rest to judge.
            if let Some(ambiguous) = adapter.ambiguity(&import.specifier) {
                let entry = ambiguities
                    .entry(ambiguous.name)
                    .or_insert_with(|| (ambiguous.files, Vec::new()));
                entry.1.push(file.clone());
            }
        }
    }
    cache.parsed.retain(|path, _| seen.contains(path));

    // Sorted, as the set is.
    let excluded: Vec<String> = excluded.into_iter().collect();
    let indexed = indexer::run(
        root,
        &all_files,
        &excluded,
        cache.index_cache,
        cache.jobs,
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
    let mut types: Vec<TypeDef> = Vec::new();
    for document in documents
        .iter()
        .filter(|d| !target_only.is_match(d.path.as_str()))
    {
        paths.push(document.path.clone());
        if let Ok(source) = std::fs::read_to_string(root.join(&document.path)) {
            suppressions.extend(suppress::parse(&document.path, &source));
        }
        types.extend(document.types.iter().map(|(name, line)| TypeDef {
            file: document.path.clone(),
            line: *line,
            name: name.clone(),
        }));
    }
    let flow =
        (policy.reach == crate::config::Reach::Symbols).then(|| crate::scip::flow(&documents));
    let facts = scip::facts_watching(
        &documents,
        &hidden,
        policy.uses_symbols,
        &policy.watched_symbols,
        Some(map),
    );
    let is_source = |file: &Path| {
        let file = slash(file);
        project.contains(file.as_str()) && !target_only.is_match(&file)
    };
    let (sources, mut unread_deps): (Vec<Dep>, Vec<Dep>) =
        facts.deps.into_iter().partition(|dep| is_source(&dep.file));
    deps.extend(sources);
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
        unread_deps.extend(scip::facts(&hidden, &documents, false).deps);
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
    // Names that more than one component declares and that a file imports: the
    // import is taken for the copy in the component of the file that imports
    // it, and the ambiguity is said, for the rest to judge.
    let mut declarations_problems = Vec::new();
    for (name, (files, importers)) in ambiguities {
        let mut all = files.clone();
        all.extend(importers);
        declarations_problems.push(
            crate::check::Unjudged::new(format!(
                "two places declare `{}`: {}: an import of it is taken for the copy in the component of the file that imports it",
                name,
                files.join(" and ")
            ))
            .in_files(all),
        );
    }
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
        parts_doubts,
        packages_problems,
        declarations_problems,
        known_symbols: facts.known,
        known_symbols_whole: facts.known_whole,
        indexed: indexed.ran,
        from_cache: indexed.cached,
        flow,
    })
}
