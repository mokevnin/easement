//! `package.json`: the dependency tables, and the name a package of the project is
//! known by.

use std::collections::BTreeSet;

use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};

use super::{Declared, Declares, Framework, Setting, Source, line_at};
use crate::lang::parts::{Kind, Part};
use crate::lang::{Form, typescript};

const SECTIONS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// The dependencies a `package.json` declares, with the line of each; `None` if it is
/// no JSON.
pub(super) fn declared(text: &str) -> Option<Vec<Declared>> {
    let strict = ParseOptions {
        allow_comments: false,
        allow_loose_object_property_names: false,
        allow_trailing_commas: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    };
    let document = parse_to_ast(text, &CollectOptions::default(), &strict).ok()?;
    let mut found = Vec::new();
    let value = document.value?;
    let Some(root) = value.as_object() else {
        return Some(found);
    };
    for section in &root.properties {
        let name = section.name.as_str().to_string();
        if !SECTIONS.contains(&name.as_str()) {
            continue;
        }
        for dependency in section
            .value
            .as_object()
            .into_iter()
            .flat_map(|o| &o.properties)
        {
            let Some(version) = dependency.value.as_string_lit() else {
                continue;
            };
            let dependency_name = dependency.name.as_str();
            let (package, source) = self::dependency(dependency_name, &version.value);
            found.push(Declared {
                part: (name == "devDependencies").then(|| Part {
                    name: name.clone(),
                    kind: Some(Kind::Test),
                }),
                ..Declared::new(
                    line_at(text, dependency.range.start),
                    dependency_name,
                    &package,
                    source,
                )
            });
        }
    }
    Some(found)
}

/// The package a dependency is and where it comes from, by what its version says: a
/// path (`file:`, `link:`, `workspace:../x`), an alias (`npm:real@1`, `workspace:real@*`)
/// or a range of the package that goes by its name.
fn dependency(name: &str, version: &str) -> (String, Source) {
    let alias = |text: &str| match text.rfind('@') {
        Some(at) if at > 0 => text[..at].to_string(),
        _ => name.to_string(),
    };
    if let Some(path) = version
        .strip_prefix("file:")
        .or_else(|| version.strip_prefix("link:"))
    {
        return (name.to_string(), Source::Path(path.to_string()));
    }
    if let Some(rest) = version.strip_prefix("workspace:") {
        return if rest.starts_with('.') {
            (name.to_string(), Source::Path(rest.to_string()))
        } else {
            (alias(rest), Source::Package)
        };
    }
    if let Some(rest) = version.strip_prefix("npm:") {
        let real = match rest.rfind('@') {
            Some(at) if at > 0 => rest[..at].to_string(),
            _ => rest.to_string(),
        };
        return (real, Source::Package);
    }
    (name.to_string(), Source::Package)
}

/// The name of the package a `package.json` declares.
pub(super) fn name(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    json["name"].as_str().map(str::to_string)
}

/// The files of the project a `package.json` in `dir` names as what its package
/// runs or exports: `main`, `module`, `types`, `bin`, `exports` and `browser`.
pub(super) fn entries(text: &str, dir: &str, files: &BTreeSet<&str>) -> Option<Vec<String>> {
    let json = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let mut named = Vec::new();
    for key in [
        "main", "module", "types", "typings", "bin", "exports", "browser",
    ] {
        collect_strings(&json[key], &mut named);
    }
    named
        .into_iter()
        .filter_map(|path| {
            let spec = if path.starts_with('.') {
                path
            } else {
                format!("./{path}")
            };
            typescript::resolve(dir, &spec, files)
        })
        .collect::<Vec<_>>()
        .into()
}

/// The frameworks a package can depend on, and the files each loads by
/// convention. A later framework is one more row.
const FRAMEWORKS: &[Framework] = &[
    Framework {
        name: "SvelteKit",
        dependency: "@sveltejs/kit",
        entries: &[
            "src/routes/**/{+page,+page.server,+layout,+layout.server,+server,+error}.*",
            "src/hooks.*",
            "src/hooks/index.*",
            "src/hooks.*/index.*",
            "src/instrumentation.server.*",
            "src/params/*",
            "src/service-worker.*",
            "src/service-worker/index.*",
            "src/app.d.ts",
            "svelte.config.*",
            "vite.config.*",
        ],
        configs: &["svelte.config.*"],
        moves: &[Setting {
            name: "kit.files",
            key: "files",
        }],
    },
    Framework {
        name: "Next.js",
        dependency: "next",
        entries: &[
            "{app,src/app}/**/{page,layout,template,loading,error,global-error,not-found,forbidden,unauthorized,route,default}.*",
            "{app,src/app}/**/{opengraph-image,twitter-image,icon,apple-icon,sitemap,robots,manifest}.*",
            "{pages,src/pages}/**",
            "{middleware,proxy,instrumentation,instrumentation-client}.*",
            "src/{middleware,proxy,instrumentation,instrumentation-client}.*",
            "next.config.*",
        ],
        configs: &["next.config.*"],
        moves: &[Setting {
            name: "pageExtensions",
            key: "pageExtensions",
        }],
    },
];

/// The frameworks a `package.json` lists in `dependencies` or `devDependencies`.
pub(super) fn frameworks(text: &str) -> Vec<&'static Framework> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    FRAMEWORKS
        .iter()
        .filter(|framework| {
            ["dependencies", "devDependencies"]
                .iter()
                .any(|section| !json[section][framework.dependency].is_null())
        })
        .collect()
}

fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

/// The package an external module belongs to: `lodash` for `lodash/fp`,
/// `@scope/pkg` for `@scope/pkg/x`.
pub(super) fn package(module: &str) -> String {
    let mut parts = module.split('/');
    let first = parts.next().unwrap_or_default();
    match parts.next() {
        Some(second) if first.starts_with('@') => format!("{first}/{second}"),
        _ => first.to_string(),
    }
}

/// The package `specifier` names when it is spelled as an npm package: a name,
/// scoped or not, of the characters npm takes (letters, digits, `-`, `.`, `_`,
/// neither of the last two first), with any path after it.
pub(super) fn named_by(specifier: &str) -> Option<String> {
    let valid = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['.', '_'])
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    };
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    match first.strip_prefix('@') {
        Some(scope) => {
            let name = parts.next()?;
            (valid(scope) && valid(name)).then(|| format!("{first}/{name}"))
        }
        None => valid(first).then(|| first.to_string()),
    }
}

/// What the files under a `package.json` may import: the names of its
/// dependencies, the packages whose types an `@types/` one declares, and its own
/// name. A `package.json` with neither name nor dependency table only marks its
/// directory (`{"type": "module"}`) and is no package of its own.
pub(super) fn importable(text: &str) -> Declares {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return Declares::Unreadable;
    };
    if json["name"].is_null() && SECTIONS.iter().all(|s| json[s].is_null()) {
        return Declares::Marker;
    }
    let mut names: BTreeSet<String> = name(text).into_iter().collect();
    for dependency in declared(text).unwrap_or_default() {
        // `@types/node` declares the types of `node`, `@types/a__b` of `@a/b`.
        if dependency.name.starts_with("@types/") {
            names.insert(npm_name(&dependency.name));
        }
        names.insert(dependency.name);
    }
    Declares::Package(names)
}

/// `@types/lodash` is `lodash`, `@types/scope__name` is `@scope/name`.
pub(crate) fn npm_name(package: &str) -> String {
    match package.strip_prefix("@types/") {
        Some(rest) => match rest.split_once("__") {
            Some((scope, name)) => format!("@{scope}/{name}"),
            None => rest.to_string(),
        },
        None => package.to_string(),
    }
}

/// The packages TypeScript or JavaScript source names in an import, a `require`
/// or an `import()` it writes, not in a comment or a string; a relative or
/// absolute path names a file instead.
pub(super) fn named_in_source(file: &str, source: &str) -> BTreeSet<String> {
    typescript::written_imports(file, source)
        .into_iter()
        .filter(|import| import.form != Form::Computed)
        .map(|import| import.specifier)
        .filter(|specifier| !specifier.starts_with(['.', '/']))
        .map(|specifier| self::package(&specifier))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::Source;
    use super::*;

    type Row = (usize, String, String, Source, Option<String>);

    fn rows(text: &str) -> Vec<Row> {
        declared(text)
            .unwrap()
            .into_iter()
            .map(|d| (d.line, d.name, d.package, d.source, d.part.map(|p| p.name)))
            .collect()
    }

    #[test]
    fn a_package_json_declares_a_dependency_on_the_line_of_its_name() {
        let text = concat!(
            "{\n",                                               // 1
            "  \"name\": \"web\",\n",                            // 2
            "  \"dependencies\": {\n",                           // 3
            "    \"api\": \"workspace:*\",\n",                   // 4
            "    \"lodash\": \"^4\",\n",                         // 5
            "    \"local\": \"file:../local\",\n",               // 6
            "    \"alias\": \"npm:real-pkg@^1\"\n",              // 7
            "  },\n",                                            // 8
            "  \"devDependencies\": {\n",                        // 9
            "    \"testkit\": \"workspace:^\"\n",                // 10
            "  },\n",                                            // 11
            "  \"peerDependencies\": { \"react\": \"^18\" },\n", // 12
            "  \"optionalDependencies\": {\n",                   // 13
            "    \"fsevents\": \"^2\"\n",                        // 14
            "  },\n",                                            // 15
            "  \"scripts\": { \"build\": \"tsc\" }\n",           // 16
            "}\n",                                               // 17
        );
        let n = |x: &str| x.to_string();
        assert_eq!(
            rows(text),
            [
                (4, n("api"), n("api"), Source::Package, None),
                (5, n("lodash"), n("lodash"), Source::Package, None),
                (6, n("local"), n("local"), Source::Path(n("../local")), None),
                (7, n("alias"), n("real-pkg"), Source::Package, None),
                (
                    10,
                    n("testkit"),
                    n("testkit"),
                    Source::Package,
                    Some(n("devDependencies"))
                ),
                (12, n("react"), n("react"), Source::Package, None),
                (14, n("fsevents"), n("fsevents"), Source::Package, None),
            ]
        );
        let kinds: Vec<_> = declared(text)
            .unwrap()
            .into_iter()
            .map(|d| d.part.and_then(|p| p.kind))
            .collect();
        assert_eq!(kinds[4], Some(crate::lang::parts::Kind::Test));
        assert_eq!(kinds[5], None);
    }

    #[test]
    fn a_scoped_alias_keeps_its_scope_and_a_name_is_read() {
        let text = "{\n  \"name\": \"@acme/web\",\n  \"dependencies\": { \"a\": \"npm:@acme/real@1.0.0\" }\n}\n";
        assert_eq!(rows(text)[0].2, "@acme/real");
        assert_eq!(name(text), Some("@acme/web".to_string()));
    }

    #[test]
    fn a_package_json_that_is_no_json_is_unread_not_empty() {
        assert!(declared("{ \"dependencies\": ").is_none());
        assert_eq!(declared("{}").map(|v| v.len()), Some(0));
        assert_eq!(name("{}"), None);
    }

    #[test]
    fn a_key_with_a_quote_or_a_brace_in_a_string_does_not_confuse_the_lines() {
        let text = "{\n  \"description\": \"a } \\\" { b\",\n  \"dependencies\": {\n    \"x\": \"1\"\n  }\n}\n";
        assert_eq!(rows(text)[0].0, 4);
    }

    #[test]
    fn the_workspace_protocol_names_a_path_or_an_alias_as_well_as_a_range() {
        let text = "{\n  \"dependencies\": {\n    \"a\": \"workspace:^\",\n    \"b\": \"workspace:../b\",\n    \"c\": \"workspace:real@*\",\n    \"d\": \"workspace:@acme/real@^1\"\n  }\n}\n";
        let got: Vec<_> = rows(text).into_iter().map(|r| (r.1, r.2, r.3)).collect();
        let n = |x: &str| x.to_string();
        assert_eq!(
            got,
            [
                (n("a"), n("a"), Source::Package),
                (n("b"), n("b"), Source::Path(n("../b"))),
                (n("c"), n("real"), Source::Package),
                (n("d"), n("@acme/real"), Source::Package),
            ]
        );
    }
}
