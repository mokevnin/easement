# Commands

[Documentation](README.md) · [Project overview](../README.md)

- [Everyday commands](#everyday-commands)
- [init](#init)
- [suggest](#suggest)
- [impact and why](#impact-and-why)
- [propose](#propose)
- [policy](#policy)
- [doctor](#doctor)

## Everyday commands

Start from the code as it is: `easement init` finds the components in the layout of the project and allows every dependency it sees between them.

```text
$ easement init
easement: wrote easement.yml with 4 components and 5 allowed dependencies; remove the dependencies that should not exist, then run `easement check`
```

```yaml
rules:
  handlers: { may_depend_on: [repository, services] }
  repository: { may_depend_on: [models] }
  services: { may_depend_on: [models, repository] }
```

Then say what the architecture should be. Handlers should not reach the storage directly, so drop `repository` from their allowances and give the reason:

```yaml
  handlers:
    may_depend_on: [services]
    because: handlers go through services, never straight to storage
```

`easement check` now points at the code that breaks the rule, like any linter:

```text
$ easement check
internal/handlers/books.go:6: handlers -> repository is not allowed (written as internal/repository/books.go) [dependency]
  because: handlers go through services, never straight to storage

easement failed: 1 violation(s), 0 stale, 0 legalized; 5 files, 6 internal deps, 1 files outside components
```

It exits 0 when the code matches the config, 1 on violations and 2 on an error (a broken config, a missing indexer); 3 and 4 come from `--cache-only` and `--require-evidence`. Fix the import (or, if it is right, the rule) and run it again.

The commands you will use most:

```sh
easement init                 # write a starting easement.yml
easement check                # check the project
easement check --diff main    # only what this branch adds, see Diff mode
easement baseline             # record today's violations; later runs only shrink it
easement explain <file>       # which component a file is in and what it may do
easement policy               # the policy as loaded: extends merged, globs and sets expanded
easement why <a> <b>          # the imports, or the chain, behind a -> b
easement propose <file> <target>  # the decision on an import not written yet, and what it does to the graph
easement impact --diff main   # every file that depends on what this branch changed
easement review --base main   # what this branch's change to the policy permits and accepts
easement suggest              # allowances nothing uses, imports nothing allows
easement check --suggest      # check, and fail on allowances nothing uses too
easement doctor               # are the indexers the project needs installed
easement doctor --cache       # would a check read every index from the cache
```

`easement --help` and `easement <command> --help` list the rest: the report formats for CI, `graph`, `schema`, `--watch`, the cache flags, shell completions (`easement completions zsh > ~/.zfunc/_easement`). The text report is coloured on a terminal; `--color auto|always|never` overrides that, and `auto` honours `NO_COLOR` and `CLICOLOR_FORCE`.

## init

`easement init` finds the components in the layout of the project (the subdirectories of `internal/`, `src/`, `app/`, … or the top-level directories) and allows every dependency it finds between them. The first `easement check` passes; remove the dependencies that should not exist, and add `because:` reasons.

## suggest

`easement suggest` reads the real graph against the config and lists:

- the `may_depend_on` entries no dependency uses;
- the dependencies that nothing allows, and those the config forbids;
- the arrows of the diagram that no dependency stands for;
- the components whose instances never depend on each other and so could be `isolated` (only `*` patterns, never `**`).

An entry next to files no indexer covered is listed apart as unreliable, because the dependency it would remove may be in a file that was never read. Check those by hand before removing anything. `--format json` gives the same lists as data (`removable`, `unreliable`, `not_allowed`, `forbidden`, `stale_arrows`, `isolation`), each item next to unread files with `unread`, the files of each component no indexer read, and the `evidence_gaps`, `accepted_gaps` and `cached` of `check`.

- `--patch` prints the removals as a patch: `easement suggest --patch | git apply`. It edits the text of `easement.yml`, so comments and `because` reasons stay, and it never touches an entry next to unindexed files. An entry it cannot place (a rule that comes from `extends` or an alias, a list written over several lines) is named on stderr to remove by hand, and so is a glob or a set of components, a rule key or an item, with the allowances no import uses: it covers other components too.

`easement check --suggest` gives both verdicts in one pass, so that CI keeps the allowances from piling up without reading the indexes twice. Each entry `--patch` would remove fails as `unused-allowance`, at its line in `easement.yml`, with the message `suggest` gives and the `provenance` of the entry; one that a file named in `extends` writes fails at line 1, with that file in `provenance.input`, to remove there. Like `unmatched`, it is about the policy: the baseline never accepts it and no comment suppresses it, but `warn: [unused-allowance]` reports it without failing. An entry `--patch` leaves alone does not fail, since whether it can go is not known, and is listed instead, under its heading in the text, in `--format json`, and as a note in the `github`, `sarif` and `junit` reports: one next to files no indexer covered under `unreliable_allowances`, and one that comes through a glob or a set, which may let in another component that uses it, under `held_allowances`. Whether an allowance is used depends on every import, so `--suggest` judges the whole project and refuses `--diff`, `--component` and `--scope` (exit 2).

## impact and why

`easement impact <paths>` lists every file that depends on the given files or directories, directly or through others, grouped by component: what a change can break and which tests to run. `--diff <base>` adds the files changed since the merge-base, and `--format json` gives `changed` and `dependents`. When files were not covered by an indexer, `evidence_gaps` says so, because dependents through them are missing.

`easement why <a> <b>` says whether the config allows `a -> b` and why, then lists the imports behind it with file and line, each alias a line writes as a hop from the line through the definition of every alias to the file (`gw/gw.go:5 -> chunk/chunk.go:9 -> blob/blob.go`), or, when there is none, the shortest chain of imports from `a` to `b`.

With direct dependencies, `why` then gives the size of the edge: the number of distinct names of `b` that `a` writes, a module or a name at the top level of one (never a member reached through a value, whose count depends on the indexer), each with how many places write it and `[not public]` when the policy does not let it in through `b`'s public surface. A name re-exported or written through an alias counts once, in the file that defines it. A module of `b` that `a` names without writing any name through it (an import for its effect, a namespace it never reads) is listed apart, so the edge never looks smaller than its imports. The files of a language whose uses of names easement does not read (the registry's `symbols`: Java, Kotlin, C#, PHP and Swift) are an evidence gap: their dependencies are listed, the names they write are not counted.

`--format json` gives the same answer as data. `why` carries the `decision` on the pair (`allowed`, the `grant` or the clause it is `rejected_by`, the `rules` it breaks: `dependency` for a direct dependency and `reach` for a chain under `must_not_reach`, `because` and `provenance` as in `check`), each direct dependency in `dependencies` with the `decisions` on what is written at its line, the rules it `violates` and, for an alias written there, its `aliases` as typed `hops` with the file they `end` in, `names`, one per name of `b` that the direct dependencies write, sorted by name, with the `file` of `b` that defines it, every place of `a` that writes it in `places`, and `public`: whether its public files, the named surfaces open to `a` (`privacy`) and its `public_symbols` let it in at every place, absent when `b` declares none of them; `modules`, each module of `b` named without a name, with its `places`; or the `chain` with the file it `end`s in (then `names` is empty: the size is that of the direct edge), with `evidence_gaps`, `accepted_gaps` and `cached` as in `impact`. `explain` carries the `ownership` of the path (`status` `owned`, `unowned` or `excluded`, the `component`, the `pattern` that owns it, its `captures`, `instance`, whether it is `test` code or `target_only`, and where the component is defined), the component's `grants`, `prohibitions` and `not_allowed` with the same decision fields, its `public` surface, `surfaces` and `allow_importers`, and its `rule`, the `symbols` that bind the path: in `forbidden`, each name it may not use (an item of the top-level `forbid_symbol` that lets neither its component nor its path in, with who may use it in `only`, then an item of its component's own `forbid_symbol`; a name written alike in both lists once), with the `because` a violation gives and the `provenance` of the item, in `allowed`, each name of the top-level `forbid_symbol` it may use all the same, with the component of the path `allow` lets in or the `allow_in` pattern that matches it, the `because` and the `provenance` of the grant (the component's own `forbid_symbol` still forbids a name an `allow_in` pattern lets in), and in `public`, each component the path's component may depend on that has `public_symbols`, with the `names` open to it, the component's `because` and `provenance` (`symbols` is left out when no symbol rule binds the path, or the scan reads none of its uses: `exclude` or `target_only`), and its `forbid_external`: each item of the top-level `forbid_external`, with the component of the path its `allow` lets in or the `allow_in` pattern that lets the path in, whether the path is `allowed` the module, who may (`only`), the `because` and the `provenance` of the grant, else of the item. A path no component owns is an error in text and a status in JSON, unless the top-level `forbid_symbol` or `forbid_external` binds it: the text then lists their items.

`easement graph --focus <component>` draws only that component and the ones it shares an arrow with.

## propose

Before writing an import, `easement propose <file> <target>` says what the policy will make of it: the dependency decision `check` would give the import, with the grant that allows it or the rule that rejects it and its `because`. A target outside the public surface of its component is rejected as `privacy`, and the report names the surface. `--external <module>` proposes an external module instead of a path. Neither path needs to exist: what decides is the path, so a new file is judged as the component, instance and slice it would land in, and a file of a Go package as the package.

The proposal is also laid over the project's graph, and the report lists what it would add there: a new component cycle, a new `must_not_reach` chain, a crossed `max_fan_out` or `max_instability`, a dependency the diagram has no arrow for. Each comes with a witness, the steps that make it, at the file and line of an existing import or naming the proposal, so a cycle reads as the shortest one through a proposal:

```text
src/checkout/pay.ts -> src/billing/api/charge.ts: allowed by may_depend_on (checkout -> billing)

What they do to the graph:
  component dependency cycle: checkout -> billing -> checkout [cycle]
    checkout -> billing: proposed: src/checkout/pay.ts -> src/billing/api/charge.ts
    billing -> checkout: src/billing/api/charge.ts:1
```

`--plan <file>` (or `-` for stdin) judges several at once, a JSON list of `{"from": ..., "to": ...}` and `{"from": ..., "external": ...}`, and lays them over the graph together: two proposals that are allowed alone can close a cycle together, and the consequence names both. `--format json` gives `proposals` (one decision each: `allowed`, `rejected`, `unowned`, `excluded` or `target_only`, with `grant`, `rule`, `message`, `because` and `public_surface`) and `consequences` (each with `witness` and the indexes of its `proposals`), with `narrowed`, `evidence_gaps`, `accepted_gaps` and `cached` as in `check`.

The baseline and suppressions never apply: they are for code that is already written. A source no component owns is `unowned`, a rejection (`orphan`) under `strict` unless `unowned` lists it; a source `exclude` hides is `excluded`, and one `target_only` names is `target_only`: `check` never reads their imports, so no rule judges them. A cycle names every proposal that makes an edge of its group of components, not only those on the shortest cycle shown. A test-only part of a file (Rust's `#[cfg(test)]`) cannot be proposed: only the test globs make a path test code, and the report notes it for such a file. The graph's evidence gaps (files no indexer read, test code not told apart) carry over. `--no-graph` judges the decisions alone, without scanning the project, and the report is `partial` for it. It exits 0 when every decision and consequence passes, 1 when one fails (a rule in `warn` is reported but does not fail), and 2 and 3 as `check` does; it takes `--config`, `--no-cache` and `--cache-only` like `check`.

## policy

`easement policy` prints the policy as loaded: `extends` merged, the YAML anchors, the globs and the sets expanded. It reads no code and runs no indexer, so it answers at once. Check a document of the project, such as a list of who may use a client, against `easement policy --format json` instead of reading `easement.yml` again by hand.

`--format json` gives:

- `inputs`: each policy input, its `path` and its `kind` (`config`, `extended`, `diagram`), the configuration first;
- `components`: each component with its `name`, its `in` and `except` patterns, its `public` surface, `public_symbols`, `surfaces`, `allow_importers`, `isolated`, `optional`, `priority`, `owner`, `defines`, `part` and `because`, its `rule` (the fields of `explain`'s `rule`, and its own `forbid_symbol`; each item of its `forbid_symbol` and `forbid_external` with its own `provenance`), and its `grants` and `prohibitions` on the other components with the decision fields of `explain`;
- `common`, each with its `name`, and `layers`, from the top one down, each row with its `components` and whether it is `closed`;
- `forbid_symbol` and `forbid_external`: each top-level item with its `name`, its `because`, the components its `allow` lets use it (globs and sets expanded) and the paths of its `allow_in`, apart;
- `examples`: each with `from`, `to`, `external` or `symbol` (with its `defined_in`), `expect`, `rule` and `because`;
- the settings `no_cycles` (`true`, `false` or the components), `strict`, `unowned`, `tests` (whether `tests: auto` is on), `entry`, `exclude`, `target_only`, `uncovered_ok`, `reach`, `manifests`, `packages` and `warn`.

Each component, rule, decision and item carries its `provenance` (the settings carry none): the `field` in the merged policy (`rules.web.may_depend_on[0]`, `examples[1]`), the `input` that writes it, and the globs and sets it comes `through`. The component `tests: auto` makes is written at `tests`.

## doctor

`easement doctor` looks at the languages the project has and says, for each, whether its indexer is on the `PATH`, which version, and whether what it needs is in place (`node_modules` for TypeScript, `composer install` for PHP). For a missing indexer it prints how to install it and, where mise can, the lines for the `[tools]` of a `mise.toml`.

It exits 1 when something is missing, so a script or an agent can run it first and read `--format json`.

`easement doctor --cache` says instead, for each directory an indexer would index, whether a run would read its index from the cache: `cached`, or `would run` with the reason (no cache directory, the indexer not installed, no cached index of the directory, the files it reads changed, or the indexer, its arguments or its environment changed). It computes the key `check` computes, with the same function, but reads no index, runs no indexer and judges nothing, so it answers in the time it takes to hash the files. It exits 0 when every index is cached and 3 when one is not, the code of `--cache-only`; `--format json` gives `cached` and, per index, `language`, `dir`, `cached` and `reason` (`no_cache_dir`, `not_installed`, `no_entry`, `files_changed`, `indexer_changed`). An entry that is there but cannot be read counts as cached; `check` then indexes again. A CI job runs it first and sets up an indexer's toolchain only when it exits 3, then runs `check`:

```sh
easement doctor --cache || setup-toolchains   # pnpm install, the Go module cache, …
easement check --suggest
```
