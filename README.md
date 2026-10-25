# easement

Architecture quality gate for polyglot codebases.

Declare the components of your system and which of them may depend on which. `easement` extracts the real dependencies from the code, builds the graph and fails the build when the code drifts from the declared architecture.

> **easement** — a legal right to cross someone else's land. In `easement`, every allowed dependency between components is an explicit easement; everything else is trespassing.

## Status

Early design. Nothing works yet.

## Idea

- **Language adapters** extract facts from source files (imports first; later calls, inheritance, annotations) using tree-sitter / ast-grep, and resolve them to files inside the repo or to external packages.
- **A language-agnostic core** maps files to components, builds the dependency graph and evaluates rules.
- **Gate semantics are first-class**: a baseline of legalized violations that can only shrink (ratchet), a diff mode that judges only what a pull request changes, stable JSON output and exit codes.

Planned languages: Go, TypeScript/JavaScript, Python, Rust, Java/Kotlin. Languages whose dependencies are not visible from imports (Ruby, Elixir) are out of scope.

## Config sketch

```yaml
# easement.yml
version: 1

components:
  handler:    { in: "internal/handlers/**" }
  service:    { in: "internal/services/**" }
  repository: { in: "internal/domain/*/repository" }
  model:      { in: "internal/models" }

common: [model]

rules:
  handler:    { may_depend_on: [service] }
  service:    { may_depend_on: [repository] }
```

## License

MIT
