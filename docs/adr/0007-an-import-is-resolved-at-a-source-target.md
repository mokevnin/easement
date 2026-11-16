---
status: accepted
---

# An import is resolved where the index takes it to a source target

easement trusted that the index held every dependency a file wrote, and it compared the imports a file writes with the index only for Go, and only to see that some module reference stood on the specifier ([#139](https://github.com/mokevnin/easement/issues/139)). scip-typescript breaks that trust in ways that leave no trace: an import a wildcard declaration catches (`declare module "*.css"` in `vite/client`, `"*.svelte"` in a shim) has a module reference, and became a dependency on `vite` or on the shim, while the file it imports was missing from the graph; an import the compiler cannot resolve, a side-effect import of a file with no exports, `require()` in a `.ts` file and a computed `import()` leave nothing at all. So an import now counts as resolved only when the index takes it to a source target, a file of the project or an external package, and every other written import is either read from its text or an evidence gap. This is one rule for every language that lists its written imports; Go's results do not change.

## Considered options

- **Keep counting any module reference as resolved.** Rejected: a wildcard declaration is a reference that names no file, so a forbidden import of a stylesheet or a component passes clean.
- **Report every import the index takes to no source target as a gap, and read nothing from its text.** Rejected: every Vite project imports its stylesheets and assets through wildcards, and a gap on each would bury the ones that matter.
- **Resolve the text as the bundler does, guessing extensions and `index` files.** Rejected: a guess can name a file the build never reads, and a wrong dependency is worse than a gap that says so.

## Consequences

- A wildcard declaration is read as a declaration only, where the index is read: no dependency, external module or symbol use comes of it.
- When the index gives no source target, a relative path that names a file of the project exactly, once a `?query` is left off, is the dependency on that file, and a bare specifier spelled as a package (`some-pkg/style.css`) is the dependency on the package. Which specifiers are paths and which are packages, the registry entry of the language says. Such a dependency is as exact as the text and is not flagged: it names one file, or one package that `manifests` and `forbid_external` judge like any other. The index's own dependency on the same target wins, so it is kept once.
- Every other import is an `unresolved_imports` gap, one for each cause: no target, a wildcard only, a form the index does not record (a side-effect import, `require()` where the compiler does not know it), a target computed at run time. What brings each about and how to close it is in the registry entry of the language. The imports of `target_only` and excluded files are compared too, since they still reach files for `unused`, but no check judges their dependencies, so an unresolved one is no gap: `unused` alone reads it ([#144](https://github.com/mokevnin/easement/issues/144)).
- A JSON file the compiler reads but does not index was dropped before; its import now names its file and is a dependency.
- Where the text of a module may name one of several files (a Python `.x` names `x.py` or the package `x/__init__.py`), the entry lists them, and the dependency is on the one that exists; when none or more than one does, the import is a gap. Still no guess: each path is one the language itself would read.
- An import may be recorded on other text than its module: the index records Python's `from . import y as z` on `y as z`, and nothing on the dots. The import then carries that text, and a name there that a file of the project defines (`from . import VERSION`) resolves it as its module would. A module that is the importing file itself is no target: scip-python takes a `from .. import x` above the top package there.
- Languages that do not list their written imports (Ruby, PHP, Java, C#, Rust) are not compared yet; each follows in its own change.
