//! The references of an index, attributed to the definition that holds them.
//!
//! `must_not_reach` follows imports between files, so calling one function of a
//! file reaches everything the file imports. With `reach: symbols` a chain follows
//! the definitions a call references instead: a node is a method or function (a
//! symbol with the lines it spans) or what a file holds outside any of them (its
//! class bodies, constants, top-level code), and a dependency of one node on another
//! ([`NodeDep`]) is a reference at a line.
//!
//! A dependency that no reference accounts for stays a dependency on everything in
//! its target, and so does a class or constant used with no method named, so no
//! import is dropped for want of a range. The index is read into a flow in `scip::Indexed::flow`.
//!
//! `impact --diff` reads the flow whatever `reach` says: the uses of a definition the
//! change touched are the places worth re-checking ([`Flow::callers`]).

use std::collections::BTreeMap;

use crate::lang::Scope;

/// A part of a file: one of its methods, or, with no scope, the code that is in none.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Node {
    pub file: String,
    pub scope: Option<String>,
}

impl Node {
    pub fn file(file: &str) -> Self {
        Self {
            file: file.to_string(),
            scope: None,
        }
    }

    /// The node as a graph key; `file_of` takes the file back.
    pub fn key(&self) -> String {
        match &self.scope {
            Some(scope) => format!("{}{SEPARATOR}{scope}", self.file),
            None => self.file.clone(),
        }
    }
}

const SEPARATOR: char = '\u{1}';

/// The file of a node key.
pub fn file_of(key: &str) -> &str {
    key.split_once(SEPARATOR).map_or(key, |(file, _)| file)
}

/// A dependency of one node on another: a reference at `line` of the source node's
/// file to the target node.
#[derive(Debug, Clone)]
pub struct NodeDep {
    pub from: Node,
    pub to: Node,
    pub line: usize,
}

/// A reference the index records at `line` of `file` to a definition: to the method
/// it names, or to the own code of the file that defines the class or constant it
/// names. Unlike a [`NodeDep`], it is never widened to every method of a file, and a
/// module named on its own (an import) is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    pub file: String,
    pub line: usize,
    pub to: Node,
}

#[derive(Debug, Default)]
pub struct Flow {
    /// The methods of each file that the index gave a range.
    pub scopes: BTreeMap<String, Vec<Scope>>,
    pub node_deps: Vec<NodeDep>,
    /// Every reference of the index to a definition, as [`Flow::callers`] reads them.
    pub uses: Vec<Use>,
    /// The name of each method of `scopes` as its symbol spells it, without the
    /// package and the modules around it (`Order.total`).
    pub names: BTreeMap<String, String>,
}

impl Flow {
    /// The innermost scopes that hold `line` of `file`, or the file's own code.
    pub fn owners(&self, file: &str, line: usize) -> Vec<Node> {
        self.holders(file, line, line)
    }

    /// The innermost scopes that hold every line from `first` to `last` of `file`, or
    /// the file's own code when none does.
    pub fn holders(&self, file: &str, first: usize, last: usize) -> Vec<Node> {
        let holding: Vec<&Scope> = self
            .scopes
            .get(file)
            .into_iter()
            .flatten()
            .filter(|scope| scope.first <= first && last <= scope.last)
            .collect();
        let Some(smallest) = holding.iter().map(|s| s.last - s.first).min() else {
            return vec![Node::file(file)];
        };
        // Two scopes of one size on one line cannot be told apart: both hold it.
        holding
            .into_iter()
            .filter(|scope| scope.last - scope.first == smallest)
            .map(|scope| Node {
                file: file.to_string(),
                scope: Some(scope.symbol.clone()),
            })
            .collect()
    }

    /// Everything in `file`: its own code and each of its methods.
    pub fn nodes_of(&self, file: &str) -> Vec<Node> {
        std::iter::once(Node::file(file))
            .chain(
                self.scopes
                    .get(file)
                    .into_iter()
                    .flatten()
                    .map(|scope| Node {
                        file: file.to_string(),
                        scope: Some(scope.symbol.clone()),
                    }),
            )
            .collect()
    }

    /// The scope of `node`, `None` for a file's own code or a scope the flow lacks.
    pub fn scope_of(&self, node: &Node) -> Option<&Scope> {
        let symbol = node.scope.as_deref()?;
        self.scopes
            .get(&node.file)?
            .iter()
            .find(|scope| scope.symbol == symbol)
    }

    /// The uses of `node` from outside it, once per place: from another file, or from a
    /// line of its file that it does not hold (a method's range, the code of its file
    /// that is in no method).
    pub fn callers(&self, node: &Node) -> Vec<&Use> {
        let inside = |found: &Use| {
            found.file == node.file
                && match self.scope_of(node) {
                    Some(scope) => scope.first <= found.line && found.line <= scope.last,
                    None => self.owners(&found.file, found.line) == [Node::file(&found.file)],
                }
        };
        places(
            self.uses
                .iter()
                .filter(|found| found.to == *node && !inside(found)),
        )
    }

    /// The uses of anything `file` defines from the other files, once per place.
    pub fn file_callers(&self, file: &str) -> Vec<&Use> {
        places(
            self.uses
                .iter()
                .filter(|found| found.to.file == file && found.file != file),
        )
    }
}

/// `uses` sorted by place, once per place.
fn places<'a>(uses: impl Iterator<Item = &'a Use>) -> Vec<&'a Use> {
    let mut found: Vec<&Use> = uses.collect();
    found.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    found.dedup_by(|a, b| (&a.file, a.line) == (&b.file, b.line));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(symbol: &str, first: usize, last: usize) -> Scope {
        Scope {
            symbol: symbol.to_string(),
            first,
            last,
        }
    }

    fn method(symbol: &str) -> Node {
        Node {
            file: "a.ts".to_string(),
            scope: Some(symbol.to_string()),
        }
    }

    /// `outer` spans lines 10 to 20 and holds `inner`, 12 to 14.
    fn nested() -> Flow {
        Flow {
            scopes: BTreeMap::from([(
                "a.ts".to_string(),
                vec![scope("outer().", 10, 20), scope("inner().", 12, 14)],
            )]),
            ..Flow::default()
        }
    }

    #[test]
    fn the_innermost_scope_that_holds_every_line_holds_a_span() {
        let flow = nested();
        assert_eq!(flow.holders("a.ts", 12, 12), [method("inner().")]);
        assert_eq!(flow.holders("a.ts", 13, 14), [method("inner().")]);
        // A span that leaves the inner scope belongs to the outer one.
        assert_eq!(flow.holders("a.ts", 11, 12), [method("outer().")]);
        assert_eq!(flow.holders("a.ts", 14, 15), [method("outer().")]);
        assert_eq!(flow.holders("a.ts", 10, 10), [method("outer().")]);
        assert_eq!(flow.holders("a.ts", 20, 20), [method("outer().")]);
        // Past either end, the file's own code.
        assert_eq!(flow.holders("a.ts", 9, 10), [Node::file("a.ts")]);
        assert_eq!(flow.holders("a.ts", 20, 21), [Node::file("a.ts")]);
        assert_eq!(flow.holders("b.ts", 12, 12), [Node::file("b.ts")]);
    }

    #[test]
    fn a_caller_is_a_use_from_outside_what_it_names() {
        let mut flow = nested();
        let used = |file: &str, line: usize, to: Node| Use {
            file: file.to_string(),
            line,
            to,
        };
        flow.uses = vec![
            used("b.ts", 3, method("outer().")),
            used("b.ts", 3, method("outer().")),
            // A use from within its own lines is none, from a method it holds too.
            used("a.ts", 11, method("outer().")),
            used("a.ts", 13, method("outer().")),
            used("a.ts", 2, method("outer().")),
            used("b.ts", 4, method("inner().")),
            // The own code of `a.ts` used from its own code, and from a method.
            used("a.ts", 1, Node::file("a.ts")),
            used("a.ts", 15, Node::file("a.ts")),
        ];
        let places = |node: Node| -> Vec<(String, usize)> {
            flow.callers(&node)
                .into_iter()
                .map(|found| (found.file.clone(), found.line))
                .collect()
        };
        assert_eq!(
            places(method("outer().")),
            [("a.ts".to_string(), 2), ("b.ts".to_string(), 3)]
        );
        assert_eq!(places(method("inner().")), [("b.ts".to_string(), 4)]);
        assert_eq!(places(Node::file("a.ts")), [("a.ts".to_string(), 15)]);
    }

    /// A file whose index gives no ranges is used through anything it defines, from
    /// the other files only.
    #[test]
    fn a_file_is_used_from_the_other_files() {
        let used = |file: &str, line: usize, to: Node| Use {
            file: file.to_string(),
            line,
            to,
        };
        let flow = Flow {
            uses: vec![
                used("b.cs", 4, Node::file("a.cs")),
                used(
                    "b.cs",
                    2,
                    Node {
                        file: "a.cs".to_string(),
                        scope: Some("Run().".to_string()),
                    },
                ),
                used("b.cs", 2, Node::file("a.cs")),
                // From itself, and to another file: neither.
                used("a.cs", 7, Node::file("a.cs")),
                used("c.cs", 1, Node::file("b.cs")),
            ],
            ..Flow::default()
        };
        let places: Vec<(String, usize)> = flow
            .file_callers("a.cs")
            .into_iter()
            .map(|found| (found.file.clone(), found.line))
            .collect();
        assert_eq!(places, [("b.cs".to_string(), 2), ("b.cs".to_string(), 4)]);
    }
}
