//! Reads a SCIP index and turns it into the facts the rules check.
//!
//! An indexer (`scip-go`, `scip-typescript`, …) resolves every name in the
//! project the way the compiler does. Two kinds of references matter here:
//!
//! - a reference to a *module* (a Go package, a TypeScript file, a Java class) stands
//!   on the lines that import or use that module, so it is a dependency;
//! - a reference to a name at the top level of a module (`db.Client`) is a dependency
//!   on the file that defines it, also through a re-export;
//! - a reference to a *member* reached through a value (`c.Query()`) is only a use of
//!   a name.
//!
//! Every scheme is read into one shape first (see [`Name`]), so the facts are made
//! the same way for every language.
//!
//! References to symbols that no document of the index defines point outside
//! the project: they are external modules.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use protobuf::Message;
use scip::types::{Index, Occurrence, PositionEncoding, occurrence::Typed_range};

use crate::check::SymbolRef;
use crate::flow::{Edge, Flow, Node, Use};
use crate::lang::language::Language;
use crate::lang::parts::Part;
use crate::lang::registry;
use crate::lang::{
    AliasDefinition, Aliased, Dep, Elsewhere, Hop, Import, Mention, Scope, Through, Written,
};
use crate::lang::{Target, Unit};
use crate::relpath;

pub mod symbol;

use crate::lang::language::Symbols;
use symbol::{Called, Kind, Name, Qualifier, Shape, classes, modules, split};

/// `SymbolRole::Definition` in the bitset of an occurrence.
const DEFINITION: i32 = 1;

/// One reference to a symbol in a document.
#[derive(Debug, Clone)]
pub struct Reference {
    /// 1-based line.
    pub line: usize,
    /// The columns it spans on its line, counted in the code units of its
    /// document's `encoding`; `None` when its range is not on one line.
    pub columns: Option<(usize, usize)>,
    pub symbol: String,
    /// For a reference into a package the index left unnamed that stands on the
    /// string of an import: the module the string spells, as the entry of its
    /// language reads it ([`crate::lang::language::UnnamedPackages`]).
    pub spelled: Option<String>,
}

/// A source file of the index, with the paths relative to the project root.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub path: String,
    pub references: Vec<Reference>,
    /// Symbols this file defines.
    pub definitions: Vec<String>,
    /// The types this file defines (classes, structs, interfaces, traits).
    pub types: Vec<DefinedType>,
    /// The types each type of this file declares it implements or extends, as the
    /// index relates them: (type, supertype). Only a language whose relations are
    /// declared gives them, directly or through ancestors as its indexer does.
    pub supertypes: Vec<(String, String)>,
    /// The classes of this file the index gives no type but whose methods it
    /// relates to the methods they implement (a Java anonymous class).
    pub anonymous: Vec<Anonymous>,
    /// The methods and functions this file defines that the index gave a range.
    pub scopes: Vec<Scope>,
    /// The parts of the file that a dependency can belong to instead of the file.
    pub parts: Vec<crate::lang::parts::Span>,
    /// How the index counts the columns of its references.
    pub encoding: PositionEncoding,
    /// The names this file defines that the index marks as aliases.
    pub aliases: Vec<Alias>,
}

/// A type a document defines, by its symbol, its name and its 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinedType {
    pub symbol: String,
    pub name: String,
    pub line: usize,
}

/// Whether `symbol` names a type: its last descriptor is one.
fn is_type(symbol: &str) -> bool {
    split(symbol).is_some_and(|parsed| matches!(parsed.descriptors.last(), Some((_, Kind::Type))))
}

/// The supertypes the types of `document`, at `path`, declare, as (type,
/// supertype): the relations of a type to a type that are implementations and no
/// reference. scip-java also relates an interface to a record that implements it,
/// as a reference too; that one is no supertype. A method's relation to the method
/// it overrides follows from its type's and is not read. A type the index names
/// locally (`local 0`, a class inside a function) is read under [`local_key`].
fn supertypes_of(document: &scip::types::Document, path: &str) -> Vec<(String, String)> {
    let key = |symbol: &str| match is_local(symbol) {
        true => local_key(path, symbol),
        false => symbol.to_string(),
    };
    let mut found = Vec::new();
    for information in &document.symbols {
        if !is_type(&information.symbol) && !is_local(&information.symbol) {
            continue;
        }
        for relationship in &information.relationships {
            if relationship.is_implementation
                && !relationship.is_reference
                && relationship.symbol != information.symbol
                && (is_type(&relationship.symbol) || is_local(&relationship.symbol))
            {
                found.push((key(&information.symbol), key(&relationship.symbol)));
            }
        }
    }
    found
}

/// A class the index gives no type, told by its methods that implement methods
/// of types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anonymous {
    /// The 1-based line of the first of those methods.
    pub line: usize,
    /// The types whose methods they implement.
    pub implements: Vec<String>,
}

/// The classes of `document` the index gives no type, as their methods show them:
/// a method it names locally that implements a method of a type, grouped by the
/// class that encloses it when the index says which. `local_types` are the types
/// it names locally, whose relations are read as such.
fn anonymous_of(
    document: &scip::types::Document,
    path: &str,
    local_types: &HashSet<String>,
) -> Vec<Anonymous> {
    // The type a method symbol (`Port#find().`) is a member of.
    let owner = |method: &str| {
        let cut = method.rfind('#')?;
        let owner = &method[..=cut];
        (method.ends_with(").") && is_type(owner)).then(|| owner.to_string())
    };
    let mut classes: BTreeMap<String, (Vec<&str>, Vec<String>)> = BTreeMap::new();
    for information in &document.symbols {
        let symbol = information.symbol.as_str();
        if !is_local(symbol) || local_types.contains(&local_key(path, symbol)) {
            continue;
        }
        let implemented: Vec<String> = information
            .relationships
            .iter()
            .filter(|r| r.is_implementation)
            .filter_map(|r| owner(&r.symbol))
            .collect();
        if implemented.is_empty() {
            continue;
        }
        let class = match information.enclosing_symbol.as_str() {
            "" => symbol.to_string(),
            enclosing => enclosing.to_string(),
        };
        let (methods, types) = classes.entry(class).or_default();
        methods.push(symbol);
        for t in implemented {
            if !types.contains(&t) {
                types.push(t);
            }
        }
    }
    let mut found: Vec<Anonymous> = classes
        .into_values()
        .filter_map(|(methods, implements)| {
            let line = document
                .occurrences
                .iter()
                .filter(|o| {
                    o.symbol_roles & DEFINITION != 0 && methods.contains(&o.symbol.as_str())
                })
                .map(|o| line_of(o) + 1)
                .min()?;
            Some(Anonymous { line, implements })
        })
        .collect();
    found.sort_by_key(|anonymous| anonymous.line);
    found
}

/// Whether the index names `symbol` only within its document.
fn is_local(symbol: &str) -> bool {
    symbol.starts_with("local ")
}

/// The key of a symbol the index names only within the document at `path`: no
/// symbol of another document has it.
fn local_key(path: &str, symbol: &str) -> String {
    format!("{symbol} in {path}")
}

/// A name whose definition the index marks as standing for other names: a type
/// alias with the range of its definition (Go `type Block = blob.Block`, Rust
/// `pub type`, TypeScript `export type`), or a name the index relates to others by
/// reference alone (Ruby `Block = Blob::Block`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub symbol: String,
    /// The 1-based line of its definition.
    pub line: usize,
    /// The symbols its definition refers to within its range, or that the index
    /// relates it to, in the order they come.
    pub stands_for: Vec<String>,
}

/// What the index holds on the specifier of an import a file writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// A module it takes to a source target: a file of the project or an external
    /// package.
    Target,
    /// Only a declaration that stands for modules it does not name (`"*.css"`):
    /// the file the import means is not in the index.
    Wildcard,
    /// No module: the indexer did not resolve it.
    Nothing,
}

/// A position in a document: the 0-based line and column.
type Position = (i32, i32);

/// Where the enclosing range of an occurrence starts and ends.
fn enclosing(occurrence: &Occurrence) -> Option<(Position, Position)> {
    use scip::types::occurrence::Typed_enclosing_range as Typed;
    Some(
        match (
            &occurrence.typed_enclosing_range,
            occurrence.enclosing_range.as_slice(),
        ) {
            (Some(Typed::SingleLineEnclosingRange(r)), _) => {
                ((r.line, r.start_character), (r.line, r.end_character))
            }
            (Some(Typed::MultiLineEnclosingRange(r)), _) => (
                (r.start_line, r.start_character),
                (r.end_line, r.end_character),
            ),
            (Some(_), _) => return None,
            (None, [line, start, end]) => ((*line, *start), (*line, *end)),
            (None, [start_line, start, end_line, end]) => {
                ((*start_line, *start), (*end_line, *end))
            }
            (None, _) => return None,
        },
    )
}

/// The first and last line, 0-based, of the enclosing range of an occurrence.
fn enclosing_lines(occurrence: &Occurrence) -> Option<(usize, usize)> {
    let ((first, _), (last, _)) = enclosing(occurrence)?;
    Some((usize::try_from(first).ok()?, usize::try_from(last).ok()?))
}

/// Where the range of an occurrence starts.
fn start_of(occurrence: &Occurrence) -> Option<Position> {
    match (&occurrence.typed_range, occurrence.range.as_slice()) {
        (Some(Typed_range::SingleLineRange(r)), _) => Some((r.line, r.start_character)),
        (Some(Typed_range::MultiLineRange(r)), _) => Some((r.start_line, r.start_character)),
        (Some(_), _) => None,
        (None, [line, start, ..]) => Some((*line, *start)),
        (None, _) => None,
    }
}

/// The aliases `document` defines, as its `symbols` mark them: a `TypeAlias` stands
/// for what the occurrences within the enclosing range of its definition refer to, and
/// a name related to others by reference alone, not as an implementation, for those.
fn aliases_of(document: &scip::types::Document) -> Vec<Alias> {
    use scip::types::symbol_information::Kind as SymbolKind;
    let mut typed: HashSet<&str> = HashSet::new();
    let mut related: HashMap<&str, Vec<String>> = HashMap::new();
    for information in &document.symbols {
        if information.kind.enum_value() == Ok(SymbolKind::TypeAlias) {
            typed.insert(&information.symbol);
        }
        let to: Vec<String> = information
            .relationships
            .iter()
            .filter(|r| {
                r.is_reference && !r.is_implementation && !r.is_type_definition && !r.is_definition
            })
            .map(|r| r.symbol.clone())
            .filter(|symbol| *symbol != information.symbol && !symbol.starts_with("local "))
            .collect();
        if !to.is_empty() {
            related.insert(&information.symbol, to);
        }
    }
    if typed.is_empty() && related.is_empty() {
        return Vec::new();
    }
    // Where each reference starts, in order, for the ranges of the type aliases.
    let mut starts: Vec<(Position, &str)> = match typed.is_empty() {
        true => Vec::new(),
        false => document
            .occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.symbol_roles & DEFINITION == 0
                    && !occurrence.symbol.is_empty()
                    && !occurrence.symbol.starts_with("local ")
            })
            .filter_map(|occurrence| Some((start_of(occurrence)?, occurrence.symbol.as_str())))
            .collect(),
    };
    starts.sort_by_key(|(at, _)| *at);
    let mut aliases = Vec::new();
    for occurrence in &document.occurrences {
        if occurrence.symbol_roles & DEFINITION == 0 {
            continue;
        }
        let symbol = occurrence.symbol.as_str();
        let line = line_of(occurrence) + 1;
        if let Some(to) = related.get(symbol) {
            aliases.push(Alias {
                symbol: symbol.to_string(),
                line,
                stands_for: to.clone(),
            });
        } else if typed.contains(symbol)
            && let Some((start, end)) = enclosing(occurrence)
        {
            let first = starts.partition_point(|(at, _)| *at < start);
            let mut stands_for: Vec<String> = Vec::new();
            for (_, other) in starts[first..].iter().take_while(|(at, _)| *at <= end) {
                if *other != symbol && !stands_for.iter().any(|seen| seen == other) {
                    stands_for.push(other.to_string());
                }
            }
            aliases.push(Alias {
                symbol: symbol.to_string(),
                line,
                stands_for,
            });
        }
    }
    aliases
}

/// The documents of one or more indexes, merged.
#[derive(Debug, Default)]
pub struct Documents(pub Vec<Document>);

/// Parses an index written by an indexer that ran in `base` (a directory
/// relative to the project `root`, `""` for the root itself). Documents outside
/// the project, such as generated files in a build cache, are dropped. The
/// source of a document is read only for a reference into an unnamed package.
pub fn parse(bytes: &[u8], root: &Path, base: &Path) -> Result<Vec<Document>> {
    let index = Index::parse_from_bytes(bytes).context("not a SCIP index")?;
    let prefix = relpath::slash(base);
    let mut documents = Vec::new();
    for document in &index.documents {
        // Indexers on Windows may write `\` separators.
        let normalized = document.relative_path.replace('\\', "/");
        let relative = normalized.as_str();
        if relative.starts_with("..")
            || relative.starts_with('/')
            || relative.as_bytes().get(1) == Some(&b':')
        {
            continue;
        }
        let path = relpath::join(&prefix, relative);
        let encoding = document.position_encoding.enum_value_or_default();
        let mut source: Option<Vec<String>> = None;
        let declared = registry::of(&path).is_some_and(|language| language.relations.declared());
        let supertypes = match declared {
            true => supertypes_of(document, &path),
            false => Vec::new(),
        };
        // The types the index names locally that declare a supertype.
        let local_types: HashSet<String> = supertypes
            .iter()
            .filter(|(sub, _)| is_local(sub))
            .map(|(sub, _)| sub.clone())
            .collect();
        let anonymous = match declared {
            true => anonymous_of(document, &path, &local_types),
            false => Vec::new(),
        };
        let mut parsed = Document {
            path,
            anonymous,
            encoding,
            aliases: aliases_of(document),
            supertypes,
            ..Document::default()
        };
        // The packages the index of the document's language may leave unnamed.
        let unnamed = registry::of(&parsed.path)
            .and_then(Language::indexer)
            .and_then(|indexer| indexer.unnamed.as_ref());
        for occurrence in &document.occurrences {
            if is_local(&occurrence.symbol)
                && occurrence.symbol_roles & DEFINITION != 0
                && local_types.contains(&local_key(&parsed.path, &occurrence.symbol))
            {
                // Its name is the text the definition stands on.
                let lines = source.get_or_insert_with(|| read_lines(root, &parsed.path));
                let name = single_line(occurrence)
                    .and_then(|(line, start, end)| {
                        between(lines.get(line)?, (start, end), encoding)
                    })
                    .unwrap_or("a local class")
                    .to_string();
                parsed.types.push(DefinedType {
                    symbol: local_key(&parsed.path, &occurrence.symbol),
                    name,
                    line: line_of(occurrence) + 1,
                });
            }
            if occurrence.symbol.is_empty() || is_local(&occurrence.symbol) {
                continue;
            }
            if occurrence.symbol_roles & DEFINITION != 0 {
                if let Some(parsed_symbol) = split(&occurrence.symbol)
                    && let Some((name, Kind::Type)) = parsed_symbol.descriptors.last()
                {
                    parsed.types.push(DefinedType {
                        symbol: occurrence.symbol.clone(),
                        name: name.clone(),
                        line: line_of(occurrence) + 1,
                    });
                }
                // A method or function is `name().`; a parameter is `(name)`.
                if occurrence.symbol.ends_with(").")
                    && let Some((first, last)) = enclosing_lines(occurrence)
                {
                    parsed.scopes.push(Scope {
                        symbol: occurrence.symbol.clone(),
                        first: first + 1,
                        last: last + 1,
                    });
                }
                parsed.definitions.push(occurrence.symbol.clone());
            } else {
                let spelled = unnamed
                    .filter(|unnamed| (unnamed.is)(&occurrence.symbol))
                    .and_then(|unnamed| {
                        let lines = source.get_or_insert_with(|| read_lines(root, &parsed.path));
                        let string = string_at(lines, occurrence, encoding)?;
                        (unnamed.spelled)(string, &root.join(&parsed.path))
                    });
                parsed.references.push(Reference {
                    line: line_of(occurrence) + 1,
                    columns: single_line(occurrence).map(|(_, start, end)| (start, end)),
                    symbol: occurrence.symbol.clone(),
                    spelled,
                });
            }
        }
        documents.push(parsed);
    }
    Ok(documents)
}

/// The lines of the file at `path` under `root`. A name or an import is ASCII,
/// whatever the encoding of the file; one that cannot be read has no lines, and
/// the scan reports it.
fn read_lines(root: &Path, path: &str) -> Vec<String> {
    String::from_utf8_lossy(&std::fs::read(root.join(path)).unwrap_or_default())
        .lines()
        .map(str::to_string)
        .collect()
}

/// What the string an occurrence stands on holds between its quotes (`"hono"`,
/// `'@scope/pkg/sub'`); `None` for a name, or a range over more than one line.
fn string_at<'a>(
    lines: &'a [String],
    occurrence: &Occurrence,
    encoding: PositionEncoding,
) -> Option<&'a str> {
    let (line, start, end) = single_line(occurrence)?;
    unquoted(between(lines.get(line)?, (start, end), encoding)?)
}

/// The line, 0-based, and the start and end columns of an occurrence whose
/// range is on one line.
fn single_line(occurrence: &Occurrence) -> Option<(usize, usize, usize)> {
    let (line, start, end) = match (&occurrence.typed_range, occurrence.range.as_slice()) {
        (Some(Typed_range::SingleLineRange(r)), _) => (r.line, r.start_character, r.end_character),
        (Some(_), _) => return None,
        (None, [line, start, end]) => (*line, *start, *end),
        (None, _) => return None,
    };
    let column = |n: i32| usize::try_from(n).ok();
    Some((column(line)?, column(start)?, column(end)?))
}

/// The text of `line` between two columns counted in the code units of
/// `encoding`; TypeScript counts UTF-16 ones when the index does not say.
fn between(line: &str, (start, end): (usize, usize), encoding: PositionEncoding) -> Option<&str> {
    let offset = |column: usize| -> Option<usize> {
        let mut units = 0;
        for (byte, c) in line.char_indices() {
            if units == column {
                return Some(byte);
            }
            units += match encoding {
                PositionEncoding::UTF8CodeUnitOffsetFromLineStart => c.len_utf8(),
                PositionEncoding::UTF32CodeUnitOffsetFromLineStart => 1,
                _ => c.len_utf16(),
            };
        }
        (units == column).then_some(line.len())
    };
    line.get(offset(start)?..offset(end)?)
}

/// What a string literal holds between its quotes: `"…"`, `'…'` or a backtick pair.
fn unquoted(text: &str) -> Option<&str> {
    ['"', '\'', '`']
        .into_iter()
        .find_map(|quote| text.strip_prefix(quote)?.strip_suffix(quote))
}

/// The start line, 0-based, from the typed range or the deprecated one.
fn line_of(occurrence: &Occurrence) -> usize {
    let line = match &occurrence.typed_range {
        Some(Typed_range::SingleLineRange(range)) => range.line,
        Some(Typed_range::MultiLineRange(range)) => range.start_line,
        Some(_) | None => occurrence.range.first().copied().unwrap_or(0),
    };
    usize::try_from(line).unwrap_or(0)
}

/// The part of `document` that `line` is in.
fn part_at(document: &Document, line: usize) -> Option<&Part> {
    document
        .parts
        .iter()
        .find(|span| span.lines.contains(&line))
        .map(|span| &span.part)
}

/// The node of `document` that `line` is in: a part of the file, or the file.
fn unit_at(document: &Document, line: usize) -> Unit {
    match part_at(document, line) {
        Some(part) => Unit::in_part(Path::new(&document.path), part.clone()),
        None => Unit::from(document.path.as_str()),
    }
}

/// A dependency of a file before it is kept, without its line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DepKey<'a> {
    /// The file it is on; `None` for an external module.
    internal: Option<&'a str>,
    /// The external module; empty for a file of the project.
    external: String,
    /// The part of the file the line is in.
    part: Option<&'a Part>,
    written: Written,
}

/// Dependencies and symbol uses of the documents.
pub struct Facts {
    pub deps: Vec<Dep>,
    pub symbols: Vec<SymbolRef>,
    /// With `symbols`, the files that use a name of an unnamed package, whose
    /// uses are left out of `symbols`.
    pub unnamed: BTreeSet<String>,
    /// With `watched`, the qualifier and name of every symbol the index defines or
    /// references whose name is one of `watched`: what a `forbid_symbol` name can mean.
    pub known: Vec<(String, String)>,
    /// Whether `known` is whole: false when the index left some definitions unnamed.
    pub known_whole: bool,
}

/// For the tests: [`Indexed::facts`] of `documents`, with `excluded` beside them.
#[cfg(test)]
fn facts(documents: &[Document], excluded: &[Document], symbols: bool) -> Facts {
    let names = Names::of(documents, excluded);
    Indexed::of(&names).facts(symbols, &BTreeSet::new())
}

/// The files that define each symbol of some documents.
struct Definitions<'a> {
    /// Symbol -> the file that defines it, the first by path when several do.
    defined: HashMap<&'a str, &'a str>,
    /// Symbols that more than one file defines: a Ruby module reopened in each file under it.
    reopened: HashSet<&'a str>,
    /// The names defined where a reference is matched by the ending of its path, by
    /// the last part of the path: the only ones whose path can end with a reference's.
    endings: HashMap<&'a str, Vec<Ending<'a>>>,
}

/// A name defined where a reference is matched by the ending of its path.
struct Ending<'a> {
    /// The names of the module it is, or lives in.
    path: &'a [String],
    /// The names below that module.
    inner: &'a [String],
    /// The file that defines it.
    file: &'a str,
    symbol: &'a str,
}

/// Where the definitions of some documents (`mine`) and, for what none of them
/// defines, of others put a symbol.
struct Lookup<'s, 'a> {
    mine: &'s Definitions<'a>,
    others: &'s Definitions<'a>,
}

impl<'a> Lookup<'_, 'a> {
    /// The file that defines `symbol`, the first by path when several do.
    fn defined_at(&self, symbol: &str) -> Option<&'a str> {
        match self.mine.defined.get(symbol) {
            Some(file) => Some(*file),
            None => self.others.defined.get(symbol).copied(),
        }
    }

    /// The one file that defines what `name` can mean, matched by the ending of its
    /// path. An excluded definition is the answer only when no checked one matches.
    fn ending(&self, name: &Name) -> Option<&'a str> {
        let last = name.path.last()?;
        [(self.mine, false), (self.others, true)]
            .into_iter()
            .find_map(|(definitions, other)| {
                let mut found = definitions
                    .endings
                    .get(last.as_str())?
                    .iter()
                    .filter(|ending| !other || !self.mine.defined.contains_key(ending.symbol))
                    .filter(|Ending { path, inner, .. }| {
                        *inner == name.inner.as_slice() && path.ends_with(&name.path)
                    })
                    .map(|ending| ending.file);
                let first = found.next()?;
                found.all(|file| file == first).then_some(first)
            })
    }

    /// The file of the project a module, or a name at the top level of one, is in.
    fn file_of(&self, name: &Name) -> Option<&'a str> {
        if !matches!(name.shape, Shape::Module | Shape::TopLevel) {
            return None;
        }
        self.defined_at(&name.symbol)
            .or_else(|| name.reading.by_ending.then(|| self.ending(name)).flatten())
    }
}

impl<'a> Definitions<'a> {
    fn of(documents: &'a [Document], names: &'a Names<'a>) -> Self {
        let mut defined: HashMap<&str, &str> = HashMap::new();
        let mut reopened: HashSet<&str> = HashSet::new();
        for document in documents {
            for symbol in &document.definitions {
                let entry = defined.entry(symbol).or_insert(&document.path);
                if *entry != document.path.as_str() {
                    reopened.insert(symbol);
                }
                if document.path.as_str() < *entry {
                    *entry = &document.path;
                }
            }
        }
        let mut endings: HashMap<&str, Vec<Ending>> = HashMap::new();
        for (symbol, path) in &defined {
            let Some(name) = names.get(symbol).filter(|name| name.reading.by_ending) else {
                continue;
            };
            if let Some(last) = name.path.last() {
                endings.entry(last).or_default().push(Ending {
                    path: &name.path,
                    inner: &name.inner,
                    file: path,
                    symbol,
                });
            }
        }
        Self {
            defined,
            reopened,
            endings,
        }
    }
}

/// The documents of an index, checked and excluded, with what every pass over them
/// shares: each distinct symbol read once with [`read`], and the files that define
/// each symbol of either kind.
pub struct Indexed<'a> {
    names: &'a Names<'a>,
    checked: Definitions<'a>,
    hidden: Definitions<'a>,
    /// Packages that define something are the project's own: a reference into
    /// one that no definition matches (a relative import in Python) is not external.
    own: BTreeSet<String>,
    /// Every alias the checked and excluded documents define, with its file.
    aliases: HashMap<&'a str, (&'a str, &'a Alias)>,
}

/// The checked and the excluded documents of an index, with every symbol they
/// define or reference read once.
pub struct Names<'a> {
    documents: &'a [Document],
    excluded: &'a [Document],
    /// Symbol -> what it names; a symbol [`read`] cannot make out is missing.
    read: HashMap<&'a str, Name>,
}

impl<'a> Names<'a> {
    /// The checked `documents` and the `excluded` ones, whose symbols are read here.
    pub fn of(documents: &'a [Document], excluded: &'a [Document]) -> Self {
        let mut names = HashMap::new();
        for document in documents.iter().chain(excluded) {
            let symbols = document
                .definitions
                .iter()
                .chain(document.references.iter().map(|r| &r.symbol));
            for symbol in symbols {
                if !names.contains_key(symbol.as_str())
                    && let Some(name) = read(symbol)
                {
                    names.insert(symbol.as_str(), name);
                }
            }
        }
        Self {
            documents,
            excluded,
            read: names,
        }
    }

    /// What `symbol` names, when [`read`] makes it out.
    fn get(&self, symbol: &str) -> Option<&Name> {
        self.read.get(symbol)
    }
}

impl<'a> Indexed<'a> {
    /// The documents of `names`, with the files that define each of their symbols.
    pub fn of(names: &'a Names<'a>) -> Self {
        let checked = Definitions::of(names.documents, names);
        let hidden = Definitions::of(names.excluded, names);
        let own = checked
            .defined
            .keys()
            .chain(hidden.defined.keys())
            .filter_map(|symbol| names.get(symbol)?.package.clone())
            .collect();
        let mut aliases = HashMap::new();
        for document in names.documents.iter().chain(names.excluded) {
            for alias in &document.aliases {
                aliases
                    .entry(alias.symbol.as_str())
                    .or_insert((document.path.as_str(), alias));
            }
        }
        Self {
            names,
            checked,
            hidden,
            own,
            aliases,
        }
    }

    /// The files that define the names `document` writes on `lines`, and the names
    /// an alias among them stands for: a checked file, or an excluded one for a name
    /// no checked file defines. With `through_alias`, none unless one is an alias.
    pub fn files_written(
        &self,
        document: &Document,
        lines: &std::ops::RangeInclusive<usize>,
        through_alias: bool,
    ) -> BTreeSet<String> {
        let written: Vec<&str> = document
            .references
            .iter()
            .filter(|reference| lines.contains(&reference.line))
            .map(|reference| reference.symbol.as_str())
            .collect();
        if through_alias && !written.iter().any(|s| self.aliases.contains_key(s)) {
            return BTreeSet::new();
        }
        self.files_defining(written)
    }

    /// The files that define `symbols`, and the names an alias among them stands for.
    pub fn files_defining<'s>(
        &self,
        symbols: impl IntoIterator<Item = &'s str>,
    ) -> BTreeSet<String> {
        let lookup = Lookup {
            mine: &self.checked,
            others: &self.hidden,
        };
        let mut files = BTreeSet::new();
        for symbol in symbols {
            files.extend(lookup.defined_at(symbol));
            for (stood, _) in self.stood_for(symbol) {
                files.extend(lookup.defined_at(stood));
            }
        }
        files.into_iter().map(str::to_string).collect()
    }

    /// Every type of the checked documents that implements or extends another of
    /// them, directly or through any chain of ancestors, whether the index gives the
    /// relations transitively or only the direct ones. A chain goes through the types
    /// of excluded documents too. A supertype outside the project is not followed:
    /// no type it has can be the project's.
    pub fn implementations(&self) -> Vec<crate::lang::Implementation> {
        use crate::lang::{Implementation, TypeDef};
        // Every type of the project at the file that defines it, a checked one first.
        let mut types: BTreeMap<&str, TypeDef> = BTreeMap::new();
        let mut checked: BTreeSet<&str> = BTreeSet::new();
        let sides = [
            (self.names.documents, &self.checked, true),
            (self.names.excluded, &self.hidden, false),
        ];
        for (documents, definitions, is_checked) in sides {
            for document in documents {
                for defined in &document.types {
                    let symbol = defined.symbol.as_str();
                    // A local type is its document's alone.
                    let defined_here = is_local(symbol)
                        || definitions.defined.get(symbol) == Some(&document.path.as_str());
                    if !defined_here || types.contains_key(symbol) {
                        continue;
                    }
                    types.insert(
                        symbol,
                        TypeDef {
                            file: document.path.clone(),
                            line: defined.line,
                            name: defined.name.clone(),
                        },
                    );
                    if is_checked {
                        checked.insert(symbol);
                    }
                }
            }
        }
        let mut direct: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for document in self.names.documents.iter().chain(self.names.excluded) {
            for (sub, supertype) in &document.supertypes {
                if types.contains_key(sub.as_str()) && types.contains_key(supertype.as_str()) {
                    direct.entry(sub).or_default().insert(supertype);
                }
            }
        }
        let ancestors: BTreeMap<&str, BTreeSet<&str>> = direct
            .keys()
            .map(|&sub| {
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                let mut pending: Vec<&str> = vec![sub];
                while let Some(at) = pending.pop() {
                    for &up in direct.get(at).into_iter().flatten() {
                        if up != sub && seen.insert(up) {
                            pending.push(up);
                        }
                    }
                }
                (sub, seen)
            })
            .collect();
        let mut found = Vec::new();
        for (sub, ups) in ancestors.iter().filter(|(sub, _)| checked.contains(*sub)) {
            for supertype in ups.iter().filter(|up| checked.contains(*up)) {
                let between = ups
                    .iter()
                    .filter(|middle| {
                        *middle != supertype
                            && ancestors
                                .get(*middle)
                                .is_some_and(|a| a.contains(supertype))
                    })
                    .map(|middle| types[middle].clone())
                    .collect();
                found.push(Implementation {
                    sub: types[sub].clone(),
                    supertype: types[supertype].clone(),
                    between,
                });
            }
        }
        found
    }

    /// What `symbol` stands for when it is an alias: each symbol its definition
    /// names, with the definition of every alias from `symbol` to the one that names
    /// it. An alias among them is followed in turn, each once.
    fn stood_for(&self, symbol: &str) -> Vec<(&'a str, Vec<AliasDefinition>)> {
        let mut found = Vec::new();
        let Some((symbol, _)) = self.aliases.get_key_value(symbol) else {
            return found;
        };
        let mut followed: HashSet<&str> = HashSet::from([*symbol]);
        let mut pending: Vec<(&str, Vec<AliasDefinition>)> = vec![(symbol, Vec::new())];
        while let Some((symbol, chain)) = pending.pop() {
            let (file, alias) = self.aliases[symbol];
            let mut chain = chain;
            chain.push(AliasDefinition {
                file: file.to_string(),
                line: alias.line,
            });
            for named in &alias.stands_for {
                found.push((named.as_str(), chain.clone()));
                if let Some((named, _)) = self.aliases.get_key_value(named.as_str())
                    && followed.insert(named)
                {
                    pending.push((named, chain.clone()));
                }
            }
        }
        found
    }

    /// One `Dep` per file and module it refers to, at the first line that does:
    /// the import. With `symbols`, also one `SymbolRef` per use of a name.
    ///
    /// The excluded documents, of files that `exclude` hides, are never read as
    /// sources: they only resolve the names that no checked document defines. Also
    /// collects `Facts::known` for the names in `watched`.
    ///
    /// Every reference is read with [`read`] first, so what follows is the same for
    /// every language: a module, or a name at the top level of one, is a dependency on
    /// the file that defines it; every name, members included, is a symbol use.
    pub fn facts(&self, symbols: bool, watched: &BTreeSet<String>) -> Facts {
        self.facts_of(
            self.names.documents,
            &self.checked,
            &self.hidden,
            symbols,
            watched,
        )
    }

    /// The dependencies of the excluded documents, read as sources: the definitions
    /// of the checked documents resolve only what no excluded one defines.
    pub fn excluded_facts(&self) -> Facts {
        self.facts_of(
            self.names.excluded,
            &self.hidden,
            &self.checked,
            false,
            &BTreeSet::new(),
        )
    }

    /// The external module a module, or a name at the top level of one, that no
    /// file of the project defines is in, as `reference` to it names it: `None` for
    /// a symbol of the project's own package that no document defines (a relative
    /// import in Python, a JSON file scip-typescript does not index), and for an
    /// unnamed package whose string the reference does not stand on.
    fn external(&self, name: &Name, reference: &Reference) -> Option<String> {
        if name.reading.unnamed {
            // Only the string of the import names an unnamed package; a name bound
            // to one of its modules (`z` of `import { z } from "zod"`) stands on a
            // line whose string already did.
            return reference
                .spelled
                .clone()
                .filter(|module| !module.is_empty());
        }
        let own = name
            .package
            .as_ref()
            .is_some_and(|package| self.own.contains(package));
        (!own).then(|| name.external.clone())
    }

    /// What the index holds on the specifier of `import`, written in `document`,
    /// any document the index read, whose text is `lines`: a module it resolved stands on the text
    /// of the path, with or without its quotes, in every language, or on the text
    /// the import says it is recorded on ([`Import::recorded_on`]). An alias, or
    /// another import on the same line, stands on other text, so it resolves
    /// nothing here; nor does a reference whose range is not on one line, since
    /// what it stands on is unknown. A module resolves it only when it is taken
    /// where the dependencies are: to a file of the project, checked or excluded,
    /// or to an external module; a name at the top level of a file of the project
    /// resolves it too (Python's `from . import VERSION`). Neither does when it is
    /// the importing file itself.
    pub fn resolution(&self, document: &Document, lines: &[&str], import: &Import) -> Resolution {
        let text = import.line.checked_sub(1).and_then(|line| lines.get(line));
        let lookup = Lookup {
            mine: &self.checked,
            others: &self.hidden,
        };
        let mut found = Resolution::Nothing;
        for reference in &document.references {
            let on_path = reference.line == import.line
                && reference.columns.is_some_and(|columns| {
                    text.and_then(|text| between(text, columns, document.encoding))
                        .is_some_and(|spelled| {
                            unquoted(spelled).unwrap_or(spelled) == import.recorded_text()
                        })
                });
            let Some(name) = self.names.get(&reference.symbol).filter(|_| on_path) else {
                continue;
            };
            let file = lookup.file_of(name);
            match name.shape {
                // The file itself is no target of its own import.
                _ if file == Some(document.path.as_str()) => {}
                Shape::Module if file.is_some() || self.external(name, reference).is_some() => {
                    return Resolution::Target;
                }
                Shape::TopLevel if file.is_some() => return Resolution::Target,
                Shape::Wildcard => found = Resolution::Wildcard,
                _ => {}
            }
        }
        found
    }

    /// The facts of `documents`, whose definitions are `mine`; the definitions of
    /// `others` resolve only the symbols that none of `mine` defines.
    fn facts_of(
        &self,
        documents: &'a [Document],
        mine: &Definitions<'a>,
        others: &Definitions<'a>,
        symbols: bool,
        watched: &BTreeSet<String>,
    ) -> Facts {
        let defined = &mine.defined;
        let reopened = &mine.reopened;
        let own = &self.own;
        let lookup = Lookup { mine, others };
        let defined_at = |symbol: &str| lookup.defined_at(symbol);
        // The one file that defines `symbol`: a module reopened in several files is no
        // one file's.
        let home = |symbol: &str| -> Option<&str> {
            (!reopened.contains(symbol))
                .then(|| defined_at(symbol))
                .flatten()
        };
        let in_project = |symbol: &str| defined_at(symbol).is_some();
        let file_of = |name: &Name| lookup.file_of(name);
        let checked: HashSet<&str> = documents.iter().map(|d| d.path.as_str()).collect();
        let mut deps = Vec::new();
        let mut uses = Vec::new();
        let mut unnamed = BTreeSet::new();
        // Records the use of `name` that `document` makes at `line`, when it is a use of
        // a name of another file, or the file among those whose uses are unjudged when the
        // index does not say which package the name is of.
        let mut record_use =
            |document: &Document, its_own: &HashSet<&str>, name: &Name, line: usize| {
                let Some(called) = name.called.as_ref() else {
                    return;
                };
                let found = home(&name.symbol).or_else(|| name.owner.as_deref().and_then(home));
                // A name of this file, or a module it opens itself (`Shop` around its
                // classes), is no use of it.
                let own_name = found == Some(document.path.as_str())
                    || (name.shape != Shape::Member && its_own.contains(name.symbol.as_str()));
                if own_name {
                    return;
                }
                if found.is_none() && name.reading.unnamed {
                    unnamed.insert(document.path.clone());
                    return;
                }
                let target = match found {
                    Some(file) => Some(Target::Internal(Unit::from(file))),
                    None if in_project(&name.symbol)
                        || name.owner.as_deref().is_some_and(in_project) =>
                    {
                        None
                    }
                    None => Some(Target::External(name.external.clone())),
                };
                let specifier = qualifier_in(called, found);
                uses.push(SymbolRef {
                    file: PathBuf::from(&document.path),
                    from: unit_at(document, line),
                    line,
                    method: called.method(&specifier),
                    specifier,
                    name: called.name.clone(),
                    target,
                });
            };
        // What the index defines is read as if a file of no project referenced it, so
        // that a definition no one uses still tells what a name can mean.
        let definitions = (symbols && !watched.is_empty()).then(|| Document {
            path: String::new(),
            references: defined
                .keys()
                .chain(others.defined.keys().filter(|s| !defined.contains_key(*s)))
                .enumerate()
                .map(|(line, symbol)| Reference {
                    line: line + 1,
                    columns: None,
                    symbol: (*symbol).to_string(),
                    spelled: None,
                })
                .collect(),
            ..Document::default()
        });
        for document in documents.iter().chain(definitions.as_ref()) {
            let read: Vec<(&Reference, &Name)> = document
                .references
                .iter()
                .filter_map(|reference| Some((reference, self.names.get(&reference.symbol)?)))
                .collect();
            // The file of the project each module, or name at the top level of one, is in.
            let files: Vec<Option<&str>> = read.iter().map(|(_, name)| file_of(name)).collect();
            // An augmentation, a declaration of the project merged into an external name (a
            // TypeScript `declare module "pkg"` of a package), stands on the range of the
            // name the code writes, beside that external name of the same shape. What the
            // code writes is the external name, so the augmentation is no dependency on its
            // file, nor evidence of one; it stays a symbol use.
            let written_at = |reference: &Reference, name: &Name| {
                let columns = reference.columns?;
                matches!(name.shape, Shape::Module | Shape::TopLevel).then_some((
                    reference.line,
                    columns,
                    name.shape,
                ))
            };
            // Where an external module, or a name at the top level of one, is written.
            let external_at: HashSet<(usize, (usize, usize), Shape)> = read
                .iter()
                .zip(&files)
                .filter(|((_, name), file)| {
                    file.is_none() && name.package.as_ref().is_none_or(|p| !own.contains(p))
                })
                .filter_map(|((reference, name), _)| written_at(reference, name))
                .collect();
            let merged: Vec<bool> = read
                .iter()
                .zip(&files)
                .map(|((reference, name), file)| {
                    file.is_some()
                        && written_at(reference, name).is_some_and(|at| external_at.contains(&at))
                })
                .collect();
            // Every line that refers to each other file, before any is left out below: a
            // re-export need not be the first line that refers to what it re-exports.
            let mut mentions: HashMap<&str, Vec<Mention>> = HashMap::new();
            // The other files whose modules each line names.
            let mut modules_at: HashMap<usize, BTreeSet<&str>> = HashMap::new();
            // The lines that write a name of another file, and the modules those names
            // are in: what tells a module the file writes a name through from a bare one.
            let mut name_lines: HashSet<usize> = HashSet::new();
            let mut name_modules: HashSet<ModulePath> = HashSet::new();
            // Each module named, by its file, line, package and path.
            let mut named_modules: Vec<(&str, usize, ModulePath)> = Vec::new();
            // The module of the names each other file defines, and the file of each
            // module: where they differ, the module is of several files (a Go package).
            let mut module_of: HashMap<&str, ModulePath> = HashMap::new();
            let mut file_of_module: HashMap<ModulePath, &str> = HashMap::new();
            for (((reference, name), file), merged) in read.iter().zip(&files).zip(&merged) {
                let Some(file) = file.filter(|file| *file != document.path && !merged) else {
                    continue;
                };
                let module = name.shape == Shape::Module;
                let path = (&name.package, name.path.as_slice());
                match name.shape {
                    Shape::TopLevel => {
                        name_lines.insert(reference.line);
                        name_modules.insert(path);
                        module_of.insert(file, path);
                    }
                    Shape::Module => {
                        modules_at.entry(reference.line).or_default().insert(file);
                        named_modules.push((file, reference.line, path));
                        file_of_module.insert(path, file);
                    }
                    _ => {}
                }
                mentions.entry(file).or_default().push(Mention {
                    line: reference.line,
                    symbol: (!module).then(|| name.symbol.clone()),
                    called: called_in(name, file),
                    bare: module,
                });
            }
            // A module is bare unless a line that names it writes a name, or the file
            // writes a name of it.
            let through_names: HashSet<&str> = named_modules
                .iter()
                .filter(|(_, line, path)| name_lines.contains(line) || name_modules.contains(path))
                .map(|(file, _, _)| *file)
                .collect();
            for (file, found) in mentions.iter_mut() {
                if through_names.contains(file) {
                    for mention in found.iter_mut() {
                        mention.bare = false;
                    }
                }
            }
            for found in mentions.values_mut() {
                found.sort();
                found.dedup();
            }
            // The other files whose modules a line names and nothing else: the shape of a
            // glob import (`use chunk::*`), which brings in names that later lines write
            // with no module of their own.
            let names_at: HashSet<usize> = read
                .iter()
                .filter(|(_, name)| name.shape != Shape::Module)
                .map(|(reference, _)| reference.line)
                .collect();
            let globbed: BTreeSet<&str> = modules_at
                .iter()
                .filter(|(line, _)| !names_at.contains(line))
                .flat_map(|(_, files)| files.iter().copied())
                .collect();
            // Where a line also references each module around the path it writes, only the
            // deepest one of a package is what it depends on.
            let mut paths: HashMap<usize, Vec<&Name>> = HashMap::new();
            for (reference, name) in &read {
                if name.shape == Shape::Module && name.reading.enclosing {
                    paths.entry(reference.line).or_default().push(name);
                }
            }
            let shadowed = |name: &Name, line: usize| {
                name.shape == Shape::Module
                    && name.reading.enclosing
                    && paths.get(&line).is_some_and(|all| {
                        all.iter().any(|other| {
                            other.package == name.package
                                && other.path.len() > name.path.len()
                                && other.path.starts_with(&name.path)
                        })
                    })
            };
            // The modules the file names, by package and path, at the first line that does: a
            // name of one adds nothing, whichever of its files defines it (a Go package). A
            // re-exported name has a module of its own, which the line does not name.
            let mut named: HashMap<(&Option<String>, &[String]), usize> = HashMap::new();
            for ((reference, name), merged) in read.iter().zip(&merged) {
                if name.shape == Shape::Module && !merged {
                    let line = named
                        .entry((&name.package, &name.path))
                        .or_insert(reference.line);
                    *line = (*line).min(reference.line);
                }
            }
            // Each dependency at its first line.
            let mut first: BTreeMap<DepKey, usize> = BTreeMap::new();
            let its_own: HashSet<&str> = document.definitions.iter().map(String::as_str).collect();
            for (((reference, name), file), merged) in read.iter().zip(&files).zip(&merged) {
                if shadowed(name, reference.line) {
                    continue;
                }
                if symbols {
                    record_use(document, &its_own, name, reference.line);
                }
                if *merged || !matches!(name.shape, Shape::Module | Shape::TopLevel) {
                    continue;
                }
                if name.shape == Shape::TopLevel
                    && named
                        .get(&(&name.package, name.path.as_slice()))
                        .is_some_and(|line| *line <= reference.line)
                {
                    continue;
                }
                let internal = *file;
                if internal == Some(document.path.as_str()) {
                    continue;
                }
                // An external name is reached through its module, which the line names too; so
                // is a name an excluded file defines (a generated `api.pb.go` beside `api.go`):
                // only a module the code names fails as excluded.
                if name.shape == Shape::TopLevel
                    && internal.is_none_or(|file| !checked.contains(file))
                {
                    continue;
                }
                let module = match internal {
                    Some(_) => String::new(),
                    None => match self.external(name, reference) {
                        Some(module) => module,
                        None => continue,
                    },
                };
                let part = part_at(document, reference.line);
                let written = match name.shape {
                    Shape::Module => Written::Module,
                    _ => Written::Name,
                };
                let key = DepKey {
                    internal,
                    external: module,
                    part,
                    written,
                };
                let line = first.entry(key).or_insert(reference.line);
                *line = (*line).min(reference.line);
            }
            // What each alias the file writes stands for: a dependency on, and a use of,
            // each module or top-level name of a checked file, at the line of the use. The
            // file does not name them, so they are kept apart from what it names.
            let mut by_alias: BTreeMap<DepKey, BTreeSet<Aliased>> = BTreeMap::new();
            // The names the index defines, read as one file of no path, use no alias.
            let writes = match document.path.is_empty() {
                true => &[][..],
                false => &document.references[..],
            };
            for reference in writes {
                let stood: Vec<(&Name, Vec<AliasDefinition>)> = self
                    .stood_for(&reference.symbol)
                    .into_iter()
                    .filter_map(|(symbol, chain)| Some((self.names.get(symbol)?, chain)))
                    .filter(|(name, _)| matches!(name.shape, Shape::Module | Shape::TopLevel))
                    .collect();
                for (name, chain) in &stood {
                    // The module of a name the alias writes only qualifies it (`blob` of
                    // `blob.Block`), whichever of its files the module resolves to.
                    let qualifies = name.shape == Shape::Module
                        && stood.iter().any(|(other, _)| {
                            other.shape == Shape::TopLevel
                                && other.package == name.package
                                && other.path == name.path
                        });
                    // An external name is reached through its module, which the alias's
                    // own file imports.
                    let Some(file) = file_of(name).filter(|_| !qualifies) else {
                        continue;
                    };
                    if symbols {
                        record_use(document, &its_own, name, reference.line);
                    }
                    // A module the file names no later already is a dependency of it.
                    let named_before = named
                        .get(&(&name.package, name.path.as_slice()))
                        .is_some_and(|line| *line <= reference.line);
                    if file == document.path || !checked.contains(file) || named_before {
                        continue;
                    }
                    let key = DepKey {
                        internal: Some(file),
                        external: String::new(),
                        part: part_at(document, reference.line),
                        written: Written::Name,
                    };
                    let module = name.shape == Shape::Module;
                    by_alias.entry(key).or_default().insert(Aliased {
                        line: reference.line,
                        chain: chain.clone(),
                        symbol: (!module).then(|| name.symbol.clone()),
                        called: called_in(name, file),
                    });
                }
            }
            // A name of a file that the file also names as a module no later adds nothing,
            // whatever spelling each has (Python's `src.shop.db` and `shop.db`).
            let covered = |key: &DepKey, line: usize| {
                let named = DepKey {
                    written: Written::Module,
                    ..key.clone()
                };
                key.written == Written::Name && first.get(&named).is_some_and(|at| *at <= line)
            };
            let mut lines: BTreeMap<DepKey, usize> = first.clone();
            for (key, used) in &by_alias {
                let line = used.iter().map(|aliased| aliased.line).min().unwrap_or(0);
                let at = lines.entry(key.clone()).or_insert(line);
                *at = (*at).min(line);
            }
            let kept: Vec<(DepKey, usize)> = lines
                .into_iter()
                .filter(|(key, line)| !covered(key, *line))
                .collect();
            // The names of the other files of a module the file names, which no
            // dependency of their own stands for, by the file the module is.
            let targeted: HashSet<&str> = kept.iter().filter_map(|(key, _)| key.internal).collect();
            let mut elsewhere: HashMap<&str, Vec<Elsewhere>> = HashMap::new();
            for (file, module) in &module_of {
                let Some(module_file) = file_of_module.get(module) else {
                    continue;
                };
                if module_file == file || targeted.contains(file) || !checked.contains(file) {
                    continue;
                }
                let names = mentions.get(file).into_iter().flatten();
                elsewhere.entry(module_file).or_default().extend(
                    names
                        .filter(|mention| mention.symbol.is_some())
                        .map(|mention| Elsewhere {
                            file: file.to_string(),
                            mention: mention.clone(),
                        }),
                );
            }
            for found in elsewhere.values_mut() {
                found.sort();
            }
            for (key, line) in kept {
                // Only a line that writes the name itself says what it came through.
                let written_here = first.get(&key) == Some(&line);
                let aliased: Vec<Aliased> = by_alias
                    .get(&key)
                    .map(|used| used.iter().cloned().collect())
                    .unwrap_or_default();
                let (specifier, target) = match key.internal {
                    Some(path) => (path.to_string(), Target::Internal(Unit::from(path))),
                    None => (key.external.clone(), Target::External(key.external)),
                };
                // The modules the line names, where it names one besides the name's own;
                // else those the file imports whole, by a glob, on lines of their own.
                let through = match (key.written, key.internal) {
                    (Written::Name, Some(path)) if written_here => {
                        let others = |files: &BTreeSet<&str>| -> Vec<String> {
                            files
                                .iter()
                                .filter(|file| **file != path)
                                .map(|file| file.to_string())
                                .collect()
                        };
                        let on_line = modules_at.get(&line).map(others).unwrap_or_default();
                        let globs = others(&globbed);
                        match (on_line.is_empty(), globs.is_empty()) {
                            (false, _) => Through::Line(on_line),
                            (true, false) => Through::Glob(globs),
                            (true, true) => Through::Nothing,
                        }
                    }
                    _ => Through::Nothing,
                };
                let dep = Dep::module(&document.path, line, specifier, target);
                deps.push(Dep {
                    from: match key.part {
                        Some(part) => Unit::in_part(Path::new(&document.path), part.clone()),
                        None => dep.from.clone(),
                    },
                    written: key.written,
                    mentions: key
                        .internal
                        .and_then(|path| mentions.get(path))
                        .cloned()
                        .unwrap_or_default(),
                    through,
                    aliased,
                    elsewhere: match (key.written, key.internal) {
                        (Written::Module, Some(path)) => {
                            elsewhere.get(path).cloned().unwrap_or_default()
                        }
                        _ => Vec::new(),
                    },
                    ..dep
                });
            }
        }
        let known_whole = !unnamed.remove("");
        deps.retain(|dep| !dep.file.as_os_str().is_empty());
        route(&mut deps);
        let mut known = Vec::new();
        uses.retain(|used| {
            if !used.file.as_os_str().is_empty() {
                return true;
            }
            known.push(used.clone());
            false
        });
        let known = known
            .iter()
            .chain(&uses)
            .flat_map(|used| {
                let method = used.method.clone();
                std::iter::once((used.specifier.clone(), used.name.clone())).chain(method)
            })
            .filter(|(_, name)| watched.contains(name))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Facts {
            deps,
            symbols: uses,
            unnamed,
            known,
            known_whole,
        }
    }
}

/// A module by its package and path, as a name of it gives them.
type ModulePath<'a> = (&'a Option<String>, &'a [String]);

/// What the symbol rules call `name`, a name defined in `file`: `None` for a module,
/// and for a name the index gives no call of.
fn called_in(name: &Name, file: &str) -> Option<crate::lang::Qualified> {
    let called = name
        .called
        .as_ref()
        .filter(|_| name.shape != Shape::Module)?;
    Some(crate::lang::Qualified {
        qualifier: qualifier_in(called, Some(file)),
        name: called.name.clone(),
    })
}

/// Follows each name a file writes through the modules its line names, or the file
/// imports whole, that re-export it, to the file that defines it. A module its line
/// names re-exports the name where it refers to that file, by the name or by its
/// module (`export *`); a module imported whole only where it writes the name, since
/// the line that imports it may as well import one name of its own. The name came on
/// from the modules the re-exporting line names in turn. A module that does not refer
/// to the file passes on what the modules it re-exports whole (`export * from
/// "./inner"`) do. Each module is followed once on a way.
fn route(deps: &mut [Dep]) {
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    for (at, dep) in deps.iter().enumerate() {
        if matches!(dep.target, Target::Internal(_)) {
            by_file
                .entry(relpath::slash(&dep.file))
                .or_default()
                .push(at);
        }
    }
    let routes: Vec<Vec<Vec<Hop>>> = deps
        .iter()
        .map(|dep| match (&dep.written, &dep.target) {
            (Written::Name, Target::Internal(unit)) if !dep.through.files().is_empty() => {
                let reexports = Reexports {
                    deps,
                    by_file: &by_file,
                    target: unit.key(),
                    symbols: dep
                        .mentions
                        .iter()
                        .filter_map(|mention| mention.symbol.as_deref())
                        .collect(),
                };
                let by_name_alone = matches!(dep.through, Through::Glob(_));
                let mut on_the_way = HashSet::from([relpath::slash(&dep.file)]);
                reexports.ways(dep.through.files(), by_name_alone, &mut on_the_way)
            }
            _ => Vec::new(),
        })
        .collect();
    for (dep, routes) in deps.iter_mut().zip(routes) {
        dep.routes = routes;
    }
}

/// The re-exports of a name of `symbols`, defined in `target`, among `deps`.
struct Reexports<'a> {
    deps: &'a [Dep],
    /// The internal dependencies of each file, by index into `deps`.
    by_file: &'a HashMap<String, Vec<usize>>,
    target: String,
    symbols: BTreeSet<&'a str>,
}

impl Reexports<'_> {
    /// The ways the name came through each of `modules`, never through a module of
    /// `on_the_way`; with `by_name_alone`, only a module that writes the name refers
    /// to its file.
    fn ways(
        &self,
        modules: &[String],
        by_name_alone: bool,
        on_the_way: &mut HashSet<String>,
    ) -> Vec<Vec<Hop>> {
        let mut found = Vec::new();
        for module in modules {
            if !on_the_way.insert(module.clone()) {
                continue;
            }
            let theirs: Vec<&Dep> = self
                .by_file
                .get(module)
                .into_iter()
                .flatten()
                .map(|at| &self.deps[*at])
                .collect();
            found.extend(self.through(module, &theirs, by_name_alone, on_the_way));
            on_the_way.remove(module);
        }
        found
    }

    /// The ways from `module`, whose dependencies are `theirs`: from the line by
    /// which the name leaves it on; or, when it does not refer to the file of the
    /// name, through the modules it re-exports whole.
    fn through(
        &self,
        module: &str,
        theirs: &[&Dep],
        by_name_alone: bool,
        on_the_way: &mut HashSet<String>,
    ) -> Vec<Vec<Hop>> {
        let on_target: Vec<&Dep> = theirs
            .iter()
            .copied()
            .filter(|other| matches!(&other.target, Target::Internal(unit) if unit.key() == self.target))
            .collect();
        let refers = on_target
            .iter()
            .flat_map(|other| &other.mentions)
            .filter(|mention| match mention.symbol.as_deref() {
                Some(symbol) => self.symbols.contains(symbol),
                None => !by_name_alone,
            })
            .map(|mention| mention.line)
            .min();
        if let Some(line) = refers {
            let hop = Hop {
                file: module.to_string(),
                line,
            };
            let further: Vec<Vec<Hop>> = on_target
                .iter()
                .filter(|other| other.written == Written::Name)
                .flat_map(|other| {
                    let by_name_alone = matches!(other.through, Through::Glob(_));
                    self.ways(other.through.files(), by_name_alone, on_the_way)
                })
                .collect();
            if further.is_empty() {
                return vec![vec![hop]];
            }
            return further.into_iter().map(|way| after(&hop, way)).collect();
        }
        // The lines of `module` that name a module and nothing else: a re-export of it
        // whole, or an import of the module itself.
        let named: HashSet<usize> = theirs
            .iter()
            .flat_map(|other| &other.mentions)
            .filter(|mention| mention.symbol.is_some())
            .map(|mention| mention.line)
            .collect();
        let mut wholes: Vec<(usize, String)> = theirs
            .iter()
            .filter_map(|other| match &other.target {
                Target::Internal(unit) => Some((unit.key(), &other.mentions)),
                Target::External(_) => None,
            })
            .flat_map(|(file, mentions)| {
                mentions
                    .iter()
                    .filter(|mention| mention.symbol.is_none() && !named.contains(&mention.line))
                    .map(move |mention| (mention.line, file.clone()))
            })
            .collect();
        wholes.sort();
        wholes.dedup();
        let mut found = Vec::new();
        for (line, file) in wholes {
            let hop = Hop {
                file: module.to_string(),
                line,
            };
            found.extend(
                self.ways(std::slice::from_ref(&file), by_name_alone, on_the_way)
                    .into_iter()
                    .map(|way| after(&hop, way)),
            );
        }
        found
    }
}

/// `way`, after `hop`.
fn after(hop: &Hop, way: Vec<Hop>) -> Vec<Hop> {
    let mut whole = vec![hop.clone()];
    whole.extend(way);
    whole
}

impl Documents {
    pub fn merged(parts: Vec<Vec<Document>>) -> Result<Self> {
        let mut by_path: BTreeMap<String, Document> = BTreeMap::new();
        for document in parts.into_iter().flatten() {
            match by_path.get_mut(&document.path) {
                Some(seen) => {
                    seen.references.extend(document.references);
                    seen.definitions.extend(document.definitions);
                    seen.aliases.extend(document.aliases);
                }
                None => {
                    by_path.insert(document.path.clone(), document);
                }
            }
        }
        if by_path.is_empty() {
            bail!("the indexers produced no documents");
        }
        Ok(Self(by_path.into_values().collect()))
    }
}

/// For the tests: [`Indexed::flow`] of `documents`.
#[cfg(test)]
pub(crate) fn flow(documents: &[Document]) -> Flow {
    let names = Names::of(documents, &[]);
    Indexed::of(&names).flow()
}

/// The method of `scoped` that `symbol` is or is defined in, the innermost: a
/// parameter or a local an indexer names globally (`scale().(x)`) is part of its method.
fn within_scope<'a>(symbol: &'a str, scoped: &HashSet<&str>) -> Option<&'a str> {
    if scoped.contains(symbol) {
        return Some(symbol);
    }
    symbol
        .rmatch_indices(").")
        .map(|(at, _)| &symbol[..at + 2])
        .find(|prefix| scoped.contains(prefix))
}

impl Indexed<'_> {
    /// The references of `documents`, each from the definition that holds it to the
    /// definition it names. A symbol that more than one file defines (a Ruby module
    /// reopened in each file under it) names no one place, so its references are left to
    /// the dependencies, which keep reaching a whole file.
    pub fn flow(&self) -> Flow {
        let documents = self.names.documents;
        let defined = &self.checked.defined;
        let reopened = &self.checked.reopened;
        let mut scoped: HashSet<&str> = HashSet::new();
        let mut flow = Flow::default();
        for document in documents {
            for scope in &document.scopes {
                scoped.insert(&scope.symbol);
                flow.names
                    .insert(scope.symbol.clone(), symbol::shown_name(&scope.symbol));
            }
            if !document.scopes.is_empty() {
                flow.scopes
                    .insert(document.path.clone(), document.scopes.clone());
            }
        }
        // A method reaches the code of its own file that is in none: the class body and the
        // constants that run when the file loads.
        let own: Vec<Edge> = flow
            .scopes
            .iter()
            .flat_map(|(file, scopes)| {
                scopes.iter().map(|scope| Edge {
                    from: Node {
                        file: file.clone(),
                        scope: Some(scope.symbol.clone()),
                    },
                    to: Node::file(file),
                    line: scope.first,
                })
            })
            .collect();
        flow.edges.extend(own);
        for document in documents {
            // The files whose method a line names: such a line reaches that method, and a
            // reference to the class around it (`Document` of `Document.host`) adds nothing.
            let named: HashSet<(usize, &str)> = document
                .references
                .iter()
                .filter(|r| {
                    scoped.contains(r.symbol.as_str()) && !reopened.contains(r.symbol.as_str())
                })
                .filter_map(|r| defined.get(r.symbol.as_str()).map(|file| (r.line, *file)))
                .collect();
            for reference in &document.references {
                let symbol = reference.symbol.as_str();
                let Some(&file) = defined.get(symbol) else {
                    continue;
                };
                if reopened.contains(symbol) {
                    continue;
                }
                let targets = if scoped.contains(symbol) {
                    vec![Node {
                        file: file.to_string(),
                        scope: Some(symbol.to_string()),
                    }]
                } else if symbol.ends_with('/') || named.contains(&(reference.line, file)) {
                    // An import runs the code of the file that is in no method; the names it
                    // brings are reached by their own references.
                    vec![Node::file(file)]
                } else {
                    // A class or constant used with no method named (an opaque receiver):
                    // any of the file's methods may be what runs.
                    flow.nodes_of(file)
                };
                // A module named on its own (an import) names no definition.
                if !symbol.ends_with('/') {
                    flow.uses.push(Use {
                        file: document.path.clone(),
                        line: reference.line,
                        to: match within_scope(symbol, &scoped) {
                            Some(scope) => Node {
                                file: file.to_string(),
                                scope: Some(scope.to_string()),
                            },
                            None => Node::file(file),
                        },
                    });
                }
                for from in flow.owners(&document.path, reference.line) {
                    for to in &targets {
                        if from != *to {
                            flow.edges.push(Edge {
                                from: from.clone(),
                                to: to.clone(),
                                line: reference.line,
                            });
                        }
                    }
                }
            }
        }
        flow
    }
}

/// `symbol` in the shape every language shares, as the entry of the language whose
/// indexer writes its scheme reads it. `None` for a symbol that does not split, or
/// of a scheme no entry declares: nothing is made of it.
pub(crate) fn read(symbol: &str) -> Option<Name> {
    let parsed = split(symbol)?;
    let language = registry::of_scheme(&parsed.scheme)?;
    Some(match language.indexer()?.symbols {
        Symbols::Modules {
            qualifier,
            external,
        } => modules(
            symbol,
            &parsed,
            language.member_separator,
            qualifier,
            external,
        ),
        Symbols::Classes {
            separator,
            constructor,
            owned,
        } => classes(
            symbol,
            &parsed,
            separator,
            constructor,
            owned.then(|| parsed.owner()),
        ),
        Symbols::Own(read) => read(symbol, &parsed),
    })
}

/// The qualifier of the name as the symbol rules write it: as the index spells it,
/// else the module path of `file`, the file that defines it, when one does.
fn qualifier_in(called: &Called, file: Option<&str>) -> String {
    match (&called.qualifier, file) {
        (Qualifier::Spelled(qualifier), _) => qualifier.clone(),
        (Qualifier::File(_), Some(file)) => registry::module_path(file).to_string(),
        (Qualifier::File(otherwise), None) => otherwise.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, defs: &[&str], refs: &[(usize, &str)]) -> Document {
        Document {
            path: path.to_string(),
            definitions: defs.iter().map(|s| s.to_string()).collect(),
            types: Vec::new(),
            scopes: Vec::new(),
            parts: Vec::new(),
            references: refs
                .iter()
                .map(|(line, symbol)| Reference {
                    line: *line,
                    columns: None,
                    symbol: symbol.to_string(),
                    spelled: None,
                })
                .collect(),
            ..Document::default()
        }
    }

    const DB: &str = "scip-go gomod example.com/app . `example.com/app/internal/db`/";
    const FMT: &str = "scip-go gomod github.com/golang/go/src go1.25 fmt/";

    /// The case of #104 in one scheme: `blob` defines `Block` with a field and a
    /// method, `chunk` a function that returns one, and `gw` calls that function and
    /// uses the member on what it returns, writing no name of `blob`. The symbols are
    /// those the indexer emits (`docs/research/symbol-use-dependencies.md`).
    struct Case {
        blob: Vec<String>,
        chunk: Vec<String>,
        /// The references of `gw`: line 1 names `chunk`, line 2 calls its function,
        /// line 3 uses the members of the value it returned.
        gw: Vec<(usize, String)>,
    }

    fn case(prefix: &str, blob: &[&str], chunk: &[&str], gw: &[(usize, &str)]) -> Case {
        let full = |s: &&str| format!("{prefix}{s}");
        Case {
            blob: blob.iter().map(full).collect(),
            chunk: chunk.iter().map(full).collect(),
            gw: gw.iter().map(|(line, s)| (*line, full(s))).collect(),
        }
    }

    fn cases() -> Vec<(&'static str, Case)> {
        vec![
            (
                "java",
                case(
                    "scip-java maven maven/fx/app 1.0 ",
                    &[
                        "fx/blob/Block#",
                        "fx/blob/Block#size().",
                        "fx/blob/Block#len.",
                    ],
                    &["fx/chunk/Chunk#", "fx/chunk/Chunk#make()."],
                    &[
                        (1, "fx/chunk/Chunk#"),
                        (2, "fx/chunk/Chunk#"),
                        (2, "fx/chunk/Chunk#make()."),
                        (3, "fx/blob/Block#size()."),
                        (3, "fx/blob/Block#len."),
                    ],
                ),
            ),
            (
                "csharp",
                case(
                    "scip-dotnet nuget . . ",
                    &["Blob/Block#", "Blob/Block#Size().", "Blob/Block#Len."],
                    &["Chunk/Chunk#", "Chunk/Chunk#Make()."],
                    &[
                        (1, "Chunk/Chunk#"),
                        (2, "Chunk/"),
                        (2, "Chunk/Chunk#"),
                        (2, "Chunk/Chunk#Make()."),
                        (3, "Blob/Block#Size()."),
                        (3, "Blob/Block#Len."),
                    ],
                ),
            ),
            (
                "ruby",
                case(
                    "scip-ruby gem fx 0 ",
                    &[
                        "Blob#",
                        "Blob#Block#",
                        "Blob#Block#size().",
                        "Blob#Block#len().",
                    ],
                    &["Chunk#", "`<Class:Chunk>`#make()."],
                    &[
                        (1, "Chunk#"),
                        (2, "Chunk#"),
                        (2, "`<Class:Chunk>`#make()."),
                        (3, "Blob#Block#size()."),
                        (3, "Blob#Block#len()."),
                    ],
                ),
            ),
            (
                "rust",
                case(
                    "rust-analyzer cargo ",
                    &[
                        "blob 0.1.0 crate/",
                        "blob 0.1.0 Block#",
                        "blob 0.1.0 Block#len.",
                        "blob 0.1.0 impl#[Block]size().",
                    ],
                    &["chunk 0.1.0 crate/", "chunk 0.1.0 make()."],
                    &[
                        (1, "chunk 0.1.0 crate/"),
                        (2, "chunk 0.1.0 crate/"),
                        (2, "chunk 0.1.0 make()."),
                        (3, "blob 0.1.0 impl#[Block]size()."),
                        (3, "blob 0.1.0 Block#len."),
                    ],
                ),
            ),
            (
                "typescript",
                case(
                    "scip-typescript npm fx 1.0.0 ",
                    &[
                        "src/blob/`index.ts`/",
                        "src/blob/`index.ts`/Block#",
                        "src/blob/`index.ts`/Block#size().",
                        "src/blob/`index.ts`/Block#len.",
                    ],
                    &["src/chunk/`index.ts`/", "src/chunk/`index.ts`/make()."],
                    &[
                        (1, "src/chunk/`index.ts`/"),
                        (1, "src/chunk/`index.ts`/make()."),
                        (2, "src/chunk/`index.ts`/make()."),
                        (3, "src/blob/`index.ts`/Block#size()."),
                        (3, "src/blob/`index.ts`/Block#len."),
                    ],
                ),
            ),
            (
                "python",
                case(
                    "scip-python python fx 0.1 ",
                    &[
                        "`fx.blob`/__init__:",
                        "`fx.blob`/Block#",
                        "`fx.blob`/Block#size().",
                        "`fx.blob`/Block#len.",
                    ],
                    &["`fx.chunk`/__init__:", "`fx.chunk`/make()."],
                    &[
                        (1, "`fx.chunk`/__init__:"),
                        (1, "`fx.chunk`/make()."),
                        (2, "`fx.chunk`/make()."),
                        (3, "`fx.blob`/Block#size()."),
                        (3, "`fx.blob`/Block#len."),
                    ],
                ),
            ),
            (
                "go",
                case(
                    "scip-go gomod ex.com/fx . ",
                    &[
                        "`ex.com/fx/blob`/",
                        "`ex.com/fx/blob`/Block#",
                        "`ex.com/fx/blob`/Block#Size().",
                        "`ex.com/fx/blob`/Block#Len.",
                    ],
                    &["`ex.com/fx/chunk`/", "`ex.com/fx/chunk`/Make()."],
                    &[
                        (1, "`ex.com/fx/chunk`/"),
                        (2, "`ex.com/fx/chunk`/"),
                        (2, "`ex.com/fx/chunk`/Make()."),
                        (3, "`ex.com/fx/blob`/Block#Size()."),
                        (3, "`ex.com/fx/blob`/Block#Len."),
                    ],
                ),
            ),
        ]
    }

    /// The dependencies of `gw` and the files its symbol uses point at, by line.
    type Read = (Vec<(usize, Target)>, Vec<(usize, Option<Target>)>);

    fn read_case(case: &Case) -> Read {
        let refs: Vec<(usize, &str)> = case.gw.iter().map(|(l, s)| (*l, s.as_str())).collect();
        let blob: Vec<&str> = case.blob.iter().map(String::as_str).collect();
        let chunk: Vec<&str> = case.chunk.iter().map(String::as_str).collect();
        let docs = [
            doc("blob", &blob, &[]),
            doc("chunk", &chunk, &[]),
            doc("gw", &[], &refs),
        ];
        let read = facts(&docs, &[], true);
        let mut deps: Vec<_> = read
            .deps
            .iter()
            .map(|d| (d.line, d.target.clone()))
            .collect();
        deps.sort_by_key(|(line, target)| (*line, format!("{target:?}")));
        let mut uses: Vec<_> = read
            .symbols
            .iter()
            .map(|u| (u.line, u.target.clone()))
            .collect();
        uses.sort_by_key(|(line, target)| (*line, format!("{target:?}")));
        uses.dedup();
        (deps, uses)
    }

    #[test]
    fn a_member_on_a_value_another_module_returned_is_no_dependency_in_any_language() {
        let chunk = Target::Internal(Unit::from("chunk"));
        let blob = Target::Internal(Unit::from("blob"));
        for (language, case) in cases() {
            let (deps, uses) = read_case(&case);
            assert_eq!(deps, [(1, chunk.clone())], "{language}");
            // The member is still a use of a name, which the symbol rules read.
            assert!(
                uses.contains(&(3, Some(blob.clone()))),
                "{language}: {uses:?}"
            );
        }
    }

    /// The internal dependencies of the last document, by line and file.
    fn deps_of(docs: &[Document]) -> Vec<(usize, String)> {
        let last = &docs[docs.len() - 1].path;
        let mut found: Vec<_> = facts(docs, &[], false)
            .deps
            .into_iter()
            .filter(|d| d.file == Path::new(last))
            .map(|d| (d.line, d.specifier))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_name_reached_through_a_re_export_is_a_dependency_on_the_file_that_defines_it() {
        // Rust: `chunk` has `pub use blob::Block;`, and `gw` writes `chunk::Block` on line 6.
        let rust = "rust-analyzer cargo ";
        let rust_docs = [
            doc(
                "blob/src/lib.rs",
                &[
                    &format!("{rust}blob 0.1.0 crate/"),
                    &format!("{rust}blob 0.1.0 Block#"),
                ],
                &[],
            ),
            doc(
                "chunk/src/lib.rs",
                &[&format!("{rust}chunk 0.1.0 crate/")],
                &[],
            ),
            doc(
                "gw/src/lib.rs",
                &[],
                &[
                    (2, &format!("{rust}chunk 0.1.0 crate/")),
                    (6, &format!("{rust}chunk 0.1.0 crate/")),
                    (6, &format!("{rust}blob 0.1.0 Block#")),
                ],
            ),
        ];
        assert_eq!(
            deps_of(&rust_docs),
            [
                (2, "chunk/src/lib.rs".to_string()),
                (6, "blob/src/lib.rs".to_string())
            ]
        );
        // TypeScript: `export { Block } from "../blob"` in `chunk`, and
        // `import { Block } from "../chunk"` in `gw` on line 1, used on line 4.
        let ts = "scip-typescript npm fx 1.0.0 src/";
        let ts_docs = [
            doc(
                "src/blob/index.ts",
                &[
                    &format!("{ts}blob/`index.ts`/"),
                    &format!("{ts}blob/`index.ts`/Block#"),
                ],
                &[],
            ),
            doc(
                "src/chunk/index.ts",
                &[&format!("{ts}chunk/`index.ts`/")],
                &[],
            ),
            doc(
                "src/gw/index.ts",
                &[],
                &[
                    (1, &format!("{ts}chunk/`index.ts`/")),
                    (1, &format!("{ts}blob/`index.ts`/Block#")),
                    (4, &format!("{ts}blob/`index.ts`/Block#")),
                ],
            ),
        ];
        assert_eq!(
            deps_of(&ts_docs),
            [
                (1, "src/blob/index.ts".to_string()),
                (1, "src/chunk/index.ts".to_string())
            ]
        );
    }

    #[test]
    fn a_dependency_knows_every_line_that_refers_to_its_file_and_a_name_the_modules_on_its_line() {
        // `chunk` writes `blob::Block` in `make()` on line 1 and re-exports it on line 5;
        // `gw` writes `chunk::Block` on line 2.
        let rust = "rust-analyzer cargo ";
        let crate_of = |name: &str| format!("{rust}{name} 0.1.0 crate/");
        let block = format!("{rust}blob 0.1.0 Block#");
        let docs = [
            doc("blob/src/lib.rs", &[&crate_of("blob"), &block], &[]),
            doc(
                "chunk/src/lib.rs",
                &[&crate_of("chunk")],
                &[
                    (1, &crate_of("blob")),
                    (1, &block),
                    (5, &crate_of("blob")),
                    (5, &block),
                ],
            ),
            doc(
                "gw/src/lib.rs",
                &[],
                &[(2, &crate_of("chunk")), (2, &block)],
            ),
        ];
        let deps = facts(&docs, &[], false).deps;
        let of = |file: &str, target: &str| {
            deps.iter()
                .find(|d| d.file == Path::new(file) && d.specifier == target)
                .unwrap()
        };
        let mention = |line, symbol: Option<&str>| (line, symbol.map(str::to_string));
        let lines = |dep: &Dep| -> Vec<(usize, Option<String>)> {
            dep.mentions
                .iter()
                .map(|mention| (mention.line, mention.symbol.clone()))
                .collect()
        };
        // One dependency, at the first line, that still knows the re-export below it.
        let chunk = of("chunk/src/lib.rs", "blob/src/lib.rs");
        assert_eq!(chunk.line, 1);
        assert_eq!(
            lines(chunk),
            [
                mention(1, None),
                mention(1, Some(&block)),
                mention(5, None),
                mention(5, Some(&block)),
            ]
        );
        assert_eq!(chunk.through, Through::Nothing);
        let gw = of("gw/src/lib.rs", "blob/src/lib.rs");
        assert_eq!(lines(gw), [mention(2, Some(&block))]);
        assert_eq!(gw.through, Through::Line(vec!["chunk/src/lib.rs".into()]));
        // A module the code names is reached through nothing.
        assert_eq!(
            of("gw/src/lib.rs", "chunk/src/lib.rs").through,
            Through::Nothing
        );
    }

    #[test]
    fn a_name_is_its_own_dependency_unless_the_file_names_its_module_no_later() {
        // `billing/index.ts` re-exports `charge` of `billing/internal/charge.ts`. Line 1
        // imports it through the index, line 2 straight from its file; `Order` of
        // `orders/model.ts` is imported by name and module on line 3.
        let ts = "scip-typescript npm fx 1.0.0 src/";
        let charge = format!("{ts}billing/internal/`charge.ts`/charge.");
        let order = format!("{ts}orders/`model.ts`/Order#");
        let docs = [
            doc(
                "src/billing/index.ts",
                &[&format!("{ts}billing/`index.ts`/")],
                &[],
            ),
            doc(
                "src/billing/internal/charge.ts",
                &[&format!("{ts}billing/internal/`charge.ts`/"), &charge],
                &[],
            ),
            doc(
                "src/orders/model.ts",
                &[&format!("{ts}orders/`model.ts`/"), &order],
                &[],
            ),
            doc(
                "src/ui/page.ts",
                &[],
                &[
                    (1, &format!("{ts}billing/`index.ts`/")),
                    (1, &charge),
                    (2, &format!("{ts}billing/internal/`charge.ts`/")),
                    (2, &charge),
                    (3, &format!("{ts}orders/`model.ts`/")),
                    (3, &order),
                    (5, &charge),
                ],
            ),
        ];
        let mut found: Vec<_> = facts(&docs, &[], false)
            .deps
            .into_iter()
            .map(|d| (d.line, d.specifier, d.written))
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                (1, "src/billing/index.ts".to_string(), Written::Module),
                (
                    1,
                    "src/billing/internal/charge.ts".to_string(),
                    Written::Name
                ),
                (
                    2,
                    "src/billing/internal/charge.ts".to_string(),
                    Written::Module
                ),
                (3, "src/orders/model.ts".to_string(), Written::Module),
            ]
        );
    }

    #[test]
    fn a_name_of_a_module_the_file_names_adds_nothing_whatever_file_of_it_defines_it() {
        // A Go package of three files: `gw` imports it on line 3 and calls two of its
        // functions on line 6, each defined in another file of it.
        let go = "scip-go gomod ex.com/fx . `ex.com/fx/blob`/";
        let docs = [
            doc("blob/a.go", &[go], &[]),
            doc("blob/make.go", &[go, &format!("{go}Make().")], &[]),
            doc("blob/other.go", &[go, &format!("{go}Other().")], &[]),
            doc(
                "gw/gw.go",
                &[],
                &[
                    (3, go),
                    (6, go),
                    (6, &format!("{go}Make().")),
                    (6, &format!("{go}Other().")),
                ],
            ),
        ];
        assert_eq!(deps_of(&docs), [(3, "blob/a.go".to_string())]);
    }

    #[test]
    fn a_name_an_excluded_file_defines_is_no_dependency_of_its_own() {
        // `api` is `api.go` and a generated `api.pb.go` that `exclude` hides; `gw` imports
        // `api` on line 3 and uses `api.Request`, defined in the generated file, on line 6.
        let api = "scip-go gomod ex.com/fx . `ex.com/fx/api`/";
        let request = format!("{api}Request#");
        let sources = [
            doc("api/api.go", &[api], &[(4, &request)]),
            doc("gw/gw.go", &[], &[(3, api), (6, &request)]),
        ];
        let excluded = [doc("api/api.pb.go", &[api, &request], &[])];
        let mut found: Vec<_> = facts(&sources, &excluded, false)
            .deps
            .into_iter()
            .map(|d| (d.file.to_str().unwrap().to_string(), d.line, d.specifier))
            .collect();
        found.sort();
        assert_eq!(
            found,
            [("gw/gw.go".to_string(), 3, "api/api.go".to_string())]
        );
    }

    #[test]
    fn a_python_name_re_exported_by_an_init_is_a_dependency_on_its_module() {
        // As scip-python writes them: a module by its path from the working directory
        // (`src.app.blob`) in its definition and as imported (`app.chunk`) in a
        // reference, a name by the path of its definition in both. `app.chunk`
        // re-exports `Block`.
        let py = "scip-python python fx 0.1 ";
        let docs = [
            doc(
                "src/app/blob/__init__.py",
                &[
                    &format!("{py}`src.app.blob`/__init__:"),
                    &format!("{py}`src.app.blob`/Block#"),
                ],
                &[],
            ),
            doc(
                "src/app/chunk/__init__.py",
                &[&format!("{py}`src.app.chunk`/__init__:")],
                &[],
            ),
            doc(
                "src/app/gw/__init__.py",
                &[],
                &[
                    (1, &format!("{py}`app.chunk`/__init__:")),
                    (1, &format!("{py}`src.app.blob`/Block#")),
                    (3, &format!("{py}`src.app.blob`/Block#size().")),
                ],
            ),
        ];
        assert_eq!(
            deps_of(&docs),
            [
                (1, "src/app/blob/__init__.py".to_string()),
                (1, "src/app/chunk/__init__.py".to_string())
            ]
        );
    }

    #[test]
    fn a_constructor_that_stands_for_the_class_written_is_a_dependency_on_the_class() {
        // scip-java gives `new X()` only as `X#<init>().`, through a wildcard import or a
        // qualified name, and a nested class is reached through the class around it.
        let java = "scip-java maven maven/fx/app 1.0 fx/blob/";
        let java_docs = [
            doc(
                "Block.java",
                &[
                    &format!("{java}Block#"),
                    &format!("{java}Block#`<init>`()."),
                ],
                &[],
            ),
            doc(
                "Outer.java",
                &[&format!("{java}Outer#"), &format!("{java}Outer#Inner#")],
                &[],
            ),
            doc(
                "Gw.java",
                &[],
                &[
                    (1, "scip-java maven . . fx/blob/"),
                    (6, &format!("{java}Block#`<init>`().")),
                    (7, &format!("{java}Outer#Inner#`<init>`().")),
                    (8, &format!("{java}Block#size().")),
                ],
            ),
        ];
        assert_eq!(
            deps_of(&java_docs),
            [(6, "Block.java".to_string()), (7, "Outer.java".to_string())]
        );
        // scip-dotnet gives an attribute `[X]` only as `XAttribute#.ctor().`, the
        // project's own and the framework's in generated `AssemblyInfo.cs` alike.
        let dotnet = "scip-dotnet nuget . . Blob/";
        let dotnet_docs = [
            doc(
                "Blob/Mark.cs",
                &[
                    &format!("{dotnet}MarkAttribute#"),
                    &format!("{dotnet}MarkAttribute#`.ctor`()."),
                ],
                &[],
            ),
            doc(
                "Gw/Marked.cs",
                &[],
                &[
                    (3, &format!("{dotnet}MarkAttribute#`.ctor`().")),
                    (
                        4,
                        "scip-dotnet nuget System.Runtime 9.0.0.0 Reflection/AssemblyCompanyAttribute#`.ctor`().",
                    ),
                    (
                        5,
                        "scip-dotnet nuget System.Runtime 9.0.0.0 Reflection/AssemblyCompanyAttribute#Name.",
                    ),
                ],
            ),
        ];
        let deps = facts(&dotnet_docs, &[], false).deps;
        let found: Vec<_> = deps.iter().map(|d| (d.line, d.target.clone())).collect();
        assert_eq!(
            found,
            [
                (
                    4,
                    Target::External("Reflection.AssemblyCompanyAttribute".into())
                ),
                (3, Target::Internal(Unit::from("Blob/Mark.cs"))),
            ]
        );
    }

    #[test]
    fn only_a_deeper_module_of_the_same_crate_hides_the_one_around_it() {
        // `chunk` has `pub use blob::inner;`, and `gw` writes `chunk::inner::deep()`.
        let rust = "rust-analyzer cargo ";
        let docs = [
            doc(
                "blob/src/inner.rs",
                &[
                    &format!("{rust}blob 0.1.0 inner/"),
                    &format!("{rust}blob 0.1.0 inner/deep()."),
                ],
                &[],
            ),
            doc(
                "blob/src/lib.rs",
                &[&format!("{rust}blob 0.1.0 crate/")],
                &[],
            ),
            doc(
                "chunk/src/lib.rs",
                &[&format!("{rust}chunk 0.1.0 crate/")],
                &[],
            ),
            doc(
                "gw/src/lib.rs",
                &[],
                &[
                    (3, &format!("{rust}chunk 0.1.0 crate/")),
                    (3, &format!("{rust}blob 0.1.0 crate/")),
                    (3, &format!("{rust}blob 0.1.0 inner/")),
                    (3, &format!("{rust}blob 0.1.0 inner/deep().")),
                ],
            ),
        ];
        assert_eq!(
            deps_of(&docs),
            [
                (3, "blob/src/inner.rs".to_string()),
                (3, "chunk/src/lib.rs".to_string())
            ]
        );
    }

    /// A member of a Rust type (a variant, a method, an associated constant) is
    /// used through the type, as a Ruby method is through its class, so
    /// `forbid_symbol` can name it as `path::Type.member`.
    #[test]
    fn a_member_of_a_rust_type_is_named_through_the_type() {
        let rust = "rust-analyzer cargo ";
        let docs = [doc(
            "src/main.rs",
            &[&format!("{rust}app 0.1.0 crate/")],
            &[
                (3, &format!("{rust}std 1.0.0 sync/atomic/Ordering#")),
                (3, &format!("{rust}std 1.0.0 sync/atomic/Ordering#Relaxed.")),
                (4, &format!("{rust}db 0.1.0 client/Client#")),
                (4, &format!("{rust}db 0.1.0 client/Client#query().")),
            ],
        )];
        let uses: Vec<_> = facts(&docs, &[], true)
            .symbols
            .into_iter()
            .map(|u| {
                (
                    u.line,
                    format!("{}.{}", u.specifier, u.name),
                    u.method.map(|(owner, name)| format!("{owner}.{name}")),
                )
            })
            .collect();
        let method = |m: &str| Some(m.to_string());
        assert_eq!(
            uses,
            [
                (3, "std::sync::atomic.Ordering".into(), None),
                (
                    3,
                    "std::sync::atomic.Ordering".into(),
                    method("std::sync::atomic::Ordering.Relaxed")
                ),
                (4, "db::client.Client".into(), None),
                (
                    4,
                    "db::client.Client".into(),
                    method("db::client::Client.query")
                ),
            ]
        );
    }

    #[test]
    fn the_same_shapes_make_the_same_facts_whatever_the_scheme() {
        let all: Vec<_> = cases()
            .into_iter()
            .map(|(l, c)| (l, read_case(&c)))
            .collect();
        let (first, expected) = &all[0];
        for (language, read) in &all[1..] {
            // A file's module is a class in Java and C# and a constant in Ruby, so they name
            // `chunk` as a name too; the dependencies and the members are the same.
            assert_eq!(read.0, expected.0, "{language} and {first}");
            let members = |uses: &[(usize, Option<Target>)]| {
                uses.iter()
                    .filter(|(line, _)| *line == 3)
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                members(&read.1),
                members(&expected.1),
                "{language} and {first}"
            );
        }
    }

    #[test]
    fn a_use_in_a_cfg_test_part_is_a_dependency_of_that_part_apart_from_the_file() {
        let testkit = "rust-analyzer cargo testkit 0.1.0 crate/";
        let mut document = doc("src/blob_db.rs", &[], &[(1, testkit), (9, testkit)]);
        document.parts = crate::lang::rust::parts(
            "src/blob_db.rs",
            "pub fn open() {}\n\n#[cfg(test)]\nmod tests {\n    fn a() {}\n    fn b() {}\n    fn c() {}\n    fn d() {}\n    fn e() {}\n}\n",
        )
        .spans;
        let deps = facts(&[document], &[], false).deps;
        let found: Vec<_> = deps
            .iter()
            .map(|d| (d.line, d.from.is_test(), d.file.to_str().unwrap()))
            .collect();
        assert_eq!(
            found,
            [(1, false, "src/blob_db.rs"), (9, true, "src/blob_db.rs")]
        );
    }

    #[test]
    fn a_symbol_used_in_a_cfg_test_part_leaves_the_part() {
        let tablet = "rust-analyzer cargo testkit 0.1.0 tablet().";
        let mut document = doc("src/blob_db.rs", &[], &[(1, tablet), (9, tablet)]);
        document.parts = crate::lang::rust::parts(
            "src/blob_db.rs",
            "pub fn open() {}\n\n#[cfg(test)]\nmod tests {\n    fn a() {}\n    fn b() {}\n    fn c() {}\n    fn d() {}\n    fn e() {}\n}\n",
        )
        .spans;
        let uses = facts(&[document], &[], true).symbols;
        let found: Vec<_> = uses.iter().map(|u| (u.line, u.from.is_test())).collect();
        assert_eq!(found, [(1, false), (9, true)]);
    }

    #[test]
    fn ruby_depends_on_the_deepest_constant_of_a_line() {
        let order = "scip-ruby gem project 0 Shop#Domain#Order#";
        let docs = [
            doc("lib/order.rb", &[order], &[]),
            doc(
                "lib/web.rb",
                &[],
                &[
                    (3, "scip-ruby gem project 0 Shop#"),
                    (3, "scip-ruby gem project 0 Shop#Domain#"),
                    (3, order),
                    (3, "scip-ruby gem project 0 Class#new()."),
                    (4, "scip-ruby gem project 0 Shop#Domain#Order#`@id`."),
                ],
            ),
        ];
        let deps = facts(&docs, &[], false).deps;
        let found: Vec<_> = deps
            .iter()
            .map(|d| (d.line, d.specifier.as_str()))
            .collect();
        assert_eq!(found, [(3, "lib/order.rb")]);
    }

    #[test]
    fn a_ruby_class_method_depends_through_the_constant_it_is_called_on() {
        // `Shop::Domain::Order.find` on line 5, as scip-ruby emits it: the constants of the
        // path beside the method of the singleton class. A method alone, called on a
        // value, depends on nothing.
        let ruby = "scip-ruby gem project 0 ";
        let order = format!("{ruby}Shop#Domain#Order#");
        let find = format!("{ruby}Shop#Domain#`<Class:Order>`#find().");
        let docs = [
            doc("lib/order.rb", &[&order, &find], &[]),
            doc(
                "lib/web.rb",
                &[],
                &[
                    (5, &format!("{ruby}Shop#")),
                    (5, &format!("{ruby}Shop#Domain#")),
                    (5, &order),
                    (5, &find),
                    (6, &find),
                ],
            ),
        ];
        let deps = facts(&docs, &[], false).deps;
        let found: Vec<_> = deps
            .iter()
            .map(|d| (d.line, d.specifier.as_str()))
            .collect();
        assert_eq!(found, [(5, "lib/order.rb")]);
    }

    #[test]
    fn ruby_constants_and_methods_are_symbol_uses() {
        let ruby = "scip-ruby gem project 0 ";
        let client = format!("{ruby}Shop#Db#Client#");
        let query = format!("{ruby}Shop#Db#Client#query().");
        let connect = format!("{ruby}Shop#Db#`<Class:Client>`#connect().");
        let helper = format!("{ruby}Shop#`<Class:Db>`#helper().");
        let config = format!("{ruby}`<Class:Shop>`#config().");
        let docs = [
            doc(
                "lib/shop/db/client.rb",
                &[
                    &format!("{ruby}Shop#"),
                    &format!("{ruby}Shop#Db#"),
                    &client,
                    &query,
                    &connect,
                    &helper,
                ],
                &[],
            ),
            doc(
                "lib/shop/db/rollback.rb",
                &[&format!("{ruby}Shop#"), &format!("{ruby}Shop#Db#")],
                &[],
            ),
            doc("lib/shop.rb", &[&format!("{ruby}Shop#"), &config], &[]),
            doc(
                "lib/shop/web/controller.rb",
                &[&format!("{ruby}Shop#"), &format!("{ruby}Shop#Web#")],
                &[
                    (7, &format!("{ruby}Shop#")),
                    (7, &format!("{ruby}Shop#Db#")),
                    (7, &client),
                    (7, &connect),
                    (8, &query),
                    (9, &format!("{ruby}Kernel#raise().")),
                    (9, &format!("{ruby}ActiveRecord#")),
                    (9, &format!("{ruby}ActiveRecord#Rollback#")),
                    (12, &format!("{ruby}Shop#")),
                    (12, &format!("{ruby}Shop#Db#")),
                    (12, &helper),
                    (14, &config),
                ],
            ),
        ];
        let uses: Vec<_> = facts(&docs, &[], true)
            .symbols
            .into_iter()
            .map(|u| {
                let method = u.method.map(|(owner, name)| format!("{owner}.{name}"));
                (
                    u.line,
                    format!("{}.{}", u.specifier, u.name),
                    method,
                    u.target,
                )
            })
            .collect();
        let db = || Some(Target::Internal(Unit::from("lib/shop/db/client.rb")));
        let method = |m: &str| Some(m.to_string());
        assert_eq!(
            uses,
            [
                (7, "Shop::Db.Client".into(), None, db()),
                (
                    7,
                    "Shop::Db.Client".into(),
                    method("Shop::Db::Client.connect"),
                    db()
                ),
                (
                    8,
                    "Shop::Db.Client".into(),
                    method("Shop::Db::Client.query"),
                    db()
                ),
                (
                    9,
                    ".Kernel".into(),
                    method("Kernel.raise"),
                    Some(Target::External("Kernel".into()))
                ),
                (
                    9,
                    "ActiveRecord.Rollback".into(),
                    None,
                    Some(Target::External("ActiveRecord::Rollback".into()))
                ),
                // Shop::Db is reopened in both files of lib/shop/db: no one file owns it.
                (12, "Shop.Db".into(), None, None),
                (12, "Shop.Db".into(), method("Shop::Db.helper"), db()),
                // The file opens Shop too, but the method is defined elsewhere.
                (
                    14,
                    ".Shop".into(),
                    method("Shop.config"),
                    Some(Target::Internal(Unit::from("lib/shop.rb")))
                ),
            ]
        );
        assert!(facts(&docs, &[], false).symbols.is_empty());
    }

    #[test]
    fn module_references_are_dependencies_at_their_first_line() {
        let documents = vec![
            doc("internal/db/db.go", &[DB, &format!("{DB}Client#")], &[]),
            doc(
                "internal/handlers/h.go",
                &[],
                &[
                    (12, DB),
                    (6, DB),
                    (6, FMT),
                    (9, &format!("{DB}Client#")),
                    (
                        9,
                        "scip-go gomod github.com/golang/go/src go1.25 fmt/Println().",
                    ),
                ],
            ),
        ];
        let facts = facts(&documents, &[], false);
        let got: Vec<_> = facts
            .deps
            .iter()
            .map(|d| (d.file.to_str().unwrap(), d.line, d.target.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("internal/handlers/h.go", 6, Target::External("fmt".into())),
                (
                    "internal/handlers/h.go",
                    6,
                    Target::Internal(Unit::from("internal/db/db.go"))
                ),
            ]
        );
    }

    /// The text and references scip-go wrote for an alias of `db` beside `fmt`
    /// on one line, and an import of `os` it dropped beside a resolved `io`.
    #[test]
    fn an_import_is_resolved_by_a_module_on_its_own_path() {
        let lines = [
            "package h",
            "",
            r#"import ( "fmt"; dbx "example.com/app/internal/db" )"#,
            "",
            r#"import ( "os"; "io" )"#,
        ];
        let reference = |line, columns, symbol: &str| Reference {
            line,
            columns,
            symbol: symbol.to_string(),
            spelled: None,
        };
        let io = "scip-go gomod github.com/golang/go/src go1.25 io/";
        let document = Document {
            path: "internal/handlers/h.go".into(),
            references: vec![
                reference(3, Some((10, 13)), FMT),
                reference(3, Some((16, 19)), DB),
                reference(3, Some((21, 48)), DB),
                reference(5, Some((16, 18)), io),
            ],
            encoding: PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
            ..Document::default()
        };
        let resolution = |document: &Document, line, specifier: &str| {
            let names = Names::of(std::slice::from_ref(document), &[]);
            Indexed::of(&names).resolution(
                document,
                &lines,
                &Import::new(specifier, line, crate::lang::Form::Static),
            )
        };
        let resolved = |line, specifier: &str| resolution(&document, line, specifier);
        assert_eq!(resolved(3, "fmt"), Resolution::Target);
        assert_eq!(
            resolved(3, "example.com/app/internal/db"),
            Resolution::Target
        );
        assert_eq!(resolved(5, "io"), Resolution::Target);
        assert_eq!(
            resolved(5, "os"),
            Resolution::Nothing,
            "`io` on its line does not resolve `os`"
        );
        // What a range over several lines stands on is unknown: it resolves nothing.
        let spanning = Document {
            references: vec![reference(5, None, io)],
            ..document.clone()
        };
        assert_eq!(resolution(&spanning, 5, "io"), Resolution::Nothing);
        // A name of the module is no module.
        let member = Document {
            references: vec![reference(5, Some((16, 18)), &format!("{io}Reader#"))],
            ..document.clone()
        };
        assert_eq!(resolution(&member, 5, "io"), Resolution::Nothing);
    }

    /// An import is resolved only by a module the index takes somewhere: a file of
    /// the project or an external package. A wildcard declaration (`"*.css"`) is
    /// no target, nor is a module of the project's own package that no document
    /// defines (a JSON file the compiler read but did not index).
    #[test]
    fn an_import_is_resolved_only_by_a_module_with_a_source_target() {
        let lines = [
            r#"import css from "./app.css";"#,
            r#"import data from "./data.json";"#,
            r#"import { util } from "./util";"#,
            r#"import { z } from "zod";"#,
        ];
        let at = |line, symbol: &str| Reference {
            line,
            columns: Some((
                lines[line - 1].find('"').unwrap(),
                lines[line - 1].len() - 1,
            )),
            symbol: symbol.to_string(),
            spelled: None,
        };
        let app = "scip-typescript npm app 1.0.0 src/";
        let main = Document {
            path: "src/main.ts".into(),
            definitions: vec![format!("{app}`main.ts`/")],
            references: vec![
                at(
                    1,
                    "scip-typescript npm vite 5.0.0 `client.d.ts`/`\"*.css\"`/",
                ),
                at(2, &format!("{app}`data.json`/")),
                at(3, &format!("{app}`util.ts`/")),
                at(4, "scip-typescript npm zod 3.0.0 `index.d.ts`/"),
            ],
            encoding: PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
            ..Document::default()
        };
        let util = doc("src/util.ts", &[&format!("{app}`util.ts`/")], &[]);
        let documents = [main.clone(), util];
        let names = Names::of(&documents, &[]);
        let index = Indexed::of(&names);
        let resolution = |line, specifier: &str| {
            index.resolution(
                &main,
                &lines,
                &Import::new(specifier, line, crate::lang::Form::Static),
            )
        };
        assert_eq!(resolution(1, "./app.css"), Resolution::Wildcard);
        assert_eq!(resolution(2, "./data.json"), Resolution::Nothing);
        assert_eq!(resolution(3, "./util"), Resolution::Target);
        assert_eq!(resolution(4, "zod"), Resolution::Target);
        // Nor is a wildcard a dependency or a use.
        let facts = index.facts(true, &BTreeSet::new());
        let externals: Vec<&Target> = facts
            .deps
            .iter()
            .map(|dep| &dep.target)
            .chain(facts.symbols.iter().filter_map(|used| used.target.as_ref()))
            .filter(
                |target| matches!(target, Target::External(module) if module.starts_with("vite")),
            )
            .collect();
        assert!(externals.is_empty(), "{externals:?}");
    }

    /// A name of a file of the project that stands on the text the index records
    /// an import on resolves it, as Python's `from . import VERSION` does; a module
    /// that is the importing file itself names no target, as scip-python takes a
    /// `from .. import x` above the top package to.
    #[test]
    fn a_name_of_the_project_resolves_an_import_and_the_file_itself_does_not() {
        let lines = [
            "from . import VERSION as V",
            "from .. import lib",
            "from . import sibling",
        ];
        let py = "scip-python python project 0 ";
        let on = |line: usize, text: &str, symbol: String| Reference {
            line,
            columns: Some({
                let start = lines[line - 1].rfind(text).unwrap();
                (start, start + text.len())
            }),
            symbol,
            spelled: None,
        };
        let main = Document {
            path: "src/app/main.py".into(),
            definitions: vec![format!("{py}`src.app.main`/__init__:")],
            references: vec![
                on(1, "VERSION as V", format!("{py}`src.app`/VERSION.")),
                on(2, "lib", format!("{py}`src.app.main`/__init__:")),
                on(3, "sibling", format!("{py}sibling/__init__:")),
            ],
            encoding: PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
            ..Document::default()
        };
        let init = doc(
            "src/app/__init__.py",
            &[
                &format!("{py}`src.app`/__init__:"),
                &format!("{py}`src.app`/VERSION."),
            ],
            &[],
        );
        let sibling = doc(
            "src/app/sibling.py",
            &[&format!("{py}`src.app.sibling`/__init__:")],
            &[],
        );
        let documents = [main.clone(), init, sibling];
        let names = Names::of(&documents, &[]);
        let index = Indexed::of(&names);
        let resolution = |line, specifier: &str, recorded_on: &str| {
            index.resolution(
                &main,
                &lines,
                &Import {
                    recorded_on: Some(recorded_on.into()),
                    ..Import::new(specifier, line, crate::lang::Form::Static)
                },
            )
        };
        assert_eq!(
            resolution(1, ".VERSION", "VERSION as V"),
            Resolution::Target
        );
        assert_eq!(resolution(2, "..lib", "lib"), Resolution::Nothing);
        assert_eq!(resolution(3, ".sibling", "sibling"), Resolution::Target);
        assert_eq!(
            resolution(3, ".sibling", ".sibling"),
            Resolution::Nothing,
            "the module stands on `sibling`, not on the specifier"
        );
    }

    #[test]
    fn excluded_documents_only_resolve_what_no_source_defines() {
        let generated = "scip-go gomod example.com/app . `example.com/app/gen`/";
        let sources = vec![
            doc("internal/db/db.go", &[DB], &[]),
            doc("internal/handlers/h.go", &[], &[(3, DB), (4, generated)]),
        ];
        // `a_gen` sorts first and also defines DB; it must not win.
        let excluded = vec![
            doc("a_gen/db.go", &[DB], &[(1, DB)]),
            doc("gen/gen.go", &[generated], &[]),
        ];
        let got: Vec<_> = facts(&sources, &excluded, false)
            .deps
            .iter()
            .map(|d| (d.file.to_str().unwrap().to_string(), d.target.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "internal/handlers/h.go".to_string(),
                    Target::Internal(Unit::from("gen/gen.go"))
                ),
                (
                    "internal/handlers/h.go".to_string(),
                    Target::Internal(Unit::from("internal/db/db.go"))
                ),
            ]
        );
    }

    #[test]
    fn member_references_are_symbol_uses() {
        let documents = vec![
            doc("internal/db/db.go", &[DB, &format!("{DB}Client#")], &[]),
            doc(
                "internal/handlers/h.go",
                &[],
                &[
                    (9, &format!("{DB}Client#")),
                    (10, &format!("{FMT}Println().")),
                ],
            ),
        ];
        let uses = facts(&documents, &[], true).symbols;
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].name, "Client");
        assert_eq!(uses[0].specifier, "example.com/app/internal/db");
        assert_eq!(
            uses[0].target,
            Some(Target::Internal(Unit::from("internal/db/db.go")))
        );
        assert_eq!(uses[1].target, Some(Target::External("fmt".into())));
        assert!(facts(&documents, &[], false).symbols.is_empty());
    }

    #[test]
    fn a_class_is_the_module_in_php() {
        let documents = vec![
            doc(
                "src/Domain/Order.php",
                &["scip-php composer __root__ 1.0 App/Domain/Order#"],
                &[],
            ),
            // `use function App\Domain\total;` names a function at the top level of a
            // namespace: a dependency on its file, like a class.
            doc(
                "src/Domain/functions.php",
                &["scip-php composer __root__ 1.0 App/Domain/total()."],
                &[],
            ),
            doc(
                "src/Web/C.php",
                &[],
                &[
                    (4, "scip-php composer __root__ 1.0 App/Domain/Order#"),
                    (
                        5,
                        "scip-php composer illuminate/support 10.0 Illuminate/Support/Str#",
                    ),
                    (6, "scip-php composer __root__ 1.0 App/Domain/total()."),
                    (7, "scip-php composer __root__ 1.0 App/Domain/Order#id()."),
                ],
            ),
        ];
        let deps = facts(&documents, &[], false).deps;
        let got: Vec<_> = deps.iter().map(|d| (d.line, d.target.clone())).collect();
        assert_eq!(
            got,
            vec![
                (5, Target::External("Illuminate\\Support\\Str".into())),
                (4, Target::Internal(Unit::from("src/Domain/Order.php"))),
                (6, Target::Internal(Unit::from("src/Domain/functions.php"))),
            ]
        );
    }

    #[test]
    fn an_augmentation_of_an_external_name_is_no_dependency_on_its_file() {
        // `src/app/router.ts` augments the package `router` (`declare module "router"`).
        // scip-typescript points the string of an import of `router` and a name the
        // package defines at the package and at the augmentation, on the same range.
        let app = "scip-typescript npm fx 1.0.0 src/app/`router.ts`/`\"router\"`/";
        let register = format!("{app}Register#");
        let package = "scip-typescript npm router 1.0.0 `index.d.ts`/";
        let at = |line: usize, columns: (usize, usize), symbol: &str| Reference {
            line,
            columns: Some(columns),
            symbol: symbol.to_string(),
            spelled: None,
        };
        let mut platform = doc("src/platform/link.ts", &[], &[]);
        platform.references = vec![
            at(1, (14, 22), &format!("{package}Register#")),
            at(1, (14, 22), &register),
            at(1, (30, 38), package),
            at(1, (30, 38), app),
        ];
        let mut extra = doc("src/platform/extra.ts", &[], &[]);
        // The augmentation alone on its range: a name only the project defines.
        extra.references = vec![at(1, (14, 22), &register)];
        let documents = vec![
            doc("src/app/router.ts", &[app, &register], &[]),
            platform,
            extra,
        ];
        let mut found: Vec<_> = facts(&documents, &[], false)
            .deps
            .into_iter()
            .map(|d| (d.file, d.target))
            .collect();
        found.sort_by_key(|(file, _)| file.clone());
        assert_eq!(
            found,
            [
                (
                    PathBuf::from("src/platform/extra.ts"),
                    Target::Internal(Unit::from("src/app/router.ts"))
                ),
                (
                    PathBuf::from("src/platform/link.ts"),
                    Target::External("router".into())
                ),
            ]
        );
    }

    #[test]
    fn typescript_externals_use_the_npm_name() {
        let documents = vec![doc(
            "src/a.ts",
            &[],
            &[(1, "scip-typescript npm @types/lodash 4.0.0 `index.d.ts`/")],
        )];
        let facts = facts(&documents, &[], false);
        assert_eq!(facts.deps[0].target, Target::External("lodash".into()));
    }

    #[test]
    fn a_name_of_an_unnamed_package_is_no_symbol_use_but_is_reported_as_unjudged() {
        // scip-typescript before 0.6.1 indexes what sits under a `package.json` without
        // `name` (`{"type": "module"}` in `esm/`) as the unnamed package `. .`.
        let documents = vec![doc(
            "src/web/a.ts",
            &[],
            &[(1, "scip-typescript npm . . esm/`index.d.ts`/Thing#")],
        )];
        let facts = facts(&documents, &[], true);
        assert!(facts.symbols.is_empty());
        assert_eq!(
            facts.unnamed.into_iter().collect::<Vec<_>>(),
            ["src/web/a.ts"]
        );
    }

    #[test]
    fn the_string_an_occurrence_stands_on_is_read_between_its_quotes() {
        let at = |line: i32, start: i32, end: i32| {
            let mut occurrence = Occurrence::new();
            occurrence.range = vec![line, start, end];
            occurrence
        };
        let lines: Vec<String> = [
            "import { Hono } from 'hono'",
            "import x from \"./local\"",
            // `é` is one UTF-16 unit and two UTF-8 bytes.
            "/*é*/ import a from \"@scope/pkg/sub\"",
        ]
        .map(str::to_string)
        .to_vec();
        let utf16 = PositionEncoding::UnspecifiedPositionEncoding;
        assert_eq!(string_at(&lines, &at(0, 21, 27), utf16), Some("hono"));
        assert_eq!(string_at(&lines, &at(0, 9, 13), utf16), None, "a name");
        assert_eq!(string_at(&lines, &at(1, 14, 23), utf16), Some("./local"));
        assert_eq!(
            string_at(&lines, &at(2, 20, 36), utf16),
            Some("@scope/pkg/sub")
        );
        let utf8 = PositionEncoding::UTF8CodeUnitOffsetFromLineStart;
        assert_eq!(
            string_at(&lines, &at(2, 21, 37), utf8),
            Some("@scope/pkg/sub")
        );
        assert_eq!(string_at(&lines, &at(9, 0, 1), utf16), None, "no such line");
    }

    #[test]
    fn documents_outside_the_project_are_dropped_and_paths_rebased() {
        let mut index = Index::new();
        for path in ["a.go", "../../go-build/x.go", "C:\\cache\\y.go", "b\\c.go"] {
            let mut document = scip::types::Document::new();
            document.relative_path = path.to_string();
            let mut occurrence = Occurrence::new();
            occurrence.symbol = DB.to_string();
            occurrence.typed_range =
                Some(Typed_range::SingleLineRange(scip::types::SingleLineRange {
                    line: 4,
                    ..Default::default()
                }));
            document.occurrences.push(occurrence);
            index.documents.push(document);
        }
        let bytes = index.write_to_bytes().unwrap();
        let documents =
            parse(&bytes, Path::new("/nonexistent"), Path::new("services/api")).unwrap();
        let paths: Vec<_> = documents.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["services/api/a.go", "services/api/b/c.go"]);
        assert_eq!(documents[0].references[0].line, 5);
    }

    /// Go, as scip-go indexes it: `chunk` defines `type Block = blob.Block` on line
    /// 9, `type Two = Block` on line 11, `type Ctx = context.Context` on line 13 and
    /// `type Mine = Own` on line 15; `gw` names `chunk` on line 1 and writes each
    /// alias on a line of its own.
    fn go_aliases() -> Vec<Document> {
        let go = "scip-go gomod example.com/app . ";
        let module = |pkg: &str| format!("{go}`example.com/app/{pkg}`/");
        let name = |pkg: &str, name: &str| format!("{}{name}", module(pkg));
        let context = "scip-go gomod github.com/golang/go/src go1.25 context/";
        let context_type = format!("{context}Context#");
        let alias = |symbol: String, line, stands_for: &[&str]| Alias {
            symbol,
            line,
            stands_for: stands_for.iter().map(|s| s.to_string()).collect(),
        };
        let mut chunk = doc(
            "chunk/chunk.go",
            &[
                &module("chunk"),
                &name("chunk", "Block#"),
                &name("chunk", "Two#"),
                &name("chunk", "Ctx#"),
                &name("chunk", "Own#"),
                &name("chunk", "Mine#"),
            ],
            &[
                (5, &module("blob")),
                (9, &module("blob")),
                (9, &name("blob", "Block#")),
                (11, &name("chunk", "Block#")),
                (13, context),
                (13, &context_type),
                (15, &name("chunk", "Own#")),
            ],
        );
        chunk.aliases = vec![
            alias(
                name("chunk", "Block#"),
                9,
                &[&module("blob"), &name("blob", "Block#")],
            ),
            alias(name("chunk", "Two#"), 11, &[&name("chunk", "Block#")]),
            alias(name("chunk", "Ctx#"), 13, &[context, &context_type]),
            alias(name("chunk", "Mine#"), 15, &[&name("chunk", "Own#")]),
        ];
        let gw = doc(
            "gw/gw.go",
            &[&module("gw")],
            &[
                (1, &module("chunk")),
                (3, &module("chunk")),
                (3, &name("chunk", "Block#")),
                (4, &module("chunk")),
                (4, &name("chunk", "Two#")),
                (5, &module("chunk")),
                (5, &name("chunk", "Ctx#")),
                (6, &module("chunk")),
                (6, &name("chunk", "Mine#")),
            ],
        );
        vec![
            doc(
                "blob/blob.go",
                &[&module("blob"), &name("blob", "Block#")],
                &[],
            ),
            chunk,
            gw,
        ]
    }

    #[test]
    fn a_use_of_an_alias_depends_on_the_internal_names_it_stands_for_through_chains() {
        let docs = go_aliases();
        let facts = facts(&docs, &[], true);
        let mut deps: Vec<(&str, usize, &str)> = facts
            .deps
            .iter()
            .filter(|d| d.file == Path::new("gw/gw.go"))
            .map(|d| ("gw/gw.go", d.line, d.specifier.as_str()))
            .collect();
        deps.sort();
        // `context` is external, and `Own` is in `chunk`, which `gw` names already.
        assert_eq!(
            deps,
            [
                ("gw/gw.go", 1, "chunk/chunk.go"),
                ("gw/gw.go", 3, "blob/blob.go")
            ]
        );
        let blob = facts
            .deps
            .iter()
            .find(|d| d.file == Path::new("gw/gw.go") && d.specifier == "blob/blob.go")
            .unwrap();
        assert_eq!(blob.written, Written::Name);
        assert_eq!(blob.through, Through::Nothing);
        let at = |line| AliasDefinition {
            file: "chunk/chunk.go".to_string(),
            line,
        };
        let aliased: Vec<(usize, &[AliasDefinition], String)> = blob
            .aliased
            .iter()
            .map(|aliased| {
                let called = aliased.called.as_ref().unwrap().to_string();
                (aliased.line, aliased.chain.as_slice(), called)
            })
            .collect();
        let block = "example.com/app/blob.Block".to_string();
        assert_eq!(
            aliased,
            [
                (3, &[at(9)][..], block.clone()),
                (4, &[at(11), at(9)][..], block),
            ]
        );
        // Each use is a use of `blob.Block` too, and still one of the alias.
        let uses: Vec<(usize, &str, &str)> = facts
            .symbols
            .iter()
            .filter(|u| u.file == Path::new("gw/gw.go"))
            .map(|u| (u.line, u.specifier.as_str(), u.name.as_str()))
            .collect();
        for expected in [
            (3, "example.com/app/blob", "Block"),
            (3, "example.com/app/chunk", "Block"),
            (4, "example.com/app/blob", "Block"),
            (4, "example.com/app/chunk", "Two"),
        ] {
            assert!(uses.contains(&expected), "{expected:?} in {uses:?}");
        }
        assert!(
            !uses.iter().any(|(_, specifier, _)| *specifier == "context"),
            "{uses:?}"
        );
    }

    #[test]
    fn an_alias_in_an_excluded_file_is_followed_to_the_checked_names_it_stands_for() {
        let mut docs = go_aliases();
        let gw = docs.pop().unwrap();
        let chunk = docs.pop().unwrap();
        let facts = facts(&[docs.pop().unwrap(), gw], &[chunk], false);
        assert!(
            facts
                .deps
                .iter()
                .any(|d| d.file == Path::new("gw/gw.go") && d.specifier == "blob/blob.go"),
            "{:?}",
            facts.deps
        );
    }

    #[test]
    fn a_cycle_of_aliases_ends() {
        let go = "scip-go gomod example.com/app . `example.com/app/chunk`/";
        let (a, b) = (format!("{go}A#"), format!("{go}B#"));
        let mut chunk = doc("chunk/chunk.go", &[go, &a, &b], &[]);
        chunk.aliases = vec![
            Alias {
                symbol: a.clone(),
                line: 1,
                stands_for: vec![b.clone()],
            },
            Alias {
                symbol: b.clone(),
                line: 2,
                stands_for: vec![a.clone()],
            },
        ];
        let gw = doc("gw/gw.go", &[], &[(1, &a)]);
        let deps = deps_of(&[chunk, gw]);
        assert_eq!(deps, [(1, "chunk/chunk.go".to_string())]);
    }

    /// An occurrence of [`parsed`]: `[line, start, end]`, an enclosing range or none,
    /// the symbol, and whether it is a definition.
    type At<'a> = ([i32; 3], Option<[i32; 3]>, &'a str, bool);

    /// A document of `occurrences` and `symbols`, parsed.
    fn parsed(occurrences: &[At], symbols: Vec<scip::types::SymbolInformation>) -> Document {
        parsed_at("chunk/chunk.go", occurrences, symbols)
    }

    /// A document at `path` of `occurrences` and `symbols`, parsed.
    fn parsed_at(
        path: &str,
        occurrences: &[At],
        symbols: Vec<scip::types::SymbolInformation>,
    ) -> Document {
        parsed_in(Path::new("/nonexistent"), path, occurrences, symbols)
    }

    /// A document at `path` under `root` of `occurrences` and `symbols`, parsed.
    fn parsed_in(
        root: &Path,
        path: &str,
        occurrences: &[At],
        symbols: Vec<scip::types::SymbolInformation>,
    ) -> Document {
        let mut index = Index::new();
        let mut document = scip::types::Document::new();
        document.relative_path = path.to_string();
        for (range, enclosing, symbol, definition) in occurrences {
            let mut occurrence = Occurrence::new();
            occurrence.range = range.to_vec();
            occurrence.enclosing_range = enclosing.map(|r| r.to_vec()).unwrap_or_default();
            occurrence.symbol = symbol.to_string();
            occurrence.symbol_roles = if *definition { DEFINITION } else { 0 };
            document.occurrences.push(occurrence);
        }
        document.symbols = symbols;
        index.documents.push(document);
        let bytes = index.write_to_bytes().unwrap();
        parse(&bytes, root, Path::new("")).unwrap().remove(0)
    }

    fn information(
        symbol: &str,
        kind: scip::types::symbol_information::Kind,
        related: &[(&str, bool)],
    ) -> scip::types::SymbolInformation {
        let mut information = scip::types::SymbolInformation::new();
        information.symbol = symbol.to_string();
        information.kind = kind.into();
        for (to, implementation) in related {
            let mut relationship = scip::types::Relationship::new();
            relationship.symbol = to.to_string();
            relationship.is_reference = true;
            relationship.is_implementation = *implementation;
            information.relationships.push(relationship);
        }
        information
    }

    #[test]
    fn a_type_alias_stands_for_what_its_definition_refers_to_within_its_range() {
        use scip::types::symbol_information::Kind as K;
        let go = "scip-go gomod example.com/app . ";
        let chunk = |name: &str| format!("{go}`example.com/app/chunk`/{name}");
        let blob = |name: &str| format!("{go}`example.com/app/blob`/{name}");
        let (alias, two, def, a, b) = (
            chunk("Block#"),
            chunk("Two#"),
            chunk("Def#"),
            chunk("A#"),
            chunk("B#"),
        );
        let (module, block, other) = (blob(""), blob("Block#"), blob("Other#"));
        let document = parsed(
            &[
                // `type Block = blob.Block`
                ([8, 5, 10], Some([8, 0, 23]), &alias, true),
                ([8, 13, 17], None, &module, false),
                ([8, 18, 23], None, &block, false),
                // `type Two = Block`, whose reference rust-analyzer gives a range too.
                ([10, 5, 8], Some([10, 0, 16]), &two, true),
                ([10, 11, 16], Some([8, 0, 23]), &alias, false),
                // `type Def blob.Block` is a type of its own.
                ([12, 5, 8], Some([12, 0, 19]), &def, true),
                ([12, 14, 19], None, &block, false),
                // Two aliases on one line: `type A = blob.Block; type B = blob.Other;`.
                ([14, 5, 6], Some([14, 0, 20]), &a, true),
                ([14, 14, 19], None, &block, false),
                ([14, 26, 27], Some([14, 21, 41]), &b, true),
                ([14, 35, 40], None, &other, false),
            ],
            vec![
                information(&alias, K::TypeAlias, &[]),
                information(&two, K::TypeAlias, &[]),
                information(&def, K::Struct, &[]),
                information(&a, K::TypeAlias, &[]),
                information(&b, K::TypeAlias, &[]),
            ],
        );
        let alias_of = |symbol: &str, line: usize, stands_for: &[&str]| Alias {
            symbol: symbol.to_string(),
            line,
            stands_for: stands_for.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(
            document.aliases,
            [
                alias_of(&alias, 9, &[&module, &block]),
                alias_of(&two, 11, &[&alias]),
                alias_of(&a, 15, &[&block]),
                alias_of(&b, 15, &[&other]),
            ]
        );
    }

    #[test]
    fn a_constant_the_index_relates_to_a_name_by_reference_alone_is_an_alias() {
        use scip::types::symbol_information::Kind as K;
        let ruby = "scip-ruby gem fx 0 ";
        let (alias, made, block) = (
            format!("{ruby}Chunk#Block."),
            format!("{ruby}Chunk#Made."),
            format!("{ruby}Blob#Block#"),
        );
        let (run, base_run) = (
            format!("{ruby}Chunk#Kid#run()."),
            format!("{ruby}Blob#Base#run()."),
        );
        let document = parsed(
            &[
                // `Block = Blob::Block`
                ([1, 2, 7], None, &alias, true),
                ([1, 16, 21], None, &block, false),
                // `Made = Blob::Block.new`
                ([2, 2, 6], None, &made, true),
                ([2, 15, 20], None, &block, false),
                // A method that overrides another: TypeScript and Java relate it by
                // reference too, as an implementation.
                ([4, 8, 11], None, &run, true),
            ],
            vec![
                information(&alias, K::UnspecifiedKind, &[(&block, false)]),
                information(&made, K::UnspecifiedKind, &[]),
                information(&run, K::Method, &[(&base_run, true)]),
            ],
        );
        assert_eq!(
            document.aliases,
            [Alias {
                symbol: alias.clone(),
                line: 2,
                stands_for: vec![block.clone()],
            }]
        );
    }

    /// A relationship as an indexer writes one for a supertype: an implementation
    /// that is no reference.
    fn supertype_of(symbol: &str, flags: &[(&str, bool)]) -> scip::types::SymbolInformation {
        let mut information = scip::types::SymbolInformation::new();
        information.symbol = symbol.to_string();
        for (to, reference) in flags {
            let mut relationship = scip::types::Relationship::new();
            relationship.symbol = to.to_string();
            relationship.is_implementation = true;
            relationship.is_reference = *reference;
            information.relationships.push(relationship);
        }
        information
    }

    /// The implementations of `documents` as `sub -> supertype [between]`, by name.
    fn implementations_of(documents: &[Document]) -> Vec<String> {
        implementations_beside(documents, &[])
    }

    /// The implementations of `documents`, with `excluded` beside them.
    fn implementations_beside(documents: &[Document], excluded: &[Document]) -> Vec<String> {
        let names = Names::of(documents, excluded);
        let mut found: Vec<String> = Indexed::of(&names)
            .implementations()
            .iter()
            .map(|i| {
                let between: Vec<String> = i
                    .between
                    .iter()
                    .map(|t| format!("{}:{}", t.file, t.line))
                    .collect();
                format!(
                    "{}:{} {} -> {}:{} {} [{}]",
                    i.sub.file,
                    i.sub.line,
                    i.sub.name,
                    i.supertype.file,
                    i.supertype.line,
                    i.supertype.name,
                    between.join(", ")
                )
            })
            .collect();
        found.sort();
        found
    }

    /// A type defined on line 3 of `path`, with its supertypes.
    fn type_at(path: &str, symbol: &str, supertypes: &[(&str, bool)]) -> Document {
        parsed_at(
            path,
            &[([2, 13, 16], Some([2, 0, 20]), symbol, true)],
            vec![supertype_of(symbol, supertypes)],
        )
    }

    #[test]
    fn a_transitive_index_gives_each_implementation_with_the_types_between() {
        let java = "scip-java maven maven/fx/fx 0 fx/";
        let (port, base, sql, record) = (
            format!("{java}Port#"),
            format!("{java}Base#"),
            format!("{java}SqlRepo#"),
            "scip-java maven jdk 21 java/lang/Record#".to_string(),
        );
        let (port_get, sql_get) = (
            format!("{java}Port#get()."),
            format!("{java}SqlRepo#get()."),
        );
        let documents = [
            // scip-java relates an interface back to a record that implements it,
            // as a reference: no supertype.
            type_at("ports/Port.java", &port, &[(&sql, true)]),
            type_at("base/Base.java", &base, &[(&port, false)]),
            parsed_at(
                "sql/SqlRepo.java",
                &[
                    ([2, 13, 16], Some([2, 0, 20]), &sql, true),
                    ([4, 8, 11], Some([4, 4, 20]), &sql_get, true),
                ],
                vec![
                    supertype_of(&sql, &[(&base, false), (&port, false), (&record, false)]),
                    // A method's relation follows from its type's.
                    supertype_of(&sql_get, &[(&port_get, true)]),
                ],
            ),
        ];
        assert_eq!(
            implementations_of(&documents),
            [
                "base/Base.java:3 Base -> ports/Port.java:3 Port []",
                "sql/SqlRepo.java:3 SqlRepo -> base/Base.java:3 Base []",
                "sql/SqlRepo.java:3 SqlRepo -> ports/Port.java:3 Port [base/Base.java:3]",
            ]
        );
    }

    #[test]
    fn direct_relations_are_closed_into_the_transitive_set() {
        let python = "scip-python python fx 0.1 ";
        let (port, base, sql) = (
            format!("{python}`fx.ports`/Port#"),
            format!("{python}`fx.base`/Base#"),
            format!("{python}`fx.sql`/SqlRepo#"),
        );
        let documents = [
            type_at("fx/ports.py", &port, &[]),
            type_at("fx/base.py", &base, &[(&port, false)]),
            type_at("fx/sql.py", &sql, &[(&base, false)]),
        ];
        assert_eq!(
            implementations_of(&documents),
            [
                "fx/base.py:3 Base -> fx/ports.py:3 Port []",
                "fx/sql.py:3 SqlRepo -> fx/base.py:3 Base []",
                "fx/sql.py:3 SqlRepo -> fx/ports.py:3 Port [fx/base.py:3]",
            ]
        );
    }

    #[test]
    fn a_chain_goes_through_a_type_of_an_excluded_file() {
        let python = "scip-python python fx 0.1 ";
        let (port, base, sql) = (
            format!("{python}`fx.ports`/Port#"),
            format!("{python}`fx.base`/Base#"),
            format!("{python}`fx.sql`/SqlRepo#"),
        );
        let checked = [
            type_at("fx/ports.py", &port, &[]),
            type_at("fx/sql.py", &sql, &[(&base, false)]),
        ];
        let excluded = [type_at("fx/base.py", &base, &[(&port, false)])];
        // Only types of checked files are judged, but a change to the excluded one
        // between them may make the implementation.
        assert_eq!(
            implementations_beside(&checked, &excluded),
            ["fx/sql.py:3 SqlRepo -> fx/ports.py:3 Port [fx/base.py:3]"]
        );
    }

    #[test]
    fn a_type_the_index_names_locally_implements_too() {
        let ts = "scip-typescript npm . . ";
        let port = format!("{ts}src/`port.ts`/Port#");
        let source = "import { Port } from \"./port\";\n\nexport function make(): Port {\n  class Inner implements Port {\n    run() {}\n  }\n  return new Inner();\n}\n";
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/local.ts"), source).unwrap();
        let mut method = scip::types::SymbolInformation::new();
        method.symbol = "local 1".to_string();
        let mut overrides = scip::types::Relationship::new();
        overrides.symbol = format!("{ts}src/`port.ts`/Port#run().");
        overrides.is_implementation = true;
        overrides.is_reference = true;
        method.relationships.push(overrides);
        let documents = [
            type_at("src/port.ts", &port, &[]),
            parsed_in(
                root.path(),
                "src/local.ts",
                &[
                    // `class Inner implements Port` inside `make`.
                    ([3, 8, 13], Some([3, 2, 5]), "local 0", true),
                    ([4, 4, 7], None, "local 1", true),
                ],
                vec![supertype_of("local 0", &[(&port, false)]), method],
            ),
        ];
        assert_eq!(
            implementations_of(&documents),
            ["src/local.ts:4 Inner -> src/port.ts:3 Port []"]
        );
    }

    #[test]
    fn a_class_the_index_gives_no_type_is_told_by_its_methods() {
        let java = "scip-java maven maven/fx/fx 0 fx/";
        let (port, find, other) = (
            format!("{java}Port#"),
            format!("{java}Port#find()."),
            format!("{java}Port#other()."),
        );
        // `new Port() { find() {…} other() {…} }`: the class is `local 0`, which the
        // index never defines, and each method relates to the method it implements.
        let method = |symbol: &str, implements: &str| {
            let mut information = scip::types::SymbolInformation::new();
            information.symbol = symbol.to_string();
            information.enclosing_symbol = "local 0".to_string();
            let mut relationship = scip::types::Relationship::new();
            relationship.symbol = implements.to_string();
            relationship.is_implementation = true;
            relationship.is_reference = true;
            information.relationships.push(relationship);
            information
        };
        let document = parsed_at(
            "cache/Anonymous.java",
            &[
                ([7, 26, 30], Some([7, 12, 50]), "local 1", true),
                ([8, 26, 31], Some([8, 12, 50]), "local 2", true),
            ],
            vec![method("local 1", &find), method("local 2", &other)],
        );
        assert_eq!(
            document.anonymous,
            [Anonymous {
                line: 8,
                implements: vec![port],
            }]
        );
    }

    #[test]
    fn a_structural_relation_is_no_implementation() {
        let go = "scip-go gomod example.com/app . ";
        let (closer, file) = (
            format!("{go}`example.com/app/io`/Closer#"),
            format!("{go}`example.com/app/other`/File#"),
        );
        let documents = [
            type_at("io/io.go", &closer, &[]),
            type_at("other/file.go", &file, &[(&closer, false)]),
        ];
        assert!(implementations_of(&documents).is_empty());
    }
}
