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
    /// The types of checked files that implement or extend another type of them,
    /// as the indexes declare it, closed over every chain of ancestors.
    pub implementations: Vec<lang::Implementation>,
    /// Declarations of checked files whose index gives no relations although they
    /// may implement or extend a type.
    pub unrelated: Vec<lang::UnrelatedDeclaration>,
    pub suppressions: Vec<Suppression>,
    /// Source files of an indexed language that the indexer did not cover,
    /// for example files outside every `tsconfig.json`, and the source files an
    /// adapter could not read.
    pub uncovered: Vec<String>,
    /// Source files easement could not read: what they import beyond the index,
    /// and what they suppress, is unknown.
    pub unreadable: Vec<String>,
    /// Checked files that use a name of a package the indexer left unnamed,
    /// which no rule on names judged.
    pub unnamed_uses: Vec<String>,
    /// Imports that checked files of the index write that the index took to no
    /// source target and whose text names none either: their dependencies are
    /// missing (scip-go on an import cycle, a package that does not type-check, a
    /// TypeScript import a wildcard declaration catches).
    pub unresolved_imports: Vec<Unresolved>,
    /// Unresolved imports of the files the checks do not read (`target_only`,
    /// excluded): no check judges their dependencies, so these are no gap, but
    /// `unused` cannot follow them.
    pub unread_unresolved_imports: Vec<Unresolved>,
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

/// What the comparison of the written imports of some files found beyond the index.
#[derive(Default)]
struct Written {
    /// Dependencies read from the text of imports the index took to no source target.
    spelled: Vec<Dep>,
    /// Imports whose text names no target either.
    unresolved: Vec<Unresolved>,
}

/// `import`, written in `file`, against what the index holds on it (`resolved`).
/// When the index takes it to no source target, a path its text spells that names
/// a file that `exists` exactly, or a package it spells, is its dependency (never
/// for a call that imports a module, which reads it in its own terms); else it
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
            // A call reads its module in its own terms (a package it is given), so
            // its text names no file.
            let spelled = written.spelled.filter(|_| form != Form::ImportCall);
            let target = match spelled.map(|spelled| spelled(&import.specifier)) {
                _ if resolution == scip::Resolution::Target => return Compared::Resolved,
                Some(Spelled::Paths(paths)) => only_file(file, &paths, &exists),
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
                (_, Form::ImportCall) => Cause::ImportCall,
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

/// The file of the project that one of `paths`, relative to `file`, names, when
/// exactly one of them names one that `exists`.
fn only_file(file: &str, paths: &[String], exists: impl Fn(&str) -> bool) -> Option<Target> {
    let mut found = paths.iter().filter_map(|path| {
        crate::relpath::resolve(crate::relpath::parent(file), path).filter(|path| exists(path))
    });
    match (found.next(), found.next()) {
        (Some(path), None) => Some(Target::Internal(lang::Unit::from(path.as_str()))),
        _ => None,
    }
}

/// Adds to `deps` the dependencies read from the text of imports (`spelled`): one
/// dependency of a file on a target, the index's, or else the first line that
/// spells it.
fn add_spelled(deps: &mut Vec<Dep>, mut spelled: Vec<Dep>) {
    let mut known: HashSet<(PathBuf, Target)> = deps
        .iter()
        .map(|dep| (dep.file.clone(), dep.target.clone()))
        .collect();
    spelled.sort_by_key(|dep| dep.line);
    deps.extend(
        spelled
            .into_iter()
            .filter(|dep| known.insert((dep.file.clone(), dep.target.clone()))),
    );
}

/// The files of a project as [`scan`] walks them, `/`-separated and relative to the root.
pub struct ProjectFiles {
    /// Every file that is not excluded.
    pub files: Vec<String>,
    /// The files `exclude` hides (not the ones .gitignore hides), sorted.
    pub excluded: Vec<String>,
}

/// The project's files, as [`scan`] walks them and hands them to the indexers.
pub fn project_files(root: &Path, policy: &ScanPolicy) -> Result<ProjectFiles> {
    let exclude = glob_set(&policy.exclude, "exclude")?;
    let mut files = Vec::new();
    let mut excluded = BTreeSet::new();
    for entry in walker(root).build() {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Some(rel) = lang::relative(root, entry.path()) {
            if exclude.is_match(&rel) {
                excluded.insert(slash(&rel));
            } else {
                files.push(slash(&rel));
            }
        }
    }
    Ok(ProjectFiles {
        files,
        excluded: excluded.into_iter().collect(),
    })
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
    // Source files an adapter would parse that cannot be read.
    let mut unread_sources: Vec<String> = Vec::new();
    // Every source file that cannot be read, those of the index too.
    let mut unreadable: Vec<String> = Vec::new();
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
            // Imports are ASCII: a file in another encoding is read all the same.
            // One that cannot be read is no file nothing imports: it is uncovered.
            let source = match std::fs::read(path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => {
                    unread_sources.push(file.clone());
                    continue;
                }
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
    let mut unrelated_declarations: Vec<lang::UnrelatedDeclaration> = Vec::new();
    // What the comparison of written imports found, in checked files and in unread ones.
    let mut checked_imports = Written::default();
    let mut unread_imports = Written::default();
    // A file the text of an import may name: one of the project, excluded or not.
    let exists = |file: &str| {
        project.contains(file) || excluded.binary_search_by(|f| f.as_str().cmp(file)).is_ok()
    };
    // Every document the index read, and whether the checks read its file: what an
    // unread file imports still reaches a file for `unused`.
    let read = documents
        .iter()
        .map(|d| (d, !target_only.is_match(d.path.as_str())))
        .chain(hidden.iter().map(|d| (d, false)));
    for (document, checked) in read {
        if checked {
            paths.push(document.path.clone());
            types.extend(document.types.iter().map(|defined| TypeDef {
                file: document.path.clone(),
                line: defined.line,
                name: defined.name.clone(),
            }));
        }
        let Ok(bytes) = std::fs::read(root.join(&document.path)) else {
            if checked {
                unreadable.push(document.path.clone());
            }
            continue;
        };
        // Directives and imports are ASCII, whatever the encoding of the file.
        let source = String::from_utf8_lossy(&bytes);
        if checked {
            suppressions.extend(suppress::parse(&document.path, &source));
        }
        if checked
            && policy.implementations
            && let Some(lang::registry::Relations::Declared {
                unrelated,
                anonymous,
            }) = lang::registry::of(&document.path).map(|language| &language.relations)
        {
            if let Some(unrelated) = unrelated {
                for span in (unrelated.find)(&document.path, &source) {
                    let writes = index.files_written(document, &span.lines, span.through_alias);
                    if !writes.is_empty() {
                        unrelated_declarations.push(lang::UnrelatedDeclaration {
                            file: document.path.clone(),
                            line: *span.lines.start(),
                            because: unrelated.because,
                            writes,
                        });
                    }
                }
            }
            if let Some(because) = anonymous {
                for class in &document.anonymous {
                    unrelated_declarations.push(lang::UnrelatedDeclaration {
                        file: document.path.clone(),
                        line: class.line,
                        because,
                        writes: index.files_defining(class.implements.iter().map(String::as_str)),
                    });
                }
            }
        }
        let Some(written) = lang::registry::of(&document.path)
            .and_then(|language| language.written_imports.as_ref())
            // A declaration file describes code that lives elsewhere.
            .filter(|_| !lang::registry::is_declaration(&document.path))
        else {
            continue;
        };
        let lines: Vec<&str> = source.lines().collect();
        for import in (written.list)(&document.path, &source) {
            let resolved = |import: &Import| index.resolution(document, &lines, import);
            let found = if checked {
                &mut checked_imports
            } else {
                &mut unread_imports
            };
            match compare(&document.path, import, written, resolved, exists) {
                Compared::Resolved => {}
                Compared::Spelled(dep) => found.spelled.push(*dep),
                Compared::Unresolved(unresolved) => found.unresolved.push(unresolved),
            }
        }
    }
    let implementations = index
        .implementations()
        .into_iter()
        .filter(|i| !target_only.is_match(&i.sub.file) && !target_only.is_match(&i.supertype.file))
        .collect();
    let flow = (policy.reach == crate::config::Reach::Symbols).then(|| index.flow());
    let facts = index.facts(policy.uses_symbols, &policy.watched_symbols);
    let is_source = |file: &Path| {
        let file = slash(file);
        project.contains(file.as_str()) && !target_only.is_match(&file)
    };
    let (sources, mut unread_deps): (Vec<Dep>, Vec<Dep>) =
        facts.deps.into_iter().partition(|dep| is_source(&dep.file));
    deps.extend(sources);
    add_spelled(&mut deps, checked_imports.spelled);
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
    add_spelled(&mut unread_deps, unread_imports.spelled);
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
        implementations,
        unrelated: unrelated_declarations,
        symbols,
        unverified_symbols,
        suppressions,
        uncovered: {
            let mut uncovered = indexed.uncovered;
            uncovered.extend(unread_sources.iter().cloned());
            uncovered.sort();
            uncovered.dedup();
            uncovered
        },
        unreadable: {
            unreadable.extend(unread_sources);
            unreadable.sort();
            unreadable.dedup();
            unreadable
        },
        unnamed_uses,
        unresolved_imports: checked_imports.unresolved,
        unread_unresolved_imports: unread_imports.unresolved,
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
