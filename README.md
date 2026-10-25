# easement

Architecture quality gate for polyglot codebases.

Declare the components of your system and which of them may depend on which. `easement` extracts the real dependencies from the code, builds the graph and fails the build when the code drifts from the declared architecture.

> **easement** — a legal right to cross someone else's land. In `easement`, every allowed dependency between components is an explicit easement; everything else is trespassing.

## Status

Early. Go, TypeScript/JavaScript, Python and Java work.

## Usage

```sh
easement check                 # exit 0: passed, 1: violations, 2: error
easement check --format json   # machine-readable report
easement baseline              # legalize current violations
```

`easement.yml` lives in the project root; all paths are relative to it.

```yaml
version: 1

components:
  handler:    { in: internal/handlers }
  service:    { in: "internal/services/**" }
  repository: { in: "internal/domain/*/repository" }
  model:      { in: internal/models }

common: [model]

rules:
  handler: { may_depend_on: [service] }
  service: { may_depend_on: [repository] }

exclude: ["**/*_test.go"]
```

- **components**: an `in` pattern (a glob or a list of globs) names a module, whether it is a directory or a file: `src/app/cli` covers the `cli/` directory with everything below it and a file `cli.py`. `*` stays within one directory, `**` crosses directories. When several components match a file, the longest pattern wins, so a single file can be carved out of a larger component.
- **rules**: an allow-list. A component may depend on itself, on `common` components and on what `may_depend_on` lists — nothing else.
- Files outside every component are not checked; external packages are not checked yet.
- **exclude**: globs to skip. `.gitignore` is honoured and `node_modules` is always skipped.
- **python.roots**: `sys.path` entries inside the project, default `[".", "src"]`.

```
$ easement check
internal/handlers/books.go:6: handler -> repository is not allowed (imports example.com/shop/internal/repository)

easement failed: 1 violation(s), 0 stale, 0 legalized; 5 files, 6 internal deps
```

## Baseline: adopting on an existing project

`easement baseline` writes current violations to `easement.baseline.yml` (path configurable with `baseline:`). After that the gate is a ratchet:

- violations in the baseline are legalized;
- any new violation fails;
- a baseline entry that no longer occurs also fails, so fixed debt has to be removed from the file with `easement baseline`. The baseline can only shrink.

Entries are keyed by file, target and components, not by line, so unrelated edits do not invalidate them.

## Languages

| Language | Extracts | Resolves |
|---|---|---|
| Go | `import` | module paths from every `go.mod` in the repo |
| TypeScript / JavaScript | `import`, `export … from`, `import x = require()`, `require()`, `import()` | Node/bundler rules via [oxc_resolver]: relative paths, `tsconfig` `paths`, package `exports`, workspaces, `.js` → `.ts` |
| Python | `import`, `from … import`, relative imports | longest existing module or package under the source roots; `from a import b` resolves to `a/b.py` if it exists, otherwise to `a` |
| Java | `import`, `import static`, `import ….*` | classes indexed by their `package` declaration, so any source layout and multi-module builds work without configuration |

Known limits: Java references by fully qualified name without an import are not seen; Python imports built from strings (`importlib`) are not seen.

Adding a language means implementing one trait (`src/lang/mod.rs`): extract imports from a file, resolve a specifier to a project path or an external module. Rules, baseline and reports are language-agnostic.

Languages whose dependencies are not visible from imports (Ruby, Elixir) are out of scope.

## How it works

1. Walk the project, parse each supported file with tree-sitter and collect imports.
2. Resolve every import to a file or package directory inside the project, or to an external module.
3. Map both ends to components and check each edge against the rules.
4. Compare violations with the baseline and report.

[oxc_resolver]: https://github.com/oxc-project/oxc-resolver

## License

MIT
