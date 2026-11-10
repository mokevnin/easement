# easement

Architecture quality gate for polyglot codebases.

Catch changes that compile but break the architecture: a handler reaching storage directly, a feature importing another feature's internals, or a new dependency cycle.

Declare the components of your system and which of them may depend on which. `easement` extracts the real dependencies from the code, builds the graph and fails the build when the code drifts from the declared architecture.

## Set up with a coding agent

Copy this prompt into Claude Code, Codex, Cursor or another coding agent opened in your project:

```text
Set up easement (https://github.com/mokevnin/easement), an architecture quality gate, in this project. Read https://github.com/mokevnin/easement/blob/main/docs/README.md first. Use installation.md and languages.md for tools and indexers, configuration.md for rules, and adoption.md for baseline and CI setup; these pages are in the same docs directory.

1. Install easement the way this project installs its tools: a line in mise.toml if the project uses mise ("github:mokevnin/easement" = "latest"), otherwise Homebrew or the install script from installation.md.
2. Run `easement doctor --format json` and install every indexer it reports missing, again the way the project installs tools. Repeat until it exits 0.
3. Run `easement init`, then `easement check`. If the report says files are not covered by an indexer, fix the cause (a tsconfig include, a missing go.mod) or list them in `uncovered_ok` with a reason.
4. Study the code and turn the generated config into the intended architecture: remove the allowances that should not exist, give every rule a `because` that tells where the code should go instead, and use `layers`, `public`, `no_cycles` and `tests: auto` where they fit. Do not change application code to make the check pass. When the config is final, record the existing violations with `easement baseline`.
5. Add the check to CI: the GitHub Action from adoption.md with `diff` on pull requests if the project uses GitHub Actions, otherwise `easement check` in the existing pipeline. If the project has git hooks, run it on pre-push.
6. Install the easement skill into the project with `npx skills add mokevnin/easement`, for the agents this team uses, and commit the files it writes, so every agent that works here gets it.
7. Add these rules to the project's agent instructions (AGENTS.md, CLAUDE.md or whatever file the project already uses; create AGENTS.md if there is none):
   - Use the easement skill before adding a file or an import and whenever `easement check` fails.
   - After changing code, run `easement check`; it must exit 0.
   - Never edit easement.yml, regenerate the baseline or add an `easement:disable` comment without asking a human.
8. Run `easement check` once more and report: the components and rules you chose and why, the number of baselined violations, and anything you were unsure about.
```

Review the rules it proposes: the agent sees the code as it is, and you decide which dependencies should be allowed. The setup installs the [agent skill](skills/easement/SKILL.md) for everyday work; [manual skill and Claude Code plugin installation](https://github.com/mokevnin/easement/blob/main/docs/agent-setup.md#agent-skill) are also available.

## Languages

All languages share component boundaries, public APIs, layers, isolation, cycles and coupling rules. The table shows the analysis behind those rules and which additional checks are available:

| Language | Dependency analysis | Symbol rules | Type naming | Unused files | Package declarations |
| --- | --- | --- | --- | --- | --- |
| Go | SCIP | Yes | Yes | Yes | `go.mod` |
| TypeScript / JavaScript | SCIP | Yes | Yes | Yes | `package.json` |
| Python | SCIP | Yes | Yes | Yes | — |
| Rust | SCIP | Yes | Yes | Yes | `Cargo.toml`, features |
| Java | SCIP | Not yet | Yes | Yes | — |
| C# | SCIP | Not yet | Yes | Yes | — |
| PHP | SCIP | Not yet | Yes | Yes | — |
| Ruby | SCIP, constant references | Yes | Yes | No | — |
| Kotlin | Imports only | No | No | No | — |
| Swift | Imports only | No | No | No | — |
| Svelte | Imports only | No | No | No | — |

**SCIP** uses language indexers to resolve references to definitions; install the indexer and the project's dependencies. Kotlin, Swift and Svelte use bundled tree-sitter adapters with approximate import resolution. **Symbol rules** restrict uses of specific types, functions or methods; **type naming** assigns matching type names to components. **Package declarations** checks imports against declared dependencies and applies component rules to dependencies in manifests.

Coverage has limits: TypeScript/JavaScript needs a `tsconfig.json` that includes the files; Ruby needs Sorbet type information and cannot enforce external gem restrictions; unused-file checks stay conservative around incomplete indexes, and framework-loaded files may need explicit entry points. See [language requirements and limits](https://github.com/mokevnin/easement/blob/main/docs/languages.md).

A green verdict covers the code the tool could read and judge. Reports name evidence gaps, deliberate narrowing such as `--diff`, and cache use. In CI, `easement check --require-evidence` also fails when evidence is incomplete.

## What you can enforce

Components represent the parts of your architecture: layers, features, packages or services. Rules express which relationships are allowed and which conventions each part must follow:

| Rule area | Example policy |
| --- | --- |
| Dependencies and layers | Handlers call services; domain code cannot reach storage, even through an intermediate component. |
| Public APIs | Other components use billing's public API, also when another component re-exports billing's code; only maintenance jobs may use its admin surface. |
| Feature isolation | Each feature uses its own data layer; one page cannot import another page's internals. |
| Cycles and coupling | Components stay acyclic and below limits on outgoing dependencies or instability. |
| Symbols and type ownership | Only use cases may call a destructive function; types named `*Repository` belong in the repository component. |
| Package declarations | Imports must be declared in the manifest; production crates cannot enable a test-only Cargo feature. |
| Files and conventions | Every resource directory contains required files or imports; generated files stay outside source checks; unreachable files are reported. |
| Policy contracts and diagrams | Example dependencies pin allowed and rejected decisions; code dependencies must have arrows in the architecture diagram. |

Each violation points to the source and carries the rule's `because`, so a developer or agent knows how to fix it. See [all rules and configuration examples](https://github.com/mokevnin/easement/blob/main/docs/configuration.md).

## Why easement

Choose easement when you want architecture policy that spans languages and can guide coding agents as they work:

- **One policy across the repository.** A Go backend and a TypeScript frontend share a component model, YAML rules, baseline and report. Teams maintain the same architecture vocabulary across languages.
- **Decisions before and after a change.** Agents can ask `explain` where code belongs and `propose` whether an import would violate boundaries or create a cycle. `check` enforces the policy; `why` explains dependencies, `impact` finds dependents, and `review` exposes permissions or debt a policy change introduces.
- **Adoption with explicit trust limits.** A shrinking baseline and diff checks support existing codebases. Reports expose unread code, unreliable suggestions and cached indexes, with an option to fail CI on missing evidence.

For Java rules about inheritance, annotations or method signatures, [ArchUnit](https://www.archunit.org/userguide/html/000_Index.html) goes deeper. For a JavaScript/TypeScript project, [dependency-cruiser](https://github.com/sverweij/dependency-cruiser) offers dependency validation and visualization; [Import Linter](https://import-linter.readthedocs.io/en/stable/contract_types/) provides Python import contracts. Easement's advantage is combining a shared policy across languages with agent queries, policy review and explicit analysis limits. See the [detailed comparison](https://github.com/mokevnin/easement/blob/main/docs/comparison.md).

## Install

```sh
brew install mokevnin/tap/easement
```

Homebrew works on macOS and Linux. You can also use `cargo install easement`, the install script or release packages; see [installation options](https://github.com/mokevnin/easement/blob/main/docs/installation.md).

Most languages also need a SCIP indexer and the project's dependencies installed. From your project directory, run:

```sh
easement doctor
```

It reports missing indexers and how to install them. Follow the [indexer setup](https://github.com/mokevnin/easement/blob/main/docs/languages.md#indexers), then repeat until `doctor` exits 0.

## Quick start

Start from your project's current layout:

```sh
easement init
```

It writes `easement.yml` with components and allowances for the dependencies it finds. Review those allowances and remove the ones the architecture should forbid.

For example, handlers should reach storage through services. For a Go project with `internal/handlers`, `internal/services`, `internal/repository` and `internal/models`, the policy can be:

```yaml
version: 1

components:
  handlers:   { in: internal/handlers }
  services:   { in: internal/services }
  repository: { in: internal/repository }
  models:     { in: internal/models }

common: [models]

rules:
  handlers:
    may_depend_on: [services]
    because: handlers go through services, never straight to storage
  services: { may_depend_on: [repository] }
```

Run the check:

```sh
easement check
```

A handler importing the repository fails with the source location and the reason:

```text
internal/handlers/books.go:6: handlers -> repository is not allowed (written as internal/repository/books.go) [dependency]
  because: handlers go through services, never straight to storage
```

Change the handler to go through the service, then check again. `check` exits 0 when the code matches the policy, 1 on violations and 2 on an error, such as a broken config or a missing indexer. Additional exit codes are described in the [command reference](https://github.com/mokevnin/easement/blob/main/docs/commands.md#everyday-commands).

For an existing codebase, a [baseline](https://github.com/mokevnin/easement/blob/main/docs/adoption.md#baseline-adopting-on-an-existing-project) records today's debt so new violations fail while old ones can be fixed gradually. For a pull request, [diff mode](https://github.com/mokevnin/easement/blob/main/docs/adoption.md#diff-mode-judging-a-pull-request) judges what the change adds.

## Everyday commands

```sh
easement check                    # check the project
easement explain <file>           # who owns the file and what it may do
easement propose <file> <target>  # judge an import before writing it
easement why <a> <b>              # show the imports behind a dependency
easement impact --diff main      # find files depending on changed code
easement review --base main      # review what a policy change permits
easement suggest                  # find unused allowances and missing ones
```

Use `--format json` for structured results. Reports for CI include GitHub annotations, SARIF and JUnit. See the [command reference](https://github.com/mokevnin/easement/blob/main/docs/commands.md) and [CI integration](https://github.com/mokevnin/easement/blob/main/docs/adoption.md#in-ci); `easement <command> --help` lists the available flags.

## Documentation

Start with the [documentation index](https://github.com/mokevnin/easement/blob/main/docs/README.md), or go directly to:

| Task | Guide |
| --- | --- |
| Install easement | [Installation](https://github.com/mokevnin/easement/blob/main/docs/installation.md) |
| Set up with an agent | [Setup prompt](#set-up-with-a-coding-agent) and [skill installation](https://github.com/mokevnin/easement/blob/main/docs/agent-setup.md#agent-skill) |
| Define components and rules | [Configuration reference](https://github.com/mokevnin/easement/blob/main/docs/configuration.md) |
| Query and check the architecture | [Command reference](https://github.com/mokevnin/easement/blob/main/docs/commands.md) |
| Adopt in an existing project or CI | [Baseline, diff, CI and monorepos](https://github.com/mokevnin/easement/blob/main/docs/adoption.md) |
| Set up indexers or understand analysis limits | [Languages, indexers and cache](https://github.com/mokevnin/easement/blob/main/docs/languages.md) |
| Compare architecture tools | [Comparison and inspirations](https://github.com/mokevnin/easement/blob/main/docs/comparison.md) |

## Development

Tools are pinned in `mise.toml`: `mise install`, which also installs the git hooks, `mise run setup` for the npm dependencies, then `mise run check` (every lint, the same set CI runs) and `cargo test`. Commits follow Conventional Commits; releases are cut by merging the release-please pull request. Before proposing a new language, rule or config key, read [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
