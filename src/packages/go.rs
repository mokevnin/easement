//! `go.mod`: the `require` directives, the local `replace` ones, and the module path a
//! module of the project is known by.

use std::collections::BTreeSet;
use std::str::FromStr;

use gomod_parser::{GoMod, Replacement};

use super::{Declared, Declares, Listed, Source, line_at, path_source};
use crate::lang::registry::ManifestFile;

/// What easement reads of a `go.mod`, with every path as Go reads it.
struct Mod {
    /// The module path the `go.mod` declares; empty without a `module` directive.
    module: String,
    requires: Vec<Require>,
    /// Each module replaced by a directory next to it: (module, directory).
    local: Vec<(String, String)>,
}

struct Require {
    module: String,
    /// Where the path of the module starts in the text.
    offset: usize,
    /// Marked `// indirect`: the module needs it only for a module it requires.
    indirect: bool,
}

/// The `go.mod` in `text`; `None` for one Go does not read: a block that never
/// closes, a directive Go does not know, a path in backticks.
fn parse(text: &str) -> Option<Mod> {
    let go_mod = GoMod::from_str(text).ok()?;
    let requires = go_mod
        .require
        .iter()
        .map(|dependency| {
            Some(Require {
                module: go_string(&dependency.module.module_path)?,
                offset: dependency.module.path_span.start,
                indirect: dependency.indirect,
            })
        })
        .collect::<Option<_>>()?;
    let mut local = Vec::new();
    for replace in &go_mod.replace {
        let module = go_string(&replace.module_path)?;
        if let Replacement::FilePath(dir) = &replace.replacement {
            let dir = go_string(dir)?;
            if dir.starts_with('.') {
                local.push((module, dir));
            }
        }
    }
    Some(Mod {
        module: go_string(&go_mod.module)?,
        requires,
        local,
    })
}

/// A path as Go reads it: bare, or an interpreted string (`"a/b"`), whose quotes
/// the parser keeps. `None` for a raw string (`` `a/b` ``), which Go does not take.
fn go_string(path: &str) -> Option<String> {
    if path.starts_with('`') {
        return None;
    }
    Some(
        path.strip_prefix('"')
            .and_then(|quoted| quoted.strip_suffix('"'))
            .unwrap_or(path)
            .to_string(),
    )
}

/// The modules a `go.mod` requires directly, with the line of each. A `replace` by a
/// directory next to the module makes it a package of the project by path. `None` for a
/// `go.mod` that cannot be read.
pub(crate) fn declared(file: &ManifestFile) -> Option<Listed> {
    listed(file.path, file.text).map(Listed::from)
}

/// The modules the `go.mod` at `manifest`, holding `text`, requires directly: a
/// directory a `replace` names is resolved against its own.
fn listed(manifest: &str, text: &str) -> Option<Vec<Declared>> {
    let go_mod = parse(text)?;
    Some(
        go_mod
            .requires
            .iter()
            .filter(|require| !require.indirect)
            .map(|require| {
                let source = go_mod
                    .local
                    .iter()
                    .find(|(module, _)| *module == require.module)
                    .map_or(Source::Package, |(_, dir)| path_source(manifest, dir));
                let line = line_at(text, require.offset);
                Declared::new(line, &require.module, &require.module, source)
            })
            .collect(),
    )
}

/// The module path a `go.mod` declares.
pub(crate) fn name(text: &str) -> Option<String> {
    parse(text)
        .map(|go_mod| go_mod.module)
        .filter(|module| !module.is_empty())
}

/// What the files under a `go.mod` may import: its own module and every module it
/// requires, an `// indirect` one too, since `go build` builds with it. A `go.mod`
/// that cannot be read makes it unreadable.
pub(crate) fn importable(text: &str) -> Declares {
    let Some(go_mod) = parse(text) else {
        return Declares::Unreadable;
    };
    Declares::Package(
        std::iter::once(go_mod.module)
            .filter(|module| !module.is_empty())
            .chain(go_mod.requires.into_iter().map(|require| require.module))
            .collect(),
    )
}

/// Whether one of the modules `names` holds `package`: a Go import names a
/// package path, which its module is a prefix of.
pub(crate) fn covers(names: &BTreeSet<String>, package: &str) -> bool {
    names
        .iter()
        .any(|m| package == m || package.starts_with(&format!("{m}/")))
}

#[cfg(test)]
mod tests {
    use super::super::Source;
    use super::*;

    type Row = (usize, String, String, Source);

    /// The modules a `go.mod` at `app/go.mod` holding `text` requires.
    fn rows(text: &str) -> Vec<Row> {
        listed("app/go.mod", text)
            .unwrap()
            .into_iter()
            .map(|d| (d.line, d.name, d.package, d.source))
            .collect()
    }

    #[test]
    fn a_go_mod_requires_a_module_on_the_line_of_its_path() {
        let text = concat!(
            "module example.com/app\n",                         // 1
            "\n",                                               // 2
            "go 1.22\n",                                        // 3
            "\n",                                               // 4
            "require (\n",                                      // 5
            "\texample.com/lib v1.0.0\n",                       // 6
            "\tgithub.com/lib/pq v1.10.0\n",                    // 7
            ")\n",                                              // 8
            "\n",                                               // 9
            "require github.com/x/y v1.2.3\n",                  // 10
            "\n",                                               // 11
            "replace example.com/lib => ../lib\n",              // 12
            "replace (\n",                                      // 13
            "\tgithub.com/x/y v1.2.3 => ./vendor/y\n",          // 14
            "\texample.com/other => example.com/fork v1.0.0\n", // 15
            ")\n",                                              // 16
        );
        let n = |x: &str| x.to_string();
        assert_eq!(
            rows(text),
            [
                (
                    6,
                    n("example.com/lib"),
                    n("example.com/lib"),
                    Source::Path(n("lib"))
                ),
                (
                    7,
                    n("github.com/lib/pq"),
                    n("github.com/lib/pq"),
                    Source::Package
                ),
                (
                    10,
                    n("github.com/x/y"),
                    n("github.com/x/y"),
                    Source::Path(n("app/vendor/y"))
                ),
            ]
        );
        assert_eq!(name(text), Some(n("example.com/app")));
    }

    #[test]
    fn a_commented_out_requirement_and_a_quoted_path_are_read_as_go_does() {
        let text = "module \"example.com/app\"\n\nrequire (\n\t// example.com/gone v1.0.0\n\t\"example.com/kept\" v1.0.0\n)\n";
        let got: Vec<_> = rows(text).into_iter().map(|r| (r.0, r.1)).collect();
        assert_eq!(got, [(5, "example.com/kept".to_string())]);
        assert_eq!(name(text), Some("example.com/app".to_string()));
    }

    #[test]
    fn a_go_mod_with_no_requirements_declares_nothing() {
        assert_eq!(listed("app/go.mod", "module a\n").map(|v| v.len()), Some(0));
        assert_eq!(name("go 1.22\n"), None);
    }

    #[test]
    fn what_a_module_does_not_import_directly_is_no_requirement() {
        let text = "module a\n\nrequire (\n\texample.com/direct v1.0.0\n\texample.com/transitive v1.0.0 // indirect\n)\n";
        let got: Vec<_> = rows(text).into_iter().map(|r| r.1).collect();
        assert_eq!(got, ["example.com/direct"]);
    }

    #[test]
    fn only_a_relative_directory_makes_a_replace_local() {
        let text = "module a\n\nrequire (\n\texample.com/rel v1.0.0\n\texample.com/abs v1.0.0\n\texample.com/back v1.0.0\n)\n\nreplace example.com/rel => ./rel\nreplace example.com/abs => /opt/abs\nreplace \"example.com/back\" => \"../back\"\n";
        let got: Vec<_> = rows(text).into_iter().map(|r| (r.1, r.3)).collect();
        assert_eq!(
            got,
            [
                (
                    "example.com/rel".to_string(),
                    Source::Path("app/rel".to_string())
                ),
                ("example.com/abs".to_string(), Source::Package),
                (
                    "example.com/back".to_string(),
                    Source::Path("back".to_string())
                ),
            ]
        );
    }

    #[test]
    fn the_directives_of_a_recent_go_mod_are_read() {
        let text = concat!(
            "module example.com/app\n",
            "\n",
            "go 1.25\n",
            "\n",
            "toolchain go1.25.1\n",
            "\n",
            "godebug (\n",
            "\tpanicnil=1\n",
            ")\n",
            "\n",
            "tool golang.org/x/tools/cmd/stringer\n",
            "\n",
            "ignore ./node_modules\n",
            "\n",
            "require example.com/lib v1.0.0\n",
        );
        let got: Vec<_> = rows(text).into_iter().map(|r| (r.0, r.1)).collect();
        assert_eq!(got, [(15, "example.com/lib".to_string())]);
    }

    #[test]
    fn a_go_mod_checked_out_with_crlf_is_read() {
        let text = "module example.com/app\r\n\r\nrequire (\r\n\texample.com/lib v1.0.0\r\n\texample.com/x v1.0.0 // indirect\r\n)\r\n\r\nreplace example.com/lib => ../lib\r\n";
        assert_eq!(
            rows(text),
            [(
                4,
                "example.com/lib".to_string(),
                "example.com/lib".to_string(),
                Source::Path("lib".to_string())
            )]
        );
        assert_eq!(name(text), Some("example.com/app".to_string()));
    }

    #[test]
    fn a_directive_go_does_not_know_is_unread_not_skipped() {
        assert!(listed("app/go.mod", "module a\n\nfrobnicate x\n").is_none());
    }

    /// Go takes a path as an interpreted string (`"a/b"`), not as a raw one; a
    /// go.mod that quotes one in backticks is one Go does not read.
    #[test]
    fn a_path_in_backticks_leaves_the_go_mod_unread() {
        for text in [
            "module `example.com/app`\n",
            "module a\n\nrequire `example.com/x` v1.0.0\n",
            "module a\n\nrequire example.com/x v1.0.0\n\nreplace example.com/x => `../x`\n",
        ] {
            assert!(listed("app/go.mod", text).is_none(), "{text}");
            assert_eq!(name(text), None, "{text}");
            assert!(matches!(importable(text), Declares::Unreadable), "{text}");
        }
    }

    #[test]
    fn a_block_that_never_closes_is_unread_not_empty() {
        assert!(
            listed(
                "app/go.mod",
                "module a\n\nrequire (\n\texample.com/x v1.0.0\n"
            )
            .is_none()
        );
    }
}
