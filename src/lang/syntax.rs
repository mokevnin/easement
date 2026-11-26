//! What the tree-sitter adapters and hooks share: walking a syntax tree.

#[cfg(test)]
use std::path::{Path, PathBuf};

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

/// `root` and every named node below it, each before its children, in the
/// order of the text.
pub(crate) fn preorder(root: Node) -> impl Iterator<Item = Node> {
    let mut pending = vec![root];
    std::iter::from_fn(move || {
        let node = pending.pop()?;
        let mut cursor = node.walk();
        let children: Vec<Node> = node.named_children(&mut cursor).collect();
        pending.extend(children.into_iter().rev());
        Some(node)
    })
}

/// Whether `node` has a named child of `kind`.
pub(crate) fn has_child(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| child.kind() == kind)
}

/// The first argument of `call`, in its `arguments`: the first named node there
/// that is no extra, such as a comment written before it (`import(/* c */ "x")`).
/// `None` when the call has none.
pub(crate) fn first_argument(call: Node) -> Option<Node> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    arguments
        .named_children(&mut cursor)
        .find(|child| !child.is_extra())
}

/// The syntax tree of `source` in `language`. Any text has a tree: a syntax
/// error is a node of it.
pub(crate) fn tree(language: &Language, source: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .expect("a grammar matches the tree-sitter it is built with");
    parser
        .parse(source, None)
        .expect("a parse with no timeout and no cancellation ends with a tree")
}

/// Every node captured as `@node` by `query`, in document order.
pub(crate) fn captured_nodes<'t>(query: &Query, tree: &'t Tree, source: &str) -> Vec<Node<'t>> {
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut nodes = Vec::new();
    while let Some(m) = matches.next() {
        nodes.extend(m.captures().iter().map(|c| c.node));
    }
    nodes
}

/// What the walk of `root` sees but the directories, relative to it, in the order
/// of the walk: what the scan gives the adapters.
#[cfg(test)]
pub(crate) fn walked(root: &Path) -> Vec<PathBuf> {
    crate::relpath::walker(root)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| !t.is_dir()))
        .filter_map(|e| crate::relpath::relative(root, e.path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of the first argument of the first call (`kind`) in `source`.
    fn first_of(language: &Language, kind: &str, source: &str) -> Option<String> {
        let tree = tree(language, source);
        let call = preorder(tree.root_node()).find(|node| node.kind() == kind)?;
        first_argument(call).map(|argument| {
            argument
                .utf8_text(source.as_bytes())
                .expect("a node is text of its source")
                .to_string()
        })
    }

    /// Comments before the first argument, of any form and however many, are no
    /// argument; one after it is no part of it; a call of comments alone, or of
    /// nothing, has none.
    #[test]
    fn the_first_argument_of_a_call_is_its_first_that_is_no_comment() {
        let typescript: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        for (source, first) in [
            ("f(x);", Some("x")),
            ("f(/* a */ x);", Some("x")),
            ("f(/* a */ /* b */ x, y);", Some("x")),
            ("f(// a\n  x);", Some("x")),
            ("f(x /* a */, y);", Some("x")),
            ("f(/* a */);", None),
            ("f();", None),
        ] {
            assert_eq!(
                first_of(&typescript, "call_expression", source).as_deref(),
                first,
                "{source}"
            );
        }
        let python: Language = tree_sitter_python::LANGUAGE.into();
        for (source, first) in [
            ("f(x)\n", Some("x")),
            ("f(  # a\n  # b\n  x, y)\n", Some("x")),
            ("f(  # a\n)\n", None),
            ("f()\n", None),
        ] {
            assert_eq!(
                first_of(&python, "call", source).as_deref(),
                first,
                "{source}"
            );
        }
    }
}
