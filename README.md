# easement

Architecture quality gate for polyglot codebases.

Declare the components of your system and which of them may depend on which. `easement` extracts the real dependencies from the code, builds the graph and fails the build when the code drifts from the declared architecture.

> **easement** — a legal right to cross someone else's land. In `easement`, every allowed dependency between components is an explicit easement; everything else is trespassing.

## Install

```sh
brew install mokevnin/tap/easement
```

Homebrew works on macOS and Linux. Other options:

- `cargo install easement`;
- prebuilt binaries for macOS, Linux and Windows, with shell and PowerShell installers, on each [GitHub release](https://github.com/mokevnin/easement/releases).

## Status

Early. Go, TypeScript/JavaScript, Python and Java work.

## Usage

```sh
easement check                 # exit 0: passed, 1: violations, 2: error
easement check --format json   # machine-readable report
easement check --diff origin/main  # only imports added in this change
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
  service:
    may_depend_on: [repository]
    because: services never talk HTTP

exclude: ["**/*_test.go"]
```

- **components**: an `in` pattern (a glob or a list of globs) names a module, whether it is a directory or a file: `src/app/cli` covers the `cli/` directory with everything below it and a file `cli.py`. `*` stays within one directory, `**` crosses directories. When several components match a file, the longest pattern wins, so a single file can be carved out of a larger component.
- **rules**: an allow-list. A component may depend on itself, on `common` components and on what `may_depend_on` lists — nothing else. `because` is printed with every violation of the rule, so the reader (or an agent) learns where the code should go instead.
- Files outside every component are not checked; external packages are not checked yet.
- **exclude**: globs to skip. `.gitignore` is honoured and `node_modules` is always skipped.
- **python.roots**: `sys.path` entries inside the project, default `[".", "src"]`.

```text
$ easement check
internal/handlers/books.go:6: handler -> repository is not allowed (imports example.com/shop/internal/repository) [dependency]

easement failed: 1 violation(s), 0 stale, 0 legalized; 5 files, 6 internal deps
```

## Rules

Every violation carries a rule id, used in the JSON report, the baseline and suppressions.

| Rule | Fails when |
|---|---|
| `dependency` | a component imports one that its `may_depend_on` does not list |
| `privacy` | a component imports a path of another component outside that component's `public` API |
| `cycle` | components covered by `no_cycles` depend on each other in a cycle |
| `suppression-reason` | an inline suppression has no reason |
| `unused-suppression` | an inline suppression matches no violation, or names a rule that cannot be suppressed |

### Public API

```yaml
components:
  billing:
    in: src/billing
    public: [src/billing/index, src/billing/api]
    because: billing internals change without notice
```

Other components may import only the public paths (`src/billing/index.ts`, anything under `src/billing/api`); code inside `billing` reaches its own internals freely. A component without `public` is open.

### Cycles

```yaml
no_cycles: true                  # or a list: [billing, orders, catalog]
```

A group of components that all reach each other is reported once, with the shortest cycle to break:

```text
src/billing/charge.ts:1: component dependency cycle among 3 components: billing, catalog, orders (shortest: billing -> orders -> billing) [cycle]
```

In diff mode a cycle is reported when any import inside it was added by the change.

### Suppressions

An exception for one import, in a comment of any language, always with a reason after `--`:

```ts
// easement:disable-next-line dependency -- legacy admin export, remove with #123
import { exportAll } from "../admin/export";
import { client } from "../billing/generated"; // easement:disable-line privacy -- generated client
```

A suppression without a reason does not suppress and fails as `suppression-reason`. One that no longer matches a violation fails as `unused-suppression`, so exceptions do not outlive the code they excused. Prefer the baseline for old debt; suppressions are for deliberate, reviewed exceptions.

## Baseline: adopting on an existing project

`easement baseline` writes current violations to `easement.baseline.yml` (path configurable with `baseline:`). After that the gate is a ratchet:

- violations in the baseline are legalized;
- any new violation fails;
- a baseline entry that no longer occurs also fails, so fixed debt has to be removed from the file with `easement baseline`. The baseline can only shrink.

Entries are keyed by rule, file, target and components, not by line, so unrelated edits do not invalidate them. A cycle is keyed by its components.

## Diff mode: judging a pull request

`easement check --diff <base>` reports only violations whose import line was added or changed since the merge-base with `<base>`, including uncommitted and untracked files. Old violations elsewhere do not fail the change, so the gate can be turned on without a baseline. In CI fetch the base branch (`actions/checkout` with `fetch-depth: 0`).

The baseline still applies, and stale entries are still reported. A rule change in `easement.yml` does not touch the importing lines, so it shows up only in a full `easement check`; run both on pull requests if the config changes often.

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

## Development

Tools are pinned in `mise.toml`: `mise install`, `mise run setup` for the git hooks, then `mise run check` (every lint, the same set CI runs) and `cargo test`. Commits follow Conventional Commits; releases are cut by merging the release-please pull request.

## License

MIT
