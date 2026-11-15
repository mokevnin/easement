# Adopting easement

[Documentation](README.md) · [Project overview](../README.md)

- [Reporting on a part of the project](#reporting-on-a-part-of-the-project)
- [Monorepos](#monorepos)
- [In CI](#in-ci)
- [Baseline: adopting on an existing project](#baseline-adopting-on-an-existing-project)
- [Diff mode: judging a pull request](#diff-mode-judging-a-pull-request)
- [Reviewing a change to the policy](#reviewing-a-change-to-the-policy)

## Reporting on a part of the project

`--component <name>` and `--scope <path>` narrow the report to the files of a component or the files under a path; given together, a file has to satisfy both. The whole project is still read, because cycles, limits and every edge need all of it, so the run is not faster: the violations and the baseline's stale entries are those of the part (a violation is the part's when a line of its evidence is: a cycle through it, a name it re-exports), while the counters and the evidence gaps describe what was read, the whole project. Code outside the part that no indexer read can hide a cycle or a reach chain through it, so `--require-evidence` fails on it too; accept it in `uncovered_ok` when that is meant. With `--watch` the cache makes the reruns fast.

## Monorepos

All paths are relative to the directory of `easement.yml`, so a package of a monorepo gets its own config and `easement check -c packages/api/easement.yml`. Packages share rules with `extends`:

```yaml
# packages/api/easement.yml
extends: ../../easement.base.yml      # a path or a list of paths, relative to this file
components:
  api: { in: src }
```

`components`, `rules` and `sets` merge by name, and a definition in the extending file replaces the one it extends as a whole; `common`, `exclude`, `target_only` and `examples` are joined; any other key is replaced. Patterns in the extended file stay relative to the config being loaded.

The same part of a rule in several components is written once with a YAML anchor and merged with `<<`. A top-level key that starts with `x-` is only a place for such an anchor, easement ignores it:

```yaml
x-no-raw-client: &no-raw-client
  forbid_symbol:
    - { name: "ent.Client", because: use the scoped client }

rules:
  api-site: { <<: *no-raw-client, may_depend_on: [domain] }
  api-external: { <<: *no-raw-client }
```

A key given next to `<<` wins over the merged one, and a list is replaced, not joined. So a list or mapping written there must keep every item or key the merge brings in; one that drops any fails to load, as does the first of `<<: [*a, *b]` when it drops what a later one brings. To drop items on purpose, do not merge that key: split the anchor into smaller ones and write the list for that component out in full. `~`, or a key with no value, keeps none of the merged items, so it is refused too. A scalar such as `because` is simply replaced.

## In CI

`--format` picks the report: `text` (default), `json`, `sarif` (GitHub Code Scanning), `github` (annotations on the changed files, with a warning for each part of the code the check could not read, a notice when the run judges only a part by design (`--diff`, `--component`, `--scope`) and a notice when the indexes came from the cache) and `junit` (a failed test case per violation). `sarif` gives what a run leaves out (the narrowing, each evidence gap with its files, the cache) as notifications of its invocation, errors under `--require-evidence`; `junit` gives them in `<system-out>`, and under `--require-evidence` a failed test case per evidence gap.

The repository is a GitHub Action:

```yaml
- uses: actions/checkout@v4
  with: { fetch-depth: 0 }
- uses: mokevnin/easement@main
  with:
    diff: origin/main      # optional: judge only what the pull request adds
```

Inputs: `version`, `config`, `working-directory`, `diff`, `format` (`github` by default) and `cache` (`true` by default: the indexes are kept with `actions/cache`, saved by runs on the default branch and restored by every run, so a pull request that changes no Go file skips `scip-go`). Without the action, keep `EASEMENT_CACHE_DIR` the same way:

```yaml
- uses: actions/cache@v6
  with:
    path: ~/.cache/easement
    key: easement-${{ runner.os }}-${{ github.sha }}
    restore-keys: easement-${{ runner.os }}-
```

With SARIF:

```yaml
- run: easement check --format sarif > easement.sarif
- uses: github/codeql-action/upload-sarif@v3
  with: { sarif_file: easement.sarif }
```

For git hooks we recommend [mise](https://mise.jdx.dev) to install `easement` and [hk](https://hk.jdx.dev) to run it:

```toml
# mise.toml
[tools]
"github:mokevnin/easement" = "latest"
hk = "latest"
```

```pkl
// hk.pkl
hooks {
    ["pre-push"] {
        steps {
            ["easement"] { check = "easement check" }
        }
    }
}
```

Run it on `pre-push` rather than `pre-commit`: the check reads the whole project.

`easement schema` prints the JSON Schema of `easement.yml`; `easement init` adds it to the file for editors that read a `yaml-language-server` comment. `easement check --watch` runs again after every change and re-reads only the files that changed; the cache lives in memory, so every run of the command starts cold.

## Baseline: adopting on an existing project

`easement baseline` writes current violations to `easement.baseline.yml` (path configurable with `baseline:`). After that the gate is a ratchet:

- violations in the baseline are legalized;
- any new violation fails;
- a baseline entry that no longer occurs also fails, so fixed debt has to be removed from the file with `easement baseline`.

Once the file exists, `easement baseline` only shrinks it: it drops the entries that no longer occur and lists the new violations instead of writing them, exiting 1 while there are any. Rerunning it cannot turn a failing gate green. `easement baseline --accept-new` writes the new violations too, for when a human decides to take on the debt. Three ordinary changes need it: a rule made stricter, a rule taken out of `warn`, and a file with baselined debt moved, since the entries are keyed by file. Entries of files no indexer covered are kept, because their debt was not read, not paid.

Entries are keyed by rule, file, target and components, not by line, so unrelated edits do not invalidate them. A cycle and a coupling limit are keyed by their components and a missing requirement by its directory, so a new file that carries the same debt does not make it new. Every use is a violation of its own, at its own line, so a file that uses a forbidden name three times is reported three times; one baseline entry legalizes all the uses of its key, and a use added next to a legalized one is legalized with it.

## Diff mode: judging a pull request

`easement check --diff <base>` reports only the violations of the change: those whose import line was added or changed since the merge-base with `<base>`, including uncommitted and untracked files and every line of a moved file, which is new where it now is, and those a change to the policy causes. Old violations elsewhere do not fail the change, so the gate can be turned on without a baseline. In CI fetch the base branch (`actions/checkout` with `fetch-depth: 0`).

The report ends with the new dependencies between components that the change brings: `app -> ui` is listed when every dependency behind it is on a changed line, so a reviewer sees the change to the graph and not only the violations. `--format github` adds it as a notice and `--format json` as `stats.new_dependencies`.

A change to the policy is part of the change. `--diff` loads the policy at the merge-base, with the files it extends and its diagram, and judges it on the same code: a violation the new policy produces and the old one did not fails the change although no import line moved. A grant removed or a prohibition added shows up this way, while editing an unrelated rule surfaces no old debt. The header says `and what the change to the policy causes`, and `--format json` lists the changed policy inputs in `diff.policy_changed`.

Some changes cannot be traced to lines, so the run judges more and says why, in the header and in `diff.in_full`:

- A change to what is scanned (`exclude`, `target_only`, `indexers`, `reach`, `packages`, `tests`, `forbid_symbol`, `public_symbols`), a policy input outside the repository (its past is unknown) or a policy at the merge-base that cannot be read: the whole project is judged, so old violations the baseline does not cover fail too.
- A changed `package.json`, `go.mod` or `Cargo.toml`: the `manifests` and `packages` rules are judged in full, since they read the manifest, not a line of it.

Violations caused only by deleting code are not traced: an import removed so that a requirement fails, or a file deleted so that another becomes `unused`. The full `easement check` finds them. The baseline still applies, and stale entries are still reported.

## Reviewing a change to the policy

A pull request can turn green by weakening the policy: a grant added, a surface widened, debt accepted, code excluded. `--diff` fails what such a change rejects, but a permission that no code uses yet produces no violation. `easement review [--base <rev>]` (default `origin/main`) says what the change to the policy since the merge-base permits, forbids and accepts, over the current code, so a reviewer does not have to work it out from the YAML diff:

- **Permissions**: every ordered pair of components whose dependency decision flipped, `web -> storage newly permitted (may_depend_on)` or `newly rejected (must_not_depend_on)`, with its `because`. The pairs are finite, so the list is exhaustive. A new component is listed with what it permits; a removed one by name only, and a rename reads as one removed and one added.
- **Surfaces**: `public`, `allow_importers` and `surfaces` changes as written (`before`, `after`, `added`, `removed`; unset means every file public, any component the rules allow, or no named surface; `public` beside named surfaces is `[]` when unset, since nothing else is open), with the files of today that the change opens or closes: for `public`, to every other component; for `allow_importers`, the component's surface, to the importers added or removed; for `surfaces`, to any component that the surfaces let in now and kept out before, or the other way. No claim is made about what two globs share. Surfaces of an added component are not compared, since it had none; its permissions are listed.
- **Constraints**: every change to a clause that limits what the code of a component, or of the whole project, may use, must use or must contain beyond the decision on a pair, each `loosened` or `tightened`: an item added to or removed from `forbid_symbol`, `forbid_external`, `may_use_external`, `forbid_file`, `must_depend_on`, `must_use_external`, `requires_file`, `requires_directive`, `defines`, `public_symbols`, `unowned` or `entry` (``web.forbid_symbol: removed `tags.DeleteIn` (loosened)``), a list that allows set or unset (unset is no limit), `max_fan_out` or `max_instability` raised or lowered, `strict`, `manifests`, `isolated` or `optional` turned on or off, a component entering or leaving `no_cycles`, a component a top-level `forbid_symbol` item's `allow` now lets use the name (loosened) or no longer does (tightened; a change that does both is listed both ways, each with its components in `components`: ``forbid_symbol: `tags.DeleteIn` may now be used by `web` (loosened)``), a pattern added to the `allow_in` of a top-level `forbid_symbol` item (loosened) or removed from it (tightened), each with its patterns in `paths` (``forbid_symbol: `ent.Client` may now be used in `internal/x` (loosened)``), and an arrow drawn or erased in the `diagram`. A `forbid_symbol` item is judged by who may use the name, not by which list says so: an item a component's own rule adds or drops is listed only when that changes whether the component may use the name, so moving a name from the rules of components into a top-level item that still forbids it to them lists the top-level item alone, and a component its `allow` lets in is listed on that item. A name spelled otherwise is another item. A constraint on code not written yet, which no violation shows, is listed all the same. Items are told apart by name: a list only reordered, or a new `because`, is no change. Each entry carries the `because` a violation of it shows and where the policy writes it (`provenance`): a removed item as the merge-base wrote it, in the file it came from then. A diagram either policy cannot read is named (`unread_diagrams`), its arrows are not compared, and the delta leaves out the `diagram` rule.
- **Policy delta on the current code**: violations the change introduces or removes, with the ones the new baseline legalizes and the ones whose rule moved to `warn` told apart.
- **Ownership**: files whose component changed, became unowned, or became excluded or included.
- **Policy examples**: examples added, removed, or kept for the same dependency with another expectation, with their `because`. Removing one breaks nothing, yet the policy no longer pins that decision. A change to `because` alone is not listed.
- **Debt and trust**: baseline entries added or removed, every one of them, `uncovered_ok`, `exclude` and `target_only` entries added or removed, rules moved to or out of `warn`.

The code at the merge-base is never read: the review answers "the same code under the old policy and under the new one". When the change alters what is scanned (the keys listed under Diff mode), the current code is scanned a second time under the old policy, through the cache, and the review says so (`rescanned` in JSON). An input outside the repository has no past, so its change is not seen; the review names it (`inputs.past_unknown`). `--format json` records the base, the merge-base and the inputs of both policies, and carries `evidence_gaps` (of both scans: code the new policy no longer reads still leaves the delta blind), `accepted_gaps` and `cached` like `check`. It exits 0: the review informs, and `check` gates. A policy example the change breaks shows in the policy delta as an `example` violation it introduces.
