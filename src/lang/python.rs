//! Python beyond its index: the imports a file writes, and the file the module
//! of a relative import names.

use tree_sitter::Node;

use super::registry::{Explained, Spelled};
use super::{Cause, Form, Import};

/// What brings about each cause of an unresolved Python import, and how to close it.
pub const EXPLAINED: &[Explained] = &[
    Explained {
        cause: Cause::NoTarget,
        because: "for example, a module that does not exist, or a package not installed where scip-python runs",
        close: Some(
            "fix the module, or install the project's packages in the environment scip-python runs in (activate its virtualenv), then check again",
        ),
    },
    Explained {
        cause: Cause::Computed,
        because: "`importlib.import_module()` or `__import__()` of a name that is not a literal names no one module",
        close: Some("write each module as an `import`, or review those dependencies by hand"),
    },
    Explained {
        cause: Cause::ImportCall,
        because: "scip-python records no module that `importlib.import_module()` or `__import__()` names",
        close: Some("write each module as an `import`, or review those dependencies by hand"),
    },
];

/// The imports the text of a Python file writes, at the line of each module, as
/// spelled and resolved by nothing: the module of every `import` (`import a.b`,
/// `import a.b as c`) and of every `from … import` (`from a.b import c`,
/// `from .x import y`, `from . import *`), wherever it stands; for each name of
/// `from . import y` or `from .. import y as z`, whose module the index records on
/// no text, the module `.y` on the text of the name. And the module that
/// `importlib.import_module()`, `import_module()` or `__import__()` is given:
/// [`Form::ImportCall`] when it is a literal, else [`Form::Computed`].
///
/// `from __future__ import …` is a directive of the compiler, no module.
pub fn written_imports(_file: &str, text: &str) -> Vec<Import> {
    let tree = super::tree(&tree_sitter_python::LANGUAGE.into(), text);
    super::preorder(tree.root_node())
        .flat_map(|node| imports_at(node, text))
        .collect()
}

/// The imports `node` writes: none unless it is an import.
fn imports_at(node: Node, text: &str) -> Vec<Import> {
    let source = |node: Node| node.utf8_text(text.as_bytes()).unwrap_or_default();
    match node.kind() {
        "import_statement" => children_by_field(node, "name")
            .into_iter()
            .filter_map(unaliased)
            .map(|module| module_at(module, source(module)))
            .collect(),
        "import_from_statement" => {
            let Some(module) = node.child_by_field_name("module_name") else {
                return Vec::new();
            };
            let names = children_by_field(node, "name");
            let dots_alone = module.kind() == "relative_import"
                && !super::has_child(module, "dotted_name")
                && !names.is_empty();
            if !dots_alone {
                return vec![module_at(module, source(module))];
            }
            // `from . import y`: the index records `y`, and nothing on the dots.
            let dots = squeezed(source(module));
            names
                .into_iter()
                .filter_map(|name| {
                    let bare = unaliased(name)?;
                    let module = format!("{dots}{}", squeezed(source(bare)));
                    Some(Import {
                        recorded_on: Some(source(name).to_string()),
                        ..Import::at(name, module, Form::Static)
                    })
                })
                .collect()
        }
        "call" => {
            // `importlib.import_module()`, `import_module()` and `__import__()`, by
            // whatever name their module is reached: one of another module that
            // shares the name is reported too, never missed.
            let imports_one = node
                .child_by_field_name("function")
                .and_then(|function| match function.kind() {
                    "identifier" => Some(function),
                    "attribute" => function.child_by_field_name("attribute"),
                    _ => None,
                })
                .is_some_and(|called| matches!(source(called), "import_module" | "__import__"));
            let argument = node
                .child_by_field_name("arguments")
                .filter(|_| imports_one)
                .and_then(|arguments| {
                    let mut cursor = arguments.walk();
                    arguments.named_children(&mut cursor).next()
                });
            let Some(argument) = argument else {
                return Vec::new();
            };
            vec![match literal(argument, text) {
                Some(module) => Import::at(argument, module, Form::ImportCall { as_import: false }),
                None => Import::at(argument, source(argument), Form::Computed),
            }]
        }
        _ => Vec::new(),
    }
}

/// The module or name an import names, without the alias it may be bound to
/// (`a.b` of `import a.b as c`).
fn unaliased(name: Node) -> Option<Node> {
    match name.kind() {
        "aliased_import" => name.child_by_field_name("name"),
        _ => Some(name),
    }
}

/// The children of `node` in `field`.
fn children_by_field<'t>(node: Node<'t>, field: &str) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.children_by_field_name(field, &mut cursor).collect()
}

/// `text` without the white space a dotted name may hold (`a . b`).
fn squeezed(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// What a string holds, when nothing is interpolated in it and it is no
/// concatenation of several.
fn literal(node: Node, text: &str) -> Option<String> {
    if node.kind() != "string" || super::has_child(node, "interpolation") {
        return None;
    }
    let mut cursor = node.walk();
    let content = node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "string_content")
        .map(|child| child.utf8_text(text.as_bytes()).unwrap_or_default())
        .collect();
    Some(content)
}

/// The import of the module `node` names, whose text is `module`: a dotted name
/// read without the white space it may hold, on the text as written.
fn module_at(node: Node, module: &str) -> Import {
    let specifier = squeezed(module);
    Import {
        recorded_on: (specifier != module).then(|| module.to_string()),
        ..Import::at(node, specifier, Form::Static)
    }
}

/// What the module of an import spells when the index takes it to no source
/// target: a relative module (`.x`, `..x.y`) names the file of the module or the
/// `__init__.py` of the package, and the dots alone (`.`) that of the package; an
/// absolute one names neither, for which module of the path it is is the
/// environment's to say.
pub fn spelled(specifier: &str) -> Spelled {
    let rest = specifier.trim_start_matches('.');
    let dots = specifier.len() - rest.len();
    if dots == 0 {
        return Spelled::Neither;
    }
    let up = match dots {
        1 => "./".to_string(),
        _ => "../".repeat(dots - 1),
    };
    if rest.is_empty() {
        return Spelled::Paths(vec![format!("{up}__init__.py")]);
    }
    let path = format!("{up}{}", rest.replace('.', "/"));
    Spelled::Paths(vec![format!("{path}.py"), format!("{path}/__init__.py")])
}

/// pyright asks the `python3` on the path for its search paths, and scip-python
/// runs `pip` for the installed packages: `PYTHONPATH` changes what an import
/// resolves to. `PYTHONHASHSEED` is left out: tox sets a new one every run. So
/// is `VIRTUAL_ENV`, which alone does not change the index and names the
/// checkout when the environment lives in it, as uv and Poetry put it.
pub fn env(name: &str) -> bool {
    (name.starts_with("PYTHON") && name != "PYTHONHASHSEED") || name.starts_with("PIP_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Form;

    #[test]
    fn the_variables_of_python_and_pip_are_keyed_but_the_hash_seed() {
        for name in ["PYTHONPATH", "PIP_USER"] {
            assert!(env(name), "{name}");
        }
        for name in ["VIRTUAL_ENV", "PYTHONHASHSEED"] {
            assert!(!env(name), "{name}");
        }
    }

    /// `(line, specifier, form, recorded_on)` of each import `text` writes.
    fn written(text: &str) -> Vec<(usize, String, Form, Option<String>)> {
        written_imports("pkg/app.py", text)
            .into_iter()
            .map(|import| {
                (
                    import.line,
                    import.specifier,
                    import.form,
                    import.recorded_on,
                )
            })
            .collect()
    }

    fn expected(
        expected: &[(usize, &str, Form, Option<&str>)],
    ) -> Vec<(usize, String, Form, Option<String>)> {
        expected
            .iter()
            .map(|(line, specifier, form, on)| {
                (*line, specifier.to_string(), *form, on.map(str::to_string))
            })
            .collect()
    }

    #[test]
    fn written_imports_are_every_form_at_the_line_of_its_module() {
        let text = r#""""import not.this"""
# from not import this
from __future__ import annotations
import a.b
import c.d as e, f
from g.h import i, j as k
from .l import m
from ..n.o import (
    p,
)
from . import q, r as s
from .. import (
    t,
    u as v,
)
from . import *
from w import *
def f():
    import x
s = "import not.this"
"#;
        let s = Form::Static;
        assert_eq!(
            written(text),
            expected(&[
                (4, "a.b", s, None),
                (5, "c.d", s, None),
                (5, "f", s, None),
                (6, "g.h", s, None),
                (7, ".l", s, None),
                (8, "..n.o", s, None),
                (11, ".q", s, Some("q")),
                (11, ".r", s, Some("r as s")),
                (13, "..t", s, Some("t")),
                (14, "..u", s, Some("u as v")),
                (16, ".", s, None),
                (17, "w", s, None),
                (19, "x", s, None),
            ])
        );
    }

    #[test]
    fn an_import_call_names_its_literal_module_or_is_computed() {
        let text = r#"import importlib
a = importlib.import_module("pkg.a")
b = __import__('pkg.b')
c = import_module("pkg.c")
d = importlib.import_module(name)
e = importlib.import_module(f"pkg.{name}")
f = importlib.import_module("pkg." + name)
g = il.import_module("pkg.g")
h = builtins.__import__("pkg.h")
i = other.load("not.this")
"#;
        assert_eq!(
            written(text),
            expected(&[
                (1, "importlib", Form::Static, None),
                (2, "pkg.a", Form::ImportCall { as_import: false }, None),
                (3, "pkg.b", Form::ImportCall { as_import: false }, None),
                (4, "pkg.c", Form::ImportCall { as_import: false }, None),
                (5, "name", Form::Computed, None),
                (6, "f\"pkg.{name}\"", Form::Computed, None),
                (7, "\"pkg.\" + name", Form::Computed, None),
                (8, "pkg.g", Form::ImportCall { as_import: false }, None),
                (9, "pkg.h", Form::ImportCall { as_import: false }, None),
            ])
        );
    }

    #[test]
    fn a_relative_module_spells_its_file_or_its_package() {
        let one_of =
            |paths: &[&str]| Spelled::Paths(paths.iter().map(|path| path.to_string()).collect());
        assert_eq!(spelled(".x"), one_of(&["./x.py", "./x/__init__.py"]));
        assert_eq!(
            spelled("..x.y"),
            one_of(&["../x/y.py", "../x/y/__init__.py"])
        );
        assert_eq!(spelled("."), one_of(&["./__init__.py"]));
        assert_eq!(spelled("..."), one_of(&["../../__init__.py"]));
        for absolute in ["x", "x.y", "importlib"] {
            assert_eq!(spelled(absolute), Spelled::Neither, "{absolute}");
        }
    }
}
