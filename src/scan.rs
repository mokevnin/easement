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
    /// Imports that checked files of the index write, by file, on whose line the
    /// index names no module: the indexer did not resolve them, so their
    /// dependencies are missing (scip-go on an import cycle, a package that does
    /// not type-check).
    pub unresolved_imports: Vec<(String, Import)>,
    /// Imports of files an adapter reads that look like project code but name no
    /// file of it (an alias whose generated config is missing), so their
    /// dependencies are missing.
    pub unresolved_local: Vec<(String, Import)>,
    /// Files whose test code a finder of parts could not tell from the file's.
    pub parts_doubts: Vec<(String, lang::parts::Doubt)>,
    /// Manifests, or dependencies of one, that `packages` could not judge as declared.
    pub packages_problems: Vec<crate::check::Unjudged>,
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
    let mut suppressions = Vec::new();
    let mut seen = HashSet::new();
    let mut unresolved_local = Vec::new();
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
        paths.push(file);
        suppressions.extend(parsed.suppressions.iter().cloned());
        for import in &parsed.imports {
            let target = adapter.resolve(path, &import.specifier);
            if matches!(target, Target::External(_)) && adapter.looks_local(&import.specifier) {
                unresolved_local.push((slash(&rel), import.clone()));
                continue;
            }
            deps.push(Dep::module(
                rel.clone(),
                import.line,
                import.specifier.clone(),
                target,
            ));
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
    let mut bridged: Vec<Dep> = Vec::new();
    for document in documents
        .iter()
        .filter(|d| !target_only.is_match(d.path.as_str()))
    {
        paths.push(document.path.clone());
        if let Ok(source) = std::fs::read_to_string(root.join(&document.path)) {
            suppressions.extend(suppress::parse(&document.path, &source));
            // What the index may not follow into a file an adapter reads.
            for adapter in &adapters {
                for (import, target) in adapter.imports_into(Path::new(&document.path), &source) {
                    bridged.push(Dep::module(
                        document.path.clone(),
                        import.line,
                        import.specifier,
                        target,
                    ));
                }
            }
            // Each import the file writes must have left a module on its path in the index.
            if let Some(written) =
                lang::registry::of(&document.path).and_then(|language| language.written_imports)
            {
                let lines: Vec<&str> = source.lines().collect();
                unresolved_imports.extend(
                    written(&source)
                        .into_iter()
                        .filter(|import| !document.resolves(&lines, import, &names))
                        .map(|import| (document.path.clone(), import)),
                );
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
    // Only what the index did not resolve itself, so no dependency counts twice.
    bridged.retain(|dep| {
        !deps
            .iter()
            .any(|known| known.file == dep.file && known.target == dep.target)
    });
    deps.extend(bridged);
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
        unresolved_local,
        parts_doubts,
        packages_problems,
        known_symbols: facts.known,
        known_symbols_whole: facts.known_whole,
        indexed: indexed.ran,
        from_cache: indexed.cached,
        flow,
    })
}
