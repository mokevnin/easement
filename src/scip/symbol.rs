//! Reads a SCIP symbol into the shape every language shares ([`Name`]): what
//! differs by scheme (`scip-ruby`, `scip-java`, `scip-dotnet`, `scip-php`, the
//! namespaces of the rest) ends here.

use std::borrow::Cow;

use super::UNNAMED_NPM;
use crate::lang::registry;
use crate::packages::npm_name;

/// How a descriptor of a symbol ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Namespace,
    /// A class, interface or other type: `Name#`.
    Type,
    /// A type parameter `[T]` or a parameter `(p)`.
    Parameter,
    Other,
}

/// A symbol split into the package that owns it and its descriptors.
#[derive(Debug)]
pub(super) struct Parsed<'a> {
    pub(super) scheme: Cow<'a, str>,
    pub(super) package: Cow<'a, str>,
    pub(super) descriptors: Vec<(String, Kind)>,
}

/// `scheme manager package version descriptors`.
pub(super) fn split(symbol: &str) -> Option<Parsed<'_>> {
    let ([scheme, _manager, package, _version], start) = header(symbol)?;
    let descriptors = descriptors(&symbol[start..])?;
    Some(Parsed {
        scheme,
        package,
        descriptors,
    })
}

/// The fields before the descriptors (`scheme manager package version`), and the
/// offset at which the descriptors start. A field ends at a single space; two
/// spaces stand for one space within the field.
fn header(symbol: &str) -> Option<([Cow<'_, str>; 4], usize)> {
    let mut fields: [Cow<'_, str>; 4] = Default::default();
    let mut start = 0;
    for field in &mut fields {
        let mut end = start;
        loop {
            end += symbol[end..].find(' ')?;
            if symbol[end + 1..].starts_with(' ') {
                end += 2;
            } else {
                break;
            }
        }
        let raw = &symbol[start..end];
        *field = match raw.contains("  ") {
            true => Cow::Owned(raw.replace("  ", " ")),
            false => Cow::Borrowed(raw),
        };
        start = end + 1;
    }
    Some((fields, start))
}

/// The descriptors of a symbol: a name, quoted in backticks when it holds
/// anything but identifier characters, and a suffix (`/` namespace, `#` type,
/// `.` term, `().` method, `:` meta, `!` macro, `[T]` type parameter, `(p)` parameter).
pub(super) fn descriptors(text: &str) -> Option<Vec<(String, Kind)>> {
    scan(text).map(|found| {
        found
            .into_iter()
            .map(|(name, kind, _)| (name, kind))
            .collect()
    })
}

/// The descriptors with the offset, in characters, at which each one ends.
pub(super) fn scan(text: &str) -> Option<Vec<(String, Kind, usize)>> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let mut found = Vec::new();
    while at < chars.len() {
        // A type parameter `[T]` or a parameter `(p)` is its name in brackets; the name
        // may be quoted and hold a bracket (rust-analyzer's ``impl#[`[T]`]``).
        if let Some(close) = match chars[at] {
            '[' => Some(']'),
            '(' => Some(')'),
            _ => None,
        } {
            let (name, end) = identifier(&chars, at + 1)?;
            if chars.get(end) != Some(&close) {
                return None;
            }
            at = end + 1;
            found.push((name, Kind::Parameter, at));
            continue;
        }
        let (name, end) = identifier(&chars, at)?;
        at = end;
        let kind = match chars.get(at)? {
            '/' => {
                at += 1;
                Kind::Namespace
            }
            '#' => {
                at += 1;
                Kind::Type
            }
            '.' | ':' | '!' => {
                at += 1;
                Kind::Other
            }
            '(' => {
                // `name().` or `name(disambiguator).`
                let close = chars[at..].iter().position(|c| *c == ')')? + at;
                at = close + 1;
                if chars.get(at) == Some(&'.') {
                    at += 1;
                }
                Kind::Other
            }
            _ => return None,
        };
        found.push((name, kind, at));
    }
    Some(found)
}

/// The name that starts at `at`, quoted in backticks (a doubled one stands for itself)
/// or of identifier characters, and the offset just past it.
pub(super) fn identifier(chars: &[char], mut at: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    if chars.get(at) == Some(&'`') {
        at += 1;
        loop {
            match chars.get(at)? {
                '`' if chars.get(at + 1) == Some(&'`') => {
                    name.push('`');
                    at += 2;
                }
                '`' => return Some((name, at + 1)),
                c => {
                    name.push(*c);
                    at += 1;
                }
            }
        }
    }
    while let Some(c) = chars.get(at)
        && (c.is_alphanumeric() || matches!(c, '_' | '+' | '-' | '$'))
    {
        name.push(*c);
        at += 1;
    }
    Some((name, at))
}

/// `symbol` with only its first `count` descriptors.
pub(super) fn cut(symbol: &str, count: usize) -> String {
    let start = header(symbol).map_or(symbol.len(), |(_, start)| start);
    let text = &symbol[start..];
    let end = match count.checked_sub(1) {
        None => 0,
        Some(last) => scan(text)
            .and_then(|found| found.get(last).map(|(_, _, end)| *end))
            .unwrap_or_else(|| text.chars().count()),
    };
    format!(
        "{}{}",
        &symbol[..start],
        text.chars().take(end).collect::<String>()
    )
}

/// What a symbol names, read the same way in every language. [`read`] turns the
/// symbol of each scheme into this shape; past it, nothing tells one language from
/// another, and [`super::Indexed::facts`] makes the same facts of the same shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Name {
    pub(super) shape: Shape,
    /// The symbol whose definition is the name's home: the symbol itself, or the
    /// module that a constructor or a nested type stands for.
    pub(super) symbol: String,
    /// For a member, the name it belongs to, whose home stands for the member's
    /// when the index defines no member.
    pub(super) owner: Option<String>,
    /// The package that owns the symbol, with the scheme: what tells the project's own
    /// symbols from those of its dependencies. `None` when the index gives both the same.
    pub(super) package: Option<String>,
    /// The names of the module the symbol is, or lives in.
    pub(super) path: Vec<String>,
    /// The names below that module: the top-level name, then the member.
    pub(super) inner: Vec<String>,
    /// The external module, in the spelling of the language's imports and of `forbid_external`.
    pub(super) external: String,
    /// What `forbid_symbol` calls it; `None` for a module that is a place, not a name
    /// (a Go package, a TypeScript file).
    pub(super) called: Option<Called>,
    pub(super) reading: Reading,
}

/// What a name is to the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Shape {
    /// A package or namespace named on its own, which is no dependency.
    Package,
    /// The unit names live in, a dependency even when it is external: a Go package, a
    /// TypeScript file, a Python or Rust module, a Java, C# or PHP class, a Ruby constant.
    Module,
    /// A name at the top level of a module (a class, a function, a constant): a
    /// dependency on the file that defines it. An external one is reached through its module.
    TopLevel,
    /// A member reached through a value (a method, a field): a symbol use only.
    Member,
    /// A declaration that stands for modules it does not name, and a name in one: a
    /// TypeScript wildcard ambient module (`declare module "*.css"`). Neither a
    /// dependency nor a use: the file an import of it means is not in the index.
    Wildcard,
}

/// How `forbid_symbol` and `public_symbols` call a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Called {
    pub(super) qualifier: Qualifier,
    pub(super) name: String,
    /// The member of the name that is used: a method, a variant, a constant.
    pub(super) member: Option<Member>,
}

/// A member of a name, and what `forbid_symbol` calls it through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Member {
    pub(super) through: Through,
    pub(super) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Through {
    /// As the index spells it: `Shop::Db::Client`.
    Spelled(String),
    /// The qualifier of the name and the name, joined by this: `db::client` `::`
    /// `Client`, `app/db` `.` `Client`.
    Joined(&'static str),
}

impl Called {
    /// The qualifier of the name as the symbol rules write it: as the index spells it,
    /// else the module path of `file`, the file that defines it, when one does.
    pub(super) fn qualifier_in(&self, file: Option<&str>) -> String {
        match (&self.qualifier, file) {
            (Qualifier::Spelled(qualifier), _) => qualifier.clone(),
            (Qualifier::File(_), Some(file)) => registry::module_path(file).to_string(),
            (Qualifier::File(otherwise), None) => otherwise.clone(),
        }
    }

    /// The member used, as `forbid_symbol` names it: what it is called through, and
    /// its name. `specifier` is the qualifier of the name as the use spells it.
    pub(super) fn method(&self, specifier: &str) -> Option<(String, String)> {
        let member = self.member.as_ref()?;
        let through = match &member.through {
            Through::Spelled(owner) => owner.clone(),
            Through::Joined(separator) => format!("{specifier}{separator}{}", self.name),
        };
        Some((through, member.name.clone()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Qualifier {
    Spelled(String),
    /// The path of the file that defines the name, without its extension, or this
    /// when no file of the project does.
    File(String),
}

/// What the index of a language says of its symbols besides their shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Reading {
    /// A line that writes a path also references each module around it
    /// (`crate::domain::order`, `Shop::Domain::Order`): the line depends on the deepest.
    pub(super) enclosing: bool,
    /// A definition names its module by its path from the working directory
    /// (`src.shop.infra`) and a reference as imported (`shop.infra`), so a reference is
    /// matched by the ending of its path.
    pub(super) by_ending: bool,
    /// The index does not say which package the symbol is in; only the string of
    /// the import does (see `UNNAMED_NPM`).
    pub(super) unnamed: bool,
}

/// `symbol` in the shape every language shares.
pub(super) fn read(symbol: &str) -> Option<Name> {
    let parsed = split(symbol)?;
    Some(match parsed.scheme.as_ref() {
        "scip-ruby" => ruby(symbol, &parsed),
        // scip-java gives `new X()` only as the constructor of `X`.
        "scip-java" => classes(symbol, &parsed, ".", Some("<init>"), Some(parsed.owner())),
        // scip-dotnet gives an attribute `[X]` only as the constructor of `XAttribute`, and
        // the project's own types and the framework's the same header.
        "scip-dotnet" => classes(symbol, &parsed, ".", Some(".ctor"), None),
        "scip-php" => classes(symbol, &parsed, "\\", None, Some(parsed.owner())),
        _ => modules(symbol, &parsed),
    })
}

/// The names of the descriptors.
pub(super) fn names(parsed: &Parsed) -> Vec<String> {
    parsed
        .descriptors
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// How many descriptors from `from` on are of `kind`.
pub(super) fn leading(parsed: &Parsed, from: usize, kind: Kind) -> usize {
    parsed.descriptors[from..]
        .iter()
        .take_while(|(_, k)| *k == kind)
        .count()
}

/// Go, TypeScript, Python and Rust: modules are namespaces, and a name at their top
/// level is the first descriptor past them.
pub(super) fn modules(symbol: &str, parsed: &Parsed) -> Name {
    let names = names(parsed);
    let modules = leading(parsed, 0, Kind::Namespace);
    let mut path = names[..modules].to_vec();
    let mut inner = names[modules..].to_vec();
    let mut reading = Reading::default();
    let qualifier = match parsed.scheme.as_ref() {
        "scip-go" => Qualifier::Spelled(parsed.namespace("/")),
        "scip-python" => {
            // A module is the term `__init__` of its namespace.
            if modules == 1 && inner == ["__init__"] {
                inner.clear();
            }
            path = path
                .iter()
                .flat_map(|name| name.split('.'))
                .map(str::to_string)
                .collect();
            reading.by_ending = true;
            Qualifier::Spelled(parsed.namespace("."))
        }
        "rust-analyzer" => {
            // `crate/` is the root of the crate, around its modules (`domain/order/`).
            path.retain(|name| name != "crate");
            // An `impl` block names no type of its own: `impl#[Block]size()` (and
            // `impl#[Block][Display]fmt()` for a trait) is `size` of `Block`.
            if let [(block, Kind::Type), (of, Kind::Parameter), rest @ ..] =
                &parsed.descriptors[modules..]
                && block == "impl"
            {
                inner = std::iter::once(of)
                    .chain(
                        rest.iter()
                            .skip_while(|(_, kind)| *kind == Kind::Parameter)
                            .map(|(name, _)| name),
                    )
                    .cloned()
                    .collect();
            }
            reading.enclosing = true;
            Qualifier::Spelled(parsed.external_module())
        }
        _ => {
            reading.unnamed = symbol.starts_with(UNNAMED_NPM);
            Qualifier::File(npm_name(&parsed.package))
        }
    };
    // scip-typescript names an ambient module by its string, quotes and all
    // (`"*.css"`): one with a wildcard stands for the files it matches.
    let wildcard = matches!(qualifier, Qualifier::File(_))
        && path
            .iter()
            .any(|name| name.starts_with('"') && name.contains('*'));
    let shape = match inner.len() {
        _ if wildcard => Shape::Wildcard,
        0 => Shape::Module,
        1 => Shape::TopLevel,
        _ => Shape::Member,
    };
    Name {
        shape,
        symbol: symbol.to_string(),
        owner: (shape == Shape::Member).then(|| cut(symbol, modules + 1)),
        package: Some(parsed.owner()),
        called: inner.first().filter(|_| !wildcard).map(|name| Called {
            qualifier,
            name: name.clone(),
            member: inner.get(1).map(|member| Member {
                // A Rust path joins a type and what is in it as it joins modules,
                // as the registry says for a name an example writes.
                through: Through::Joined(if parsed.scheme == "rust-analyzer" {
                    registry::RUST.member_separator
                } else {
                    "."
                }),
                name: member.clone(),
            }),
        }),
        external: parsed.external_module(),
        path,
        inner,
        reading,
    }
}

/// Java, C# and PHP: a namespace is no module; a class is what an import names, and
/// a type nested in it is reached through it. `constructor` is the member that stands
/// for its class: the indexer records the class the code writes only as that member.
pub(super) fn classes(
    symbol: &str,
    parsed: &Parsed,
    separator: &str,
    constructor: Option<&str>,
    package: Option<String>,
) -> Name {
    let names = names(parsed);
    let namespaces = leading(parsed, 0, Kind::Namespace);
    let types = leading(parsed, namespaces, Kind::Type);
    let namespace = names[..namespaces].join(separator);
    let below = &names[namespaces + types..];
    let shape = match (types, below) {
        _ if namespaces == names.len() => Shape::Package,
        (0, [_]) => Shape::TopLevel,
        (0, _) => Shape::Member,
        (_, []) => Shape::Module,
        (_, [member]) if Some(member.as_str()) == constructor => Shape::Module,
        _ => Shape::Member,
    };
    let class = match types {
        0 => symbol.to_string(),
        _ => cut(symbol, namespaces + 1),
    };
    let path = names[..(namespaces + 1).min(names.len())].to_vec();
    Name {
        shape,
        symbol: match shape {
            Shape::Module => class.clone(),
            _ => symbol.to_string(),
        },
        owner: (shape == Shape::Member).then_some(class),
        package,
        called: (shape != Shape::Package).then(|| Called {
            qualifier: Qualifier::Spelled(namespace),
            name: names[namespaces].clone(),
            member: None,
        }),
        external: path.join(separator),
        inner: match shape {
            Shape::Module => Vec::new(),
            _ => names[(namespaces + 1).min(names.len())..].to_vec(),
        },
        path,
        reading: Reading::default(),
    }
}

/// Ruby: modules and classes are both types, and a constant names one; everything
/// below it is a member (`Shop#Web#Order#run().`). A class method belongs to the
/// singleton class (``Shop#Web#`<Class:Order>`#find().``), which stands for the class.
pub(super) fn ruby(symbol: &str, parsed: &Parsed) -> Name {
    let names = names(parsed);
    let types = leading(parsed, 0, Kind::Type);
    let mut path = names[..types].to_vec();
    let mut class = cut(symbol, types);
    if let Some(last) = path.last_mut()
        && let Some(singleton) = last
            .strip_prefix("<Class:")
            .and_then(|name| name.strip_suffix('>'))
            .map(str::to_string)
    {
        class = format!("{}{singleton}#", cut(symbol, types - 1));
        *last = singleton;
    }
    let inner = names[types..].to_vec();
    let shape = match (types, inner.is_empty()) {
        (0, _) => Shape::Package,
        (_, true) => Shape::Module,
        (_, false) => Shape::Member,
    };
    let qualified = path.join("::");
    Name {
        shape,
        symbol: match shape {
            Shape::Module => class.clone(),
            _ => symbol.to_string(),
        },
        owner: (shape == Shape::Member).then_some(class),
        package: Some(parsed.owner()),
        called: path.split_last().map(|(name, outer)| Called {
            qualifier: Qualifier::Spelled(outer.join("::")),
            name: name.clone(),
            member: inner.first().map(|member| Member {
                through: Through::Spelled(qualified.clone()),
                name: member.clone(),
            }),
        }),
        external: qualified,
        path,
        inner,
        reading: Reading {
            enclosing: true,
            ..Reading::default()
        },
    }
}

impl Parsed<'_> {
    /// The namespace descriptors joined: the import path of a Go package, the
    /// dotted name of a Python module, the path of a Rust module.
    pub(super) fn namespace(&self, separator: &str) -> String {
        self.descriptors
            .iter()
            .take_while(|(_, kind)| *kind == Kind::Namespace)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// The descriptors past the namespace joined by `.`: the name of a definition
    /// within its module (`Order.total`). Empty for a module itself.
    pub(super) fn within_namespace(&self) -> String {
        self.descriptors
            .iter()
            .filter(|(_, kind)| *kind != Kind::Namespace)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }

    /// The package that owns the symbol, with the scheme: what tells the
    /// project's own symbols from those of its dependencies.
    pub(super) fn owner(&self) -> String {
        format!("{} {}", self.scheme, self.package)
    }

    /// The external module the symbol of a language whose modules are namespaces
    /// belongs to, in the spelling of its import statements and of `forbid_external`.
    pub(super) fn external_module(&self) -> String {
        match self.scheme.as_ref() {
            // Go: the package path (`fmt`, `net/http`, `github.com/lib/pq`).
            "scip-go" => self.namespace("/"),
            // Python: the dotted module (`os.path`, `requests`).
            "scip-python" => self.namespace("."),
            // Rust: the crate and the module path; the standard library is
            // spread over `core`, `alloc` and `std` but written `std::…`.
            "rust-analyzer" => {
                let krate = match self.package.as_ref() {
                    "core" | "alloc" | "std" => "std".to_string(),
                    other => other.replace('-', "_"),
                };
                std::iter::once(krate)
                    .chain(
                        self.descriptors
                            .iter()
                            .take_while(|(_, kind)| *kind == Kind::Namespace)
                            .map(|(name, _)| name.clone())
                            .filter(|name| name != "crate"),
                    )
                    .collect::<Vec<_>>()
                    .join("::")
            }
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
                    *last = registry::TYPESCRIPT.module_path(last).to_string();
                    if last == "index" {
                        parts.pop();
                    }
                }
                std::iter::once(npm_name(&self.package))
                    .chain(parts)
                    .collect::<Vec<_>>()
                    .join("/")
            }
        }
    }
}

/// A definition as its symbol spells it, past the package and the modules around it
/// ([`Parsed::within_namespace`]); the symbol itself when it cannot be read or names
/// a module.
pub(super) fn shown_name(symbol: &str) -> String {
    split(symbol)
        .map(|parsed| parsed.within_namespace())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| symbol.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_is_the_last_descriptor_ending_in_a_hash() {
        let last = |symbol: &str| {
            split(symbol).and_then(|p| {
                p.descriptors
                    .last()
                    .map(|(n, k)| (n.clone(), matches!(k, Kind::Type)))
            })
        };
        assert_eq!(
            last("scip-go gomod ex.com/m . `ex.com/m/repo`/OrderRepository#"),
            Some(("OrderRepository".to_string(), true))
        );
        assert_eq!(
            last("semanticdb maven . . m/Outer#Inner#"),
            Some(("Inner".to_string(), true))
        );
        assert_eq!(
            last("scip-go gomod ex.com/m . `ex.com/m/repo`/OrderRepository#Find()."),
            Some(("Find".to_string(), false))
        );
    }

    #[test]
    fn parses_descriptors() {
        let d = descriptors("src/lib/`index.ts`/Client#Query().").unwrap();
        let names: Vec<_> = d.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["src", "lib", "index.ts", "Client", "Query"]);
        assert_eq!(d[2].1, Kind::Namespace);
        assert_eq!(d[3].1, Kind::Type);
        assert_eq!(d[4].1, Kind::Other);
        let go = split("scip-go gomod example.com/app . `example.com/app/internal/db`/Scoped().")
            .unwrap();
        assert_eq!(go.namespace("/"), "example.com/app/internal/db");
        let scoped =
            read("scip-go gomod example.com/app . `example.com/app/internal/db`/Scoped().")
                .unwrap();
        assert_eq!(
            (scoped.shape, scoped.inner),
            (Shape::TopLevel, vec!["Scoped".to_string()])
        );
        assert_eq!(
            read("scip-go gomod example.com/app . `example.com/app/internal/db`/")
                .unwrap()
                .shape,
            Shape::Module
        );
    }

    #[test]
    fn a_bracketed_descriptor_names_what_it_holds_even_when_quoted() {
        let names = |text: &str| -> Vec<String> {
            descriptors(text)
                .unwrap()
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        };
        // rust-analyzer quotes the type an `impl` is for, and it may hold `]`.
        assert_eq!(
            names("slice/impl#[`[T]`]join()."),
            ["slice", "impl", "[T]", "join"]
        );
        assert_eq!(
            names("result/impl#[`Result<T, E>`]unwrap()."),
            ["result", "impl", "Result<T, E>", "unwrap"]
        );
        assert_eq!(names("Block#size().(self)"), ["Block", "size", "self"]);
        let join = "rust-analyzer cargo alloc https://github.com/rust-lang/rust/library/alloc slice/impl#[`[T]`]join().";
        assert_eq!(read(join).unwrap().external, "std::slice");
        assert_eq!(
            cut(join, 3),
            "rust-analyzer cargo alloc https://github.com/rust-lang/rust/library/alloc slice/impl#[`[T]`]"
        );
    }

    #[test]
    fn a_rust_method_of_an_impl_block_is_a_member_of_its_type() {
        let called = |symbol: &str| {
            let called = read(symbol).unwrap().called.unwrap();
            (called.name, called.member.map(|member| member.name))
        };
        let size = || ("Block".to_string(), Some("size".to_string()));
        assert_eq!(
            called("rust-analyzer cargo blob 0.1.0 Block#size()."),
            size()
        );
        assert_eq!(
            called("rust-analyzer cargo blob 0.1.0 impl#[Block]size()."),
            size()
        );
        assert_eq!(
            called("rust-analyzer cargo blob 0.1.0 store/impl#[Block][Display]fmt()."),
            ("Block".to_string(), Some("fmt".to_string()))
        );
    }

    /// A space in a field of the header is written twice, so the descriptors start
    /// after the fourth single space, and `cut` keeps the header as it is written.
    #[test]
    fn a_definition_is_shown_past_its_package_and_modules() {
        assert_eq!(
            shown_name("scip-typescript npm app 1.0.0 src/`rates.ts`/Order#total()."),
            "Order.total"
        );
        assert_eq!(
            shown_name("scip-go gomod example.com/app . `example.com/app/lib`/Area()."),
            "Area"
        );
        // A module, or what cannot be read, is shown as the index spells it.
        let module = "scip-typescript npm app 1.0.0 src/`rates.ts`/";
        assert_eq!(shown_name(module), module);
        assert_eq!(shown_name("local 0"), "local 0");
    }

    #[test]
    fn a_doubled_space_is_a_space_within_a_field() {
        let symbol = "scip-dotnet nuget My  Lib 1.0  beta Shop/Order#Total().";
        let parsed = split(symbol).unwrap();
        assert_eq!(parsed.package, "My Lib");
        assert_eq!(
            parsed.descriptors,
            [
                ("Shop".to_string(), Kind::Namespace),
                ("Order".to_string(), Kind::Type),
                ("Total".to_string(), Kind::Other),
            ]
        );
        assert_eq!(parsed.owner(), "scip-dotnet My Lib");
        assert_eq!(
            cut(symbol, 2),
            "scip-dotnet nuget My  Lib 1.0  beta Shop/Order#"
        );
    }

    /// A wildcard ambient module (`declare module "*.css"`) stands for every file its
    /// pattern matches and is none of them: neither it nor a name in it is a module,
    /// a name or a use. A module declared by its whole name (`"virtual:x"`) is one.
    #[test]
    fn a_wildcard_ambient_module_is_a_declaration_only() {
        for symbol in [
            "scip-typescript npm vite 5.0.0 `client.d.ts`/`\"*.css\"`/",
            "scip-typescript npm vite 5.0.0 `client.d.ts`/`\"*?raw\"`/src.",
            "scip-typescript npm app 1.0.0 src/`shims.d.ts`/`\"*.svelte\"`/C.",
        ] {
            let name = read(symbol).unwrap();
            assert_eq!(name.shape, Shape::Wildcard, "{symbol}");
            assert_eq!(name.called, None, "{symbol}");
        }
        let virtual_module =
            read("scip-typescript npm app 1.0.0 src/`x.d.ts`/`\"virtual:x\"`/").unwrap();
        assert_eq!(virtual_module.shape, Shape::Module);
    }

    #[test]
    fn an_npm_package_is_named_as_its_imports_spell_it() {
        let deep = split("scip-typescript npm moment 2.0.0 locale/`ru.d.ts`/").unwrap();
        assert_eq!(deep.external_module(), "moment/locale/ru");
    }

    /// The file of an npm package is named by its module path, whatever extension
    /// or declaration suffix TypeScript gives it; an `index` file is its directory.
    #[test]
    fn an_npm_file_is_named_without_any_typescript_suffix() {
        let module = |file: &str| {
            split(&format!("scip-typescript npm pkg 1.0.0 `{file}`/"))
                .unwrap()
                .external_module()
        };
        for file in [
            "foo.cjs",
            "foo.mts",
            "foo.cts",
            "foo.jsx",
            "foo.d.mts",
            "foo.d.ts",
        ] {
            assert_eq!(module(file), "pkg/foo", "{file}");
        }
        for file in ["index.cjs", "index.d.cts", "index.ts"] {
            assert_eq!(module(file), "pkg", "{file}");
        }
    }
}
