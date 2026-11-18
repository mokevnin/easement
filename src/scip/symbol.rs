//! Reads a SCIP symbol into the shape every language shares ([`Name`]). What
//! differs by scheme is said by the entry of the language whose indexer writes it
//! ([`registry::Symbols`]): data, or a hook in `lang/<language>.rs`. This module
//! holds what every scheme shares: the symbol split into its fields and
//! descriptors, and the readings of modules and of classes those entries choose.

use std::borrow::Cow;

use crate::lang::registry::{self, Symbols};

/// How a descriptor of a symbol ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Namespace,
    /// A class, interface or other type: `Name#`.
    Type,
    /// A type parameter `[T]` or a parameter `(p)`.
    Parameter,
    Other,
}

/// A symbol split into the package that owns it and its descriptors.
#[derive(Debug)]
pub struct Parsed<'a> {
    pub scheme: Cow<'a, str>,
    pub package: Cow<'a, str>,
    pub descriptors: Vec<(String, Kind)>,
}

/// `scheme manager package version descriptors`.
pub fn split(symbol: &str) -> Option<Parsed<'_>> {
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
pub fn descriptors(text: &str) -> Option<Vec<(String, Kind)>> {
    scan(text).map(|found| {
        found
            .into_iter()
            .map(|(name, kind, _)| (name, kind))
            .collect()
    })
}

/// The descriptors with the offset, in characters, at which each one ends.
pub fn scan(text: &str) -> Option<Vec<(String, Kind, usize)>> {
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
pub fn identifier(chars: &[char], mut at: usize) -> Option<(String, usize)> {
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
pub fn cut(symbol: &str, count: usize) -> String {
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
pub struct Name {
    pub shape: Shape,
    /// The symbol whose definition is the name's home: the symbol itself, or the
    /// module that a constructor or a nested type stands for.
    pub symbol: String,
    /// For a member, the name it belongs to, whose home stands for the member's
    /// when the index defines no member.
    pub owner: Option<String>,
    /// The package that owns the symbol, with the scheme: what tells the project's own
    /// symbols from those of its dependencies. `None` when the index gives both the same.
    pub package: Option<String>,
    /// The names of the module the symbol is, or lives in.
    pub path: Vec<String>,
    /// The names below that module: the top-level name, then the member.
    pub inner: Vec<String>,
    /// The external module, in the spelling of the language's imports and of `forbid_external`.
    pub external: String,
    /// What `forbid_symbol` calls it; `None` for a module that is a place, not a name
    /// (a Go package, a TypeScript file).
    pub called: Option<Called>,
    pub reading: Reading,
}

/// What a name is to the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
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
pub struct Called {
    pub qualifier: Qualifier,
    pub name: String,
    /// The member of the name that is used: a method, a variant, a constant.
    pub member: Option<Member>,
}

/// A member of a name, and what `forbid_symbol` calls it through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub through: Through,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Through {
    /// As the index spells it: `Shop::Db::Client`.
    Spelled(String),
    /// The qualifier of the name and the name, joined by this: `db::client` `::`
    /// `Client`, `app/db` `.` `Client`.
    Joined(&'static str),
}

impl Called {
    /// The qualifier of the name as the symbol rules write it: as the index spells it,
    /// else the module path of `file`, the file that defines it, when one does.
    pub fn qualifier_in(&self, file: Option<&str>) -> String {
        match (&self.qualifier, file) {
            (Qualifier::Spelled(qualifier), _) => qualifier.clone(),
            (Qualifier::File(_), Some(file)) => registry::module_path(file).to_string(),
            (Qualifier::File(otherwise), None) => otherwise.clone(),
        }
    }

    /// The member used, as `forbid_symbol` names it: what it is called through, and
    /// its name. `specifier` is the qualifier of the name as the use spells it.
    pub fn method(&self, specifier: &str) -> Option<(String, String)> {
        let member = self.member.as_ref()?;
        let through = match &member.through {
            Through::Spelled(owner) => owner.clone(),
            Through::Joined(separator) => format!("{specifier}{separator}{}", self.name),
        };
        Some((through, member.name.clone()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Qualifier {
    Spelled(String),
    /// The path of the file that defines the name, without its extension, or this
    /// when no file of the project does.
    File(String),
}

/// What the index of a language says of its symbols besides their shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    /// A line that writes a path also references each module around it
    /// (`crate::domain::order`, `Shop::Domain::Order`): the line depends on the deepest.
    pub enclosing: bool,
    /// A definition names its module by its path from the working directory
    /// (`src.shop.infra`) and a reference as imported (`shop.infra`), so a reference is
    /// matched by the ending of its path.
    pub by_ending: bool,
    /// The index does not say which package the symbol is in; only the string of
    /// the import does (see [`registry::UnnamedPackages`]).
    pub unnamed: bool,
}

/// `symbol` in the shape every language shares, as the entry of the language whose
/// indexer writes its scheme reads it. `None` for a symbol that does not split, or
/// of a scheme no entry declares: nothing is made of it.
pub fn read(symbol: &str) -> Option<Name> {
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

/// The names of the descriptors.
pub fn names(parsed: &Parsed) -> Vec<String> {
    parsed
        .descriptors
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// How many descriptors from `from` on are of `kind`.
pub fn leading(parsed: &Parsed, from: usize, kind: Kind) -> usize {
    parsed.descriptors[from..]
        .iter()
        .take_while(|(_, k)| *k == kind)
        .count()
}

/// A symbol of a language whose modules are namespaces, as a hook of its entry
/// sees it: the hook settles what its language reads differently and gives the
/// qualifier its names are called by.
pub struct Module<'a> {
    pub symbol: &'a str,
    pub parsed: &'a Parsed<'a>,
    /// How many descriptors, from the first, are the module's namespaces.
    pub modules: usize,
    /// The names of the module.
    pub path: Vec<String>,
    /// The names below the module: the top-level name, then the member.
    pub inner: Vec<String>,
    pub reading: Reading,
}

/// Go, TypeScript, Python and Rust: modules are namespaces, and a name at their top
/// level is the first descriptor past them. `qualifier` and `external` are the hooks
/// of the language's entry; `member_separator` joins a type and its member.
pub fn modules(
    symbol: &str,
    parsed: &Parsed,
    member_separator: &'static str,
    qualifier: fn(&mut Module<'_>) -> Qualifier,
    external: fn(&Parsed<'_>) -> String,
) -> Name {
    let names = names(parsed);
    let modules = leading(parsed, 0, Kind::Namespace);
    let mut module = Module {
        symbol,
        parsed,
        modules,
        path: names[..modules].to_vec(),
        inner: names[modules..].to_vec(),
        reading: Reading::default(),
    };
    let qualifier = qualifier(&mut module);
    let Module {
        path,
        inner,
        reading,
        ..
    } = module;
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
                through: Through::Joined(member_separator),
                name: member.clone(),
            }),
        }),
        external: external(parsed),
        path,
        inner,
        reading,
    }
}

/// Java, C# and PHP: a namespace is no module; a class is what an import names, and
/// a type nested in it is reached through it. `constructor` is the member that stands
/// for its class: the indexer records the class the code writes only as that member.
pub fn classes(
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

impl Parsed<'_> {
    /// The namespace descriptors joined: the import path of a Go package, the
    /// dotted name of a Python module, the path of a Rust module.
    pub fn namespace(&self, separator: &str) -> String {
        self.descriptors
            .iter()
            .take_while(|(_, kind)| *kind == Kind::Namespace)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// The descriptors past the namespace joined by `.`: the name of a definition
    /// within its module (`Order.total`). Empty for a module itself.
    pub fn within_namespace(&self) -> String {
        self.descriptors
            .iter()
            .filter(|(_, kind)| *kind != Kind::Namespace)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(".")
    }

    /// The package that owns the symbol, with the scheme: what tells the
    /// project's own symbols from those of its dependencies.
    pub fn owner(&self) -> String {
        format!("{} {}", self.scheme, self.package)
    }
}

/// A definition as its symbol spells it, past the package and the modules around it
/// ([`Parsed::within_namespace`]); the symbol itself when it cannot be read or names
/// a module.
pub fn shown_name(symbol: &str) -> String {
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
}
