//! Reads a SCIP index and turns it into the facts the rules check.
//!
//! An indexer (`scip-go`, `scip-typescript`, …) resolves every name in the
//! project the way the compiler does. Two kinds of references matter here:
//!
//! - a reference to a *module* symbol (a Go package, a TypeScript file) stands
//!   on the lines that import or use that module, so it is a dependency;
//! - a reference to a *member* of a module (`db.Client`) is a use of a name.
//!
//! References to symbols that no document of the index defines point outside
//! the project: they are external modules.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use protobuf::Message;
use scip::types::{Index, Occurrence, occurrence::Typed_range};

use crate::check::SymbolRef;
use crate::lang::Target;
use crate::scan::Dep;

/// `SymbolRole::Definition` in the bitset of an occurrence.
const DEFINITION: i32 = 1;

/// One reference to a symbol in a document.
#[derive(Debug, Clone)]
pub struct Reference {
    /// 1-based line.
    pub line: usize,
    pub symbol: String,
}

/// A source file of the index, with the paths relative to the project root.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub path: String,
    pub references: Vec<Reference>,
    /// Symbols this file defines.
    pub definitions: Vec<String>,
}

/// The documents of one or more indexes, merged.
#[derive(Debug, Default)]
pub struct Documents(pub Vec<Document>);

/// Parses an index written by an indexer that ran in `base` (a directory
/// relative to the project root, `""` for the root itself). Documents outside
/// the project, such as generated files in a build cache, are dropped.
pub fn parse(bytes: &[u8], base: &Path) -> Result<Vec<Document>> {
    let index = Index::parse_from_bytes(bytes).context("not a SCIP index")?;
    let prefix = crate::scan::slash(base);
    let mut documents = Vec::new();
    for document in &index.documents {
        let relative = document.relative_path.as_str();
        if relative.starts_with("..") || relative.starts_with('/') || relative.contains(":\\") {
            continue;
        }
        let path = match prefix.as_str() {
            "" => relative.to_string(),
            base => format!("{base}/{relative}"),
        };
        let mut parsed = Document {
            path,
            ..Document::default()
        };
        for occurrence in &document.occurrences {
            if occurrence.symbol.is_empty() || occurrence.symbol.starts_with("local ") {
                continue;
            }
            if occurrence.symbol_roles & DEFINITION != 0 {
                parsed.definitions.push(occurrence.symbol.clone());
            } else {
                parsed.references.push(Reference {
                    line: line_of(occurrence) + 1,
                    symbol: occurrence.symbol.clone(),
                });
            }
        }
        documents.push(parsed);
    }
    Ok(documents)
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

/// How a descriptor of a symbol ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Namespace,
    Other,
}

/// A symbol split into the package that owns it and its descriptors.
#[derive(Debug)]
struct Parsed<'a> {
    scheme: &'a str,
    package: &'a str,
    descriptors: Vec<(String, Kind)>,
}

/// `scheme manager package version descriptors`.
fn split(symbol: &str) -> Option<Parsed<'_>> {
    let mut parts = symbol.splitn(5, ' ');
    let scheme = parts.next()?;
    let _manager = parts.next()?;
    let package = parts.next()?;
    let _version = parts.next()?;
    let descriptors = descriptors(parts.next()?)?;
    Some(Parsed {
        scheme,
        package,
        descriptors,
    })
}

/// The descriptors of a symbol: a name, quoted in backticks when it holds
/// anything but identifier characters, and a suffix (`/` namespace, `#` type,
/// `.` term, `().` method, `:` meta, `!` macro, `[T]` type parameter, `(p)` parameter).
fn descriptors(text: &str) -> Option<Vec<(String, Kind)>> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let mut found = Vec::new();
    while at < chars.len() {
        let mut name = String::new();
        if chars[at] == '`' {
            at += 1;
            loop {
                match chars.get(at)? {
                    '`' if chars.get(at + 1) == Some(&'`') => {
                        name.push('`');
                        at += 2;
                    }
                    '`' => {
                        at += 1;
                        break;
                    }
                    c => {
                        name.push(*c);
                        at += 1;
                    }
                }
            }
        } else if chars[at] == '[' || chars[at] == '(' {
            // A type or value parameter descriptor starts with its bracket.
        } else {
            while let Some(c) = chars.get(at)
                && (c.is_alphanumeric() || matches!(c, '_' | '+' | '-' | '$'))
            {
                name.push(*c);
                at += 1;
            }
        }
        let kind = match chars.get(at)? {
            '/' => {
                at += 1;
                Kind::Namespace
            }
            '#' | '.' | ':' | '!' => {
                at += 1;
                Kind::Other
            }
            '(' => {
                // `name().` or `name(disambiguator).`; a parameter is `(name)`.
                let close = chars[at..].iter().position(|c| *c == ')')? + at;
                at = close + 1;
                if chars.get(at) == Some(&'.') {
                    at += 1;
                }
                Kind::Other
            }
            '[' => {
                let close = chars[at..].iter().position(|c| *c == ']')? + at;
                if name.is_empty() {
                    name = chars[at + 1..close].iter().collect();
                }
                at = close + 1;
                Kind::Other
            }
            _ => return None,
        };
        found.push((name, kind));
    }
    Some(found)
}

/// What a reference points at.
enum Referent {
    /// A whole module: the package or file that an import names.
    Module,
    /// A name inside a module.
    Member(String),
}

impl Parsed<'_> {
    fn referent(&self) -> Referent {
        match self
            .descriptors
            .iter()
            .find(|(_, kind)| *kind == Kind::Other)
        {
            Some((name, _)) => Referent::Member(name.clone()),
            None => Referent::Module,
        }
    }

    /// The namespace descriptors joined: the import path of a Go package, the
    /// file of a TypeScript module.
    fn namespace(&self) -> String {
        self.descriptors
            .iter()
            .take_while(|(_, kind)| *kind == Kind::Namespace)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The external module the symbol belongs to, in the spelling of the
    /// language's import statements and of `forbid_external`.
    fn external_module(&self) -> String {
        match self.scheme {
            // Go: the package path (`fmt`, `net/http`, `github.com/lib/pq`).
            "scip-go" => self.namespace(),
            // TypeScript: the npm package and the file inside it that was imported
            // (`lodash/fp`, `moment/locale/ru`); `@types/x` stands for `x`.
            _ => {
                let mut parts: Vec<String> = self
                    .descriptors
                    .iter()
                    .take_while(|(_, kind)| *kind == Kind::Namespace)
                    .map(|(name, _)| name.clone())
                    .collect();
                if let Some(last) = parts.last_mut() {
                    for suffix in [".d.ts", ".d.mts", ".d.cts", ".ts", ".tsx", ".js", ".mjs"] {
                        if let Some(stem) = last.strip_suffix(suffix) {
                            *last = stem.to_string();
                            break;
                        }
                    }
                    if last == "index" {
                        parts.pop();
                    }
                }
                std::iter::once(npm_name(self.package))
                    .chain(parts)
                    .collect::<Vec<_>>()
                    .join("/")
            }
        }
    }

    /// The qualifier that `forbid_symbol` names: what the code calls the module.
    fn qualifier(&self, internal: Option<&str>) -> String {
        match (self.scheme, internal) {
            ("scip-go", _) => self.namespace(),
            (_, Some(path)) => path
                .rsplit_once('.')
                .map_or(path, |(stem, _)| stem)
                .to_string(),
            (_, None) => npm_name(self.package),
        }
    }
}

/// `@types/lodash` is `lodash`, `@types/scope__name` is `@scope/name`.
fn npm_name(package: &str) -> String {
    match package.strip_prefix("@types/") {
        Some(rest) => match rest.split_once("__") {
            Some((scope, name)) => format!("@{scope}/{name}"),
            None => rest.to_string(),
        },
        None => package.to_string(),
    }
}

/// Dependencies and symbol uses of the documents.
pub struct Facts {
    pub deps: Vec<Dep>,
    pub symbols: Vec<SymbolRef>,
}

/// One `Dep` per file and module it refers to, at the first line that does:
/// the import. With `symbols`, also one `SymbolRef` per use of a name.
pub fn facts(documents: &[Document], symbols: bool) -> Facts {
    let mut defined: HashMap<&str, &str> = HashMap::new();
    for document in documents {
        for symbol in &document.definitions {
            let entry = defined.entry(symbol).or_insert(&document.path);
            if document.path.as_str() < *entry {
                *entry = &document.path;
            }
        }
    }
    let mut deps = Vec::new();
    let mut uses = Vec::new();
    for document in documents {
        // (target, module) -> first line
        let mut first: BTreeMap<(Option<&str>, String), usize> = BTreeMap::new();
        for reference in &document.references {
            let Some(parsed) = split(&reference.symbol) else {
                continue;
            };
            let internal = defined.get(reference.symbol.as_str()).copied();
            match parsed.referent() {
                Referent::Module => {
                    if internal == Some(document.path.as_str()) {
                        continue;
                    }
                    let module = match internal {
                        Some(_) => String::new(),
                        None => parsed.external_module(),
                    };
                    if internal.is_none() && module.is_empty() {
                        continue;
                    }
                    let line = first.entry((internal, module)).or_insert(reference.line);
                    *line = (*line).min(reference.line);
                }
                Referent::Member(name) if symbols => {
                    let module_symbol = member_module(&reference.symbol);
                    let home = module_symbol
                        .as_deref()
                        .and_then(|m| defined.get(m).copied());
                    if home == Some(document.path.as_str()) {
                        continue;
                    }
                    let target = match home {
                        Some(path) => Target::Internal(PathBuf::from(path)),
                        None => Target::External(parsed.external_module()),
                    };
                    uses.push(SymbolRef {
                        file: PathBuf::from(&document.path),
                        line: reference.line,
                        specifier: parsed.qualifier(home),
                        name,
                        target,
                    });
                }
                Referent::Member(_) => {}
            }
        }
        for ((internal, module), line) in first {
            let (specifier, target) = match internal {
                Some(path) => (path.to_string(), Target::Internal(PathBuf::from(path))),
                None => (module.clone(), Target::External(module)),
            };
            deps.push(Dep {
                file: PathBuf::from(&document.path),
                line,
                specifier,
                target,
            });
        }
    }
    Facts {
        deps,
        symbols: uses,
    }
}

/// The module symbol a member symbol lives in: the same header with only the
/// namespace descriptors.
fn member_module(symbol: &str) -> Option<String> {
    let mut parts = symbol.splitn(5, ' ');
    let header: Vec<&str> = parts.by_ref().take(4).collect();
    let descriptors = parts.next()?;
    let mut module = String::new();
    let mut at = 0;
    let chars: Vec<char> = descriptors.chars().collect();
    while at < chars.len() {
        let start = at;
        if chars[at] == '`' {
            at += 1;
            while at < chars.len() && !(chars[at] == '`' && chars.get(at + 1) != Some(&'`')) {
                at += if chars[at] == '`' { 2 } else { 1 };
            }
            at += 1;
        } else {
            while at < chars.len() && !matches!(chars[at], '/' | '#' | '.' | ':' | '!' | '(' | '[')
            {
                at += 1;
            }
        }
        if chars.get(at) != Some(&'/') {
            break;
        }
        at += 1;
        module.extend(&chars[start..at]);
    }
    if module.is_empty() {
        return None;
    }
    Some(format!("{} {}", header.join(" "), module))
}

impl Documents {
    pub fn merged(parts: Vec<Vec<Document>>) -> Result<Self> {
        let mut by_path: BTreeMap<String, Document> = BTreeMap::new();
        for document in parts.into_iter().flatten() {
            match by_path.get_mut(&document.path) {
                Some(seen) => {
                    seen.references.extend(document.references);
                    seen.definitions.extend(document.definitions);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, defs: &[&str], refs: &[(usize, &str)]) -> Document {
        Document {
            path: path.to_string(),
            definitions: defs.iter().map(|s| s.to_string()).collect(),
            references: refs
                .iter()
                .map(|(line, symbol)| Reference {
                    line: *line,
                    symbol: symbol.to_string(),
                })
                .collect(),
        }
    }

    const DB: &str = "scip-go gomod example.com/app . `example.com/app/internal/db`/";
    const FMT: &str = "scip-go gomod github.com/golang/go/src go1.25 fmt/";

    #[test]
    fn parses_descriptors() {
        let d = descriptors("src/lib/`index.ts`/Client#Query().").unwrap();
        let names: Vec<_> = d.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["src", "lib", "index.ts", "Client", "Query"]);
        assert_eq!(d[2].1, Kind::Namespace);
        assert_eq!(d[3].1, Kind::Other);
        let go = split("scip-go gomod example.com/app . `example.com/app/internal/db`/Scoped().")
            .unwrap();
        assert_eq!(go.namespace(), "example.com/app/internal/db");
        assert!(matches!(go.referent(), Referent::Member(n) if n == "Scoped"));
        assert!(matches!(split(DB).unwrap().referent(), Referent::Module));
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
        let facts = facts(&documents, false);
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
                    Target::Internal(PathBuf::from("internal/db/db.go"))
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
        let uses = facts(&documents, true).symbols;
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].name, "Client");
        assert_eq!(uses[0].specifier, "example.com/app/internal/db");
        assert_eq!(
            uses[0].target,
            Target::Internal(PathBuf::from("internal/db/db.go"))
        );
        assert_eq!(uses[1].target, Target::External("fmt".into()));
        assert!(facts(&documents, false).symbols.is_empty());
    }

    #[test]
    fn typescript_externals_use_the_npm_name() {
        assert_eq!(npm_name("@types/lodash"), "lodash");
        assert_eq!(npm_name("@types/babel__core"), "@babel/core");
        assert_eq!(npm_name("react"), "react");
        let documents = vec![doc(
            "src/a.ts",
            &[],
            &[(1, "scip-typescript npm @types/lodash 4.0.0 `index.d.ts`/")],
        )];
        let facts = facts(&documents, false);
        assert_eq!(facts.deps[0].target, Target::External("lodash".into()));
        let deep = split("scip-typescript npm moment 2.0.0 locale/`ru.d.ts`/").unwrap();
        assert_eq!(deep.external_module(), "moment/locale/ru");
    }

    #[test]
    fn documents_outside_the_project_are_dropped_and_paths_rebased() {
        let mut index = Index::new();
        for path in ["a.go", "../../go-build/x.go"] {
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
        let documents = parse(&bytes, Path::new("services/api")).unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].path, "services/api/a.go");
        assert_eq!(documents[0].references[0].line, 5);
    }
}
