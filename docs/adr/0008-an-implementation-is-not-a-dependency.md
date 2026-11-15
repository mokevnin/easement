---
status: accepted
---

# An implementation is a fact apart from dependencies

Which components may implement or extend the types of a component ([#137](https://github.com/mokevnin/easement/issues/137)) cannot be read from dependencies. No indexer marks the written supertype: a SCIP `Relationship` has no range, and finding the supertype clause by position misreads `class Both implements Bar, Repository<Bar>`, field types and type-parameter bounds (`docs/research/declaration-relationships.md`). What the indexes do give is a relation between two definitions, often transitive (Java, C#, TypeScript classes, Dart) and in Go structural. Neither is a written name. So an implementation is its own fact, read from `is_implementation` relationships between types, and it never enters the dependency graph: not `may_depend_on`, not reach, not cycles, not dependency counts. ADR 0003 stands unchanged.

## Considered options

- **"Implements" as a qualifier on an existing dependency.** Rejected: no indexer ties the written supertype to its relationship, and the position heuristic has known misreadings in every language where it works at all, and no signal in C#, PHP, Rust, Kotlin or Swift.
- **Implementations as dependencies.** Rejected: an inherited ancestor or a Go structural match is no name the code writes. It would give one policy a different meaning in each language, which ADR 0003 removed.

## Consequences

- Implementations are normalized while the index is read, like symbol schemes: every language's relations are closed into the transitive set before any rule reads them (scip-python gives direct ones only). A rule never compensates per language.
- Only declared relations count. Go's structural satisfaction (including accidental matches against any loaded interface, such as `io.Closer`) is not one, so Go types are an evidence gap for rules on implementations, as are languages whose indexers emit none (rust-analyzer, scip-php, scip-ruby, the Kotlin and Swift adapters) and TypeScript interfaces, which scip-typescript skips. Each gap names its cause; the fix belongs in the indexer, not in a guess.
- Only relations between types count. A method's relation to the method it overrides follows from its type's and is not read on its own.
