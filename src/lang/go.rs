//! Go beyond its index: which files are programs of their own, the imports a
//! file writes, and the build tags the indexer builds with.

use tree_sitter::{Node, Tree};

use super::Import;
use super::language::{Inherited, Language};
use crate::relpath;
use crate::scip::symbol::{self, Parsed, Qualifier};

/// The syntax tree of the head of Go source: of its text up to `head_end`, all
/// that `head` reads. A file is parsed for its head alone, so its size, generated
/// code above all, costs nothing past its imports. Any text has a tree: a syntax
/// error is a node of it.
fn head_tree(text: &str) -> Tree {
    super::syntax::tree(&tree_sitter_go::LANGUAGE.into(), &text[..head_end(text)])
}

/// Where the head of Go source ends: at the first `func`, `type`, `var` or
/// `const` outside any comment, string, rune, parentheses or unfinished import,
/// the start of the first declaration past the imports.
///
/// The end may only fall late, never before an import: `head` stops at that
/// declaration anyway, so a late end costs time and no import. Text this cannot
/// read with certainty therefore ends at the end of the text, as does text with
/// no such word: an unclosed comment, string or rune, a `)` that closes nothing,
/// or one of the words between `import` and its path, where only broken code
/// puts it.
fn head_end(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    // After an `import` outside parentheses, until its path.
    let mut in_import = false;
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        let advance = match bytes[i] {
            b'/' if rest.starts_with("//") => rest.find('\n'),
            b'/' if rest.starts_with("/*") => rest[2..].find("*/").map(|n| n + 4),
            // A string is an import's path, or after the head.
            b'`' => {
                in_import = false;
                rest[1..].find('`').map(|n| n + 2)
            }
            b'"' => {
                in_import = false;
                quoted_end(rest)
            }
            b'\'' => quoted_end(rest),
            b'(' => {
                in_import = false;
                depth += 1;
                Some(1)
            }
            b')' => depth.checked_sub(1).map(|outer| {
                depth = outer;
                1
            }),
            b if is_word(b) => {
                let len = rest.bytes().position(|b| !is_word(b)).unwrap_or(rest.len());
                match &rest[..len] {
                    "func" | "type" | "var" | "const" if depth == 0 => {
                        return if in_import { text.len() } else { i };
                    }
                    "import" if depth == 0 => in_import = true,
                    _ => {}
                }
                Some(len)
            }
            _ => Some(1),
        };
        match advance {
            Some(n) => i += n,
            None => return text.len(),
        }
    }
    text.len()
}

/// Whether `b` is a byte of a Go word: a keyword, a name or a number. Every
/// byte past ASCII counts, so a word never ends inside a character.
fn is_word(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric() || !b.is_ascii()
}

/// The length of the interpreted string or rune `rest` starts with, up to its
/// closing quote, or `None` when a line or the text ends first.
fn quoted_end(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let quote = bytes[0];
    let mut j = 1;
    loop {
        match bytes.get(j) {
            None | Some(b'\n') => return None,
            Some(b'\\') if bytes.get(j + 1).is_some_and(|&b| b != b'\n') => j += 2,
            Some(b'\\') => return None,
            Some(&b) if b == quote => return Some(j + 1),
            Some(_) => j += 1,
        }
    }
}

/// The declarations at the head of a file, in order: its package clause, then its
/// imports, up to the first other declaration. Comments are skipped.
fn head<'t>(tree: &'t Tree) -> impl Iterator<Item = Node<'t>> {
    let root = tree.root_node();
    (0..root.named_child_count())
        .filter_map(move |i| root.named_child(i as u32))
        .filter(|node| node.kind() != "comment")
        .take_while(|node| matches!(node.kind(), "package_clause" | "import_declaration"))
}

/// The imports the text of a Go file writes, at the line of each path, as
/// spelled and resolved by nothing: those of its `import` declarations, single
/// and grouped, with any name, `.` or `_` before the path. cgo's `import "C"`
/// is no package, so it is left out.
pub fn written_imports(_file: &str, text: &str) -> Vec<Import> {
    let tree = head_tree(text);
    let mut imports = Vec::new();
    for declaration in head(&tree).filter(|node| node.kind() == "import_declaration") {
        let mut specs = Vec::new();
        collect_specs(declaration, &mut specs);
        imports.extend(specs.into_iter().filter_map(|spec| {
            let path = spec.child_by_field_name("path")?;
            let quoted = path.utf8_text(text.as_bytes()).ok()?;
            let specifier = quoted.get(1..quoted.len().checked_sub(1)?)?;
            (specifier != "C").then(|| Import::at(path, specifier, super::Form::Static))
        }));
    }
    imports
}

/// The `import_spec` nodes below `node`, single or in an `import_spec_list`.
fn collect_specs<'t>(node: Node<'t>, specs: &mut Vec<Node<'t>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_spec" => specs.push(child),
            "import_spec_list" => collect_specs(child, specs),
            _ => {}
        }
    }
}

/// Whether the text of a Go file puts it in `package main`: a program of its own.
pub fn program(text: &str) -> bool {
    let tree = head_tree(text);
    let Some(clause) = head(&tree).find(|node| node.kind() == "package_clause") else {
        return false;
    };
    let mut cursor = clause.walk();
    clause
        .named_children(&mut cursor)
        .find(|node| node.kind() == "package_identifier")
        .is_some_and(|name| name.utf8_text(text.as_bytes()) == Ok("main"))
}

/// Whether `tag` is a build tag `go build -tags` takes: letters, digits, `_` and `.`.
pub fn build_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
}

/// The environment that has the indexer build with `tags`: `-tags` added to the
/// `GOFLAGS` the indexer would inherit.
pub fn tags_env(tags: &[String], inherited: Inherited) -> (&'static str, String) {
    let mut flags = inherited("GOFLAGS").unwrap_or_default();
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str(&format!("-tags={}", tags.join(",")));
    ("GOFLAGS", flags)
}

/// Go's own variables (`GOFLAGS`, `GOOS`, `GO111MODULE`) have no `_` after `GO`,
/// unlike `GOOGLE_APPLICATION_CREDENTIALS`, which a CI job may point at another
/// file every run; cgo's are `CGO_*`.
pub fn env(name: &str) -> bool {
    name.strip_prefix("GO").is_some_and(|rest| {
        rest.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }) || name.starts_with("CGO_")
}

/// scip-go reads the Go sources and what `go` resolves them with: the module and workspace
/// files and a vendored dependency list. A vendored dependency is no code of the project, but
/// scip-go type-checks against it, and one a `replace` takes from outside the project changes
/// `vendor/` alone: its files are in.
pub fn inputs(language: &Language, path: &str) -> bool {
    let name = relpath::file_name(path);
    language.extensions.contains(&relpath::extension(name))
        || matches!(
            name,
            "go.mod" | "go.sum" | "go.work" | "go.work.sum" | "modules.txt"
        )
}

/// A Go symbol's names are called by its package path (`example.com/app/db.Client`).
pub fn qualifier(module: &mut symbol::Module<'_>) -> Qualifier {
    Qualifier::Spelled(module.parsed.namespace("/"))
}

/// The external package of a Go symbol: its path (`fmt`, `net/http`,
/// `github.com/lib/pq`).
pub fn external(parsed: &Parsed<'_>) -> String {
    parsed.namespace("/")
}

/// Whether a module is Go's standard library: a package path without a domain
/// (`fmt`, `net/http`); `C` is cgo.
pub fn std(module: &str) -> bool {
    module != "C" && !super::registry::GO.first_segment(module).contains('.')
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_standard_library_is_a_package_path_without_a_domain() {
        use crate::lang::registry::is_std;
        assert!(is_std("a.go", "net/http"));
        assert!(is_std("a.go", "fmt"));
        assert!(!is_std("a.go", "github.com/lib/pq"));
        assert!(!is_std("a.go", "C"));
    }

    use super::*;
    use crate::lang::registry::GO;

    #[test]
    fn the_cache_key_reads_only_what_scip_go_does() {
        for path in [
            "a.go",
            "svc/x_test.go",
            "go.mod",
            "go.sum",
            "go.work",
            "vendor/modules.txt",
        ] {
            assert!(inputs(&GO, path), "{path}");
        }
        for path in [
            "web/a.tsx",
            "README.md",
            "locales/en.json",
            "pnpm-lock.yaml",
            "x.goal",
        ] {
            assert!(!inputs(&GO, path), "{path}");
        }
    }

    #[test]
    fn the_variables_of_go_and_cgo_are_keyed() {
        for name in ["GOFLAGS", "GOOS", "CGO_ENABLED"] {
            assert!(env(name), "{name}");
        }
        for name in ["GOOGLE_APPLICATION_CREDENTIALS", "GOROOT_1_22_X64"] {
            assert!(!env(name), "{name}");
        }
    }

    #[test]
    fn a_build_tag_is_letters_digits_underscores_and_dots() {
        for tag in ["race", "e2e_1", "go1.22"] {
            assert!(super::build_tag(tag), "{tag}");
        }
        for tag in ["a b", "a,b", "", "-x"] {
            assert!(!super::build_tag(tag), "{tag}");
        }
    }

    /// `(line, module)` of each import `text` writes.
    fn written(text: &str) -> Vec<(usize, String)> {
        super::written_imports("a.go", text)
            .into_iter()
            .map(|import| (import.line, import.specifier))
            .collect()
    }

    #[test]
    fn written_imports_are_every_form_at_the_line_of_its_path() {
        let text = "/* License\n   import \"not/this\"\n*/\n//go:build linux\n\n// Package app.\npackage app\n\nimport \"fmt\"\nimport lb \"example.com/m/lib\"\n\nimport (\n\t// a comment, \"not/this\"\n\t. \"example.com/m/dot\" /* dot */\n\t_ \"strings\"\n\n\traw `example.com/m/raw`\n\t\"example.com/ext\" // trailing\n)\n\nimport (\"os\"; \"io\")\n\nfunc A() { _ = \"import \\\"x\\\"\" }\n";
        assert_eq!(
            written(text),
            [
                (9, "fmt"),
                (10, "example.com/m/lib"),
                (14, "example.com/m/dot"),
                (15, "strings"),
                (17, "example.com/m/raw"),
                (18, "example.com/ext"),
                (21, "os"),
                (21, "io"),
            ]
            .map(|(line, module)| (line, module.to_string()))
        );
    }

    #[test]
    fn a_byte_order_mark_hides_no_import() {
        let text = "\u{feff}package a\n\nimport _ \"example.com/repro/b\"\n";
        assert_eq!(written(text), [(3, "example.com/repro/b".to_string())]);
    }

    #[test]
    fn cgo_is_no_written_import() {
        let text =
            "package c\n\n// #include <stdio.h>\nimport \"C\"\n\nimport (\n\t\"C\"\n\t\"os\"\n)\n";
        assert_eq!(written(text), [(8, "os".to_string())]);
    }

    #[test]
    fn imports_end_at_the_first_other_declaration() {
        let text = "package a\n\nimport \"fmt\"\n\nvar s = `\nimport \"no\"\n`\n\nfunc f() {}\n";
        assert_eq!(written(text), [(3, "fmt".to_string())]);
        assert!(written("package a\n\nfunc f() {}\n").is_empty());
    }

    #[test]
    fn a_keyword_in_a_comment_or_a_path_ends_no_head() {
        let text = "/* func type */\npackage a\n\n// var const\nimport \"fmt\"\n/* func f() {} */\nimport (\n\t\"example.com/func\"\n\traw `example.com/var`\n\tfunctor \"example.com/type\" // const\n)\n\nfunc f() {}\n";
        assert_eq!(
            written(text),
            [
                (5, "fmt"),
                (8, "example.com/func"),
                (9, "example.com/var"),
                (10, "example.com/type"),
            ]
            .map(|(line, module)| (line, module.to_string()))
        );
    }

    #[test]
    fn a_head_ends_mid_line() {
        let text = "package main; import \"fmt\"; func main() { fmt.Println() }\n";
        assert_eq!(written(text), [(1, "fmt".to_string())]);
        assert!(super::program(text));
    }

    #[test]
    fn a_head_may_have_no_imports() {
        assert!(written("package a\n").is_empty());
        assert!(super::program(
            "//go:build tool\n\n// Tool does.\npackage main\n"
        ));
        let text = "//go:build linux\n\n// Package main runs.\npackage main\n\ntype t struct{}\n";
        assert!(written(text).is_empty());
        assert!(super::program(text));
    }

    #[test]
    fn an_unterminated_comment_or_string_reads_the_whole_text() {
        let comment = "package a\n\nimport \"fmt\"\n/* never closed\nfunc f() {}\n";
        assert_eq!(written(comment), [(3, "fmt".to_string())]);
        // tree-sitter reads past the line break that leaves the string open.
        let string = "package a\n\nimport (\n\t\"fmt\n\t\"os\"\n)\n\nfunc f() {}\n";
        assert_eq!(written(string), [(4, "fmt\n\t\"os".to_string())]);
    }

    #[test]
    fn a_syntax_error_in_the_imports_reads_as_a_whole_file_does() {
        let text =
            "package a\n\nimport (\n\t\"fmt\"\n\tfunc \"os\"\n)\n\nimport \"io\"\n\nfunc f() {}\n";
        assert_eq!(
            written(text),
            [(4, "fmt"), (5, "os"), (8, "io")].map(|(line, module)| (line, module.to_string()))
        );
    }

    #[test]
    fn a_syntax_error_past_the_imports_hides_none_of_them() {
        // Go's own `syntax/testdata/sample.go`: the compiler reads the import, then fails.
        let text = "package p\n\nimport \"math\" /* ERROR unexpected comma */ ,\n\ntype x = 0\n";
        assert_eq!(written(text), [(3, "math".to_string())]);
    }

    #[test]
    fn a_word_inside_an_import_hides_none_of_the_imports() {
        for text in [
            "package a\nimport func \"os\"\nimport \"io\"\n",
            "package a\nimport x.func \"os\"\nimport \"io\"\n",
        ] {
            assert_eq!(
                written(text),
                [(2, "os"), (3, "io")].map(|(line, module)| (line, module.to_string())),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_head_ends_at_the_first_declaration_or_at_the_end_of_the_text() {
        let mid_line = "package main; import \"fmt\"; func main() {}\n";
        assert_eq!(super::head_end(mid_line), 28);
        let hidden = "package a\n/* func */ import f `x/func`\n\ntype t int\n";
        assert_eq!(super::head_end(hidden), 40);
        for unsure in [
            "package a\n/* never closed\nfunc f() {}\n",
            "package a\nimport \"fmt\nfunc f() {}\n",
            "package a\n)\nfunc f() {}\n",
            "package a\nimport func \"os\"\n",
            "package a\nimport (\n\t\"os\"\nfunc f() {}\n",
        ] {
            assert_eq!(super::head_end(unsure), unsure.len(), "{unsure:?}");
        }
    }

    #[test]
    fn a_program_is_in_package_main() {
        assert!(super::program("// tool\npackage main\n\nfunc main() {}\n"));
        assert!(!super::program("package mainly\n"));
        assert!(!super::program("package app\n"));
        // The package clause is a token, not a line: an import comment may follow
        // it, and a comment before it is no clause.
        assert!(super::program(
            "package main // import \"example.com/tool\"\n"
        ));
        assert!(super::program("package/* a */main\n"));
        assert!(!super::program("/*\npackage main\n*/\npackage app\n"));
        // A byte order mark before the clause is no token, as for the compiler.
        assert!(super::program("\u{feff}package main\n"));
    }
}
