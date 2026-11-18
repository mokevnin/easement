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
