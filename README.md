# easement

Architecture quality gate for polyglot codebases.

Declare the components of your system and which of them may depend on which. `easement` extracts the real dependencies from the code, builds the graph and fails the build when the code drifts from the declared architecture.

> **easement** — a legal right to cross someone else's land. In `easement`, every allowed dependency between components is an explicit easement; everything else is trespassing.

## Install

```sh
brew install mokevnin/tap/easement
```

Homebrew works on macOS and Linux. Other options:

- the install script for macOS and Linux, which checks the archive against the checksums of the release and installs to `/usr/local/bin`:

  ```sh
  curl -fsSL https://raw.githubusercontent.com/mokevnin/easement/main/install.sh | sh
  ```

  Pass `-b <dir>` to install elsewhere and `-v <version>` to pin a release: `... | sh -s -- -b ~/.local/bin -v 0.1.0`;
- `.deb` and `.rpm` packages for amd64 and arm64 on each [GitHub release](https://github.com/mokevnin/easement/releases): `sudo apt install ./easement_<version>-1_amd64.deb` or `sudo dnf install ./easement-<version>-1.x86_64.rpm`;
- `cargo install easement`;
- prebuilt binaries for macOS, Linux and Windows, with a PowerShell installer for Windows, on each release.

## Status

Early. Go, TypeScript/JavaScript, Python, Java, Rust, C#, Kotlin, PHP and Swift work.

## Usage

```sh
easement init                  # write a starting easement.yml from the code as it is
easement check                 # exit 0: passed, 1: violations, 2: error
easement check --format json   # machine-readable report; also sarif, github, junit
easement check --diff origin/main  # only imports added in this change
easement check --watch         # check again on every change
easement baseline              # legalize current violations
easement explain src/app/a.ts  # which component a file is in and what it may do
easement graph                 # the components as a Mermaid diagram (--format dot)
easement schema                # the JSON Schema of easement.yml
```

`easement init` finds the components in the layout of the project (the subdirectories of `internal/`, `src/`, `app/`, … or the top-level directories) and allows every dependency it finds between them. The first `easement check` passes; remove the dependencies that should not exist, and add `because:` reasons.

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
- Files outside every component are not checked, unless `strict: true` turns them into `orphan` violations. External modules are checked only where a rule says so, see [External modules](#external-modules).
- **exclude**: globs to skip, for the whole project. `.gitignore` is honoured and `node_modules` is always skipped. Excluded files are not checked at all; to hold tests to rules of their own, make them a component whose pattern is longer than the one around them, since the longest pattern wins: `handler_tests: { in: "internal/handlers/*_test.go" }` takes the tests out of `handler`. A shorter glob such as `**/*_test.go` loses to the directory.
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
| `external` | a component uses an external module its rule does not allow |
| `metric` | a component exceeds `max_fan_out` or `max_instability` |
| `orphan` | with `strict: true`, a checked file belongs to no component |
| `suppression-reason` | an inline suppression has no reason |
| `unused-suppression` | an inline suppression matches no violation, or names a rule that cannot be suppressed (`dependency`, `privacy` and `external` can be) |

### Public API

```yaml
components:
  billing:
    in: src/billing
    public: [src/billing/index, src/billing/api]
    because: billing internals change without notice
```

Other components may import only the public paths (`src/billing/index.ts`, anything under `src/billing/api`); code inside `billing` reaches its own internals freely. A component without `public` is open.

### Layers and forbidden dependencies

```yaml
layers: [handler, service, repository]   # top to bottom

rules:
  handler:
    must_not_depend_on: [repository]
    because: handlers go through services
```

A layer may depend on every layer below it and never on one above. `must_not_depend_on` wins over everything that would allow the dependency: `may_depend_on`, `common` and `layers`. Its violations read `handler -> repository is forbidden`.

### External modules

```yaml
rules:
  domain:
    forbid_external: [moment, "@acme/*", net/http]
  ui:
    may_use_external: [react, lodash]
```

`forbid_external` lists modules the component must not use. `may_use_external` is an allow-list: everything else is a violation; without it any external module is fine. A pattern covers submodules, split at `/`, `.`, `::` and `\`: `lodash` covers `lodash/fp`, `@mantine/core` covers `@mantine/core/styles.css`, `java.util` covers `java.util.List`, `serde` covers `serde::Deserialize`. The standard library counts as external in Go, Python, Java and Rust, so an allow-list has to name it. `because` of the rule is printed with these violations too; it is one reason per component, not per key.

### Coupling limits

```yaml
rules:
  core:
    max_fan_out: 4          # components it depends on
    max_instability: 0.7    # out / (in + out), 0 is stable and 1 is unstable
```

Both count dependencies between components, so `common` components are counted too. A violation names the limit and the components behind it.

### Strict mode

`strict: true` fails on every checked file that belongs to no component, so a new directory cannot slip past the gate unnoticed. There is no way to exempt a directory yet: put it in `exclude` or make it a component.

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

## Monorepos

All paths are relative to the directory of `easement.yml`, so a package of a monorepo gets its own config and `easement check -c packages/api/easement.yml`. Packages share rules with `extends`:

```yaml
# packages/api/easement.yml
extends: ../../easement.base.yml      # a path or a list of paths, relative to this file
components:
  api: { in: src }
```

`components` and `rules` merge by name, and a definition in the extending file replaces the one it extends as a whole; `common` and `exclude` are joined; any other key is replaced. Patterns in the extended file stay relative to the config being loaded.

## In CI

`--format` picks the report: `text` (default), `json`, `sarif` (GitHub Code Scanning), `github` (annotations on the changed files) and `junit` (a failed test case per violation).

The repository is a GitHub Action:

```yaml
- uses: actions/checkout@v4
  with: { fetch-depth: 0 }
- uses: mokevnin/easement@main
  with:
    diff: origin/main      # optional: judge only what the pull request adds
```

Inputs: `version`, `config`, `working-directory`, `diff` and `format` (`github` by default). With SARIF:

```yaml
- run: easement check --format sarif > easement.sarif
- uses: github/codeql-action/upload-sarif@v3
  with: { sarif_file: easement.sarif }
```

For [pre-commit](https://pre-commit.com), `.pre-commit-hooks.yaml` offers `easement` (built from source with cargo) and `easement-system` (the `easement` on the `PATH`).

`easement schema` prints the JSON Schema of `easement.yml`; `easement init` adds it to the file for editors that read a `yaml-language-server` comment. `easement check --watch` runs again after every change and re-reads only the files that changed; the cache lives in memory, so every run of the command starts cold.

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
| Rust | `use`, `mod x;` | the module tree of the crate: `crate::`, `self::`, `super::`, and other crates of the workspace by name; a path points at the deepest module file it passes through |
| C# | `using`, `using static`, `using Alias =`, `global using` | namespaces indexed by their `namespace` declarations: a namespace is the directory of the first file that declares it, a type is `Type.cs` next to it |
| Kotlin | `import`, `import … as`, `import ….*` | top-level declarations indexed by the `package` of their file, like Java; a file may hold several classes |
| PHP | `use`, `use function`, `use const`, group `use`, `require`/`include` of a literal | PSR-4 maps of every `composer.json`; a function or constant points at the directory of its namespace |
| Swift | `import` | modules in the SwiftPM layout: `Sources/<Module>` next to a `Package.swift` |

Known limits: references by fully qualified name without an import (Java, C#, Kotlin, PHP) are not seen. Imports whose target is computed at run time (`importlib.import_module(name)`, `require(path)`, `import(path)`, `Class.forName(name)`) cannot be checked; the report counts them (`N dynamic imports not analysed`) and `--format json` lists them in `stats.dynamic`. easement sees imports of packages, files and modules, not of symbols, so a rule cannot tell `ent.Client` from another name in the same package. In Rust, `#[path]` attributes and items re-exported through another module are not followed. A C# namespace that spans several components resolves to the first of its directories, so keep a namespace in one component. Swift modules declared with a custom `path:` in `Package.swift` are not found.

Adding a language means implementing one trait (`src/lang/mod.rs`): extract imports from a file, resolve a specifier to a project path or an external module. Rules, baseline and reports are language-agnostic.

Languages whose dependencies are not visible from imports (Ruby, Elixir) are out of scope.

## How it works

1. Walk the project, parse each supported file with tree-sitter and collect imports.
2. Resolve every import to a file or package directory inside the project, or to an external module.
3. Map both ends to components and check each edge against the rules.
4. Compare violations with the baseline and report.

## Inspired by

`easement` builds on ideas from these tools, each of which covers one language or ecosystem:

- [ArchUnit](https://www.archunit.org/): architecture rules as tests for Java.
- [dependency-cruiser](https://github.com/sverweij/dependency-cruiser): dependency rules for JavaScript and TypeScript.
- [import-linter](https://github.com/seddonym/import-linter): import contracts between Python modules.
- [Deptrac](https://github.com/deptrac/deptrac): layers and allowed dependencies for PHP.
- [arch-go](https://github.com/arch-go/arch-go): architecture checks for Go.
- [depguard](https://github.com/OpenPeeDeeP/depguard): import allow and deny lists for Go.

`easement` puts one component model, one config and one baseline over all of them.

[oxc_resolver]: https://github.com/oxc-project/oxc-resolver

## Development

Tools are pinned in `mise.toml`: `mise install`, `mise run setup` for the git hooks, then `mise run check` (every lint, the same set CI runs) and `cargo test`. Commits follow Conventional Commits; releases are cut by merging the release-please pull request.

## License

MIT
