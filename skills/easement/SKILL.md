---
name: easement
description: Keeps code inside the architecture declared in easement.yml. Use in a project that has an easement.yml before adding a file, a directory or an import, after changing code, when `easement check` fails, and when easement.yml or its baseline needs to change.
---

# easement

`easement` checks the dependencies between the components of a project against `easement.yml`. The config is the architecture the team chose: change the code to fit it, never the config to fit the code.

If the project has no `easement.yml`, easement is not set up yet: follow [Set up with a coding agent](https://github.com/mokevnin/easement/blob/main/docs/agent-setup.md).

## Before writing code

Run `easement explain <path>` for the file or directory you are about to add or change. It prints the component the path belongs to, what that component may depend on and why. Put new code where the imports it needs are allowed.

Once you know where an import would go, run `easement propose <file> <target>` (or `--external <module>`, or `--plan -` with a JSON list of `{"from", "to"}` for several imports at once) before writing it. It gives the decision `check` will give the import, and the cycles, `must_not_reach` chains, crossed limits and missing diagram arrows the imports would cause together, each with its witness. Exit 1 means do not write it that way: read `because` and the public surface it names, and pick another target. Under `--no-graph` the graph is not judged, so a pass says nothing about cycles.

`easement why <a> <b>` shows the imports, or the chain of imports, behind a dependency between two components. `explain` and `why` take `--format json` too: every grant and prohibition comes with its `provenance`, the field of the policy and the file that writes it, so you know which file to discuss with a human. `easement policy --format json` gives the whole policy as loaded, `extends` merged and globs and sets expanded, each item with its `provenance`: read it instead of `easement.yml` when you need who may use what across the project.

## After changing code

Run `easement check`. It exits 0 when the code matches the config, 1 on violations and 2 on an error. With `--require-evidence` it exits 4 when there are no violations but the check could not read or judge some code (`evidence_gaps`): close each gap as its `close` says, or ask a human to accept it in `uncovered_ok`.

- On a branch, `easement check --diff <base>` judges the lines you added and what your change to the policy (`easement.yml`, a file it extends, its diagram) causes in unchanged code. When the report says it judged the whole project or a rule in full (`diff.in_full` in JSON), that is why older violations appear. Run the full `easement check` too when the baseline changed or you deleted code: a removed import or file can break a rule on lines nobody changed.
- After proposing a plan, keep it in a file and run `easement check --diff <base> --plan <file>` once the code is written. `Not in the plan` lists each import across components or of an external module that you added and the plan did not name; `Planned, not in the code` lists each entry you did not write. Neither fails the check: fix the code or the plan, or name the difference in your summary.
- `--format json` gives the same report as data. `narrowed` lists what you asked to judge alone (`--diff`, `--component`, `--scope`). `evidence_gaps` lists code the check could not read or judge, each with its files, the place of each part where it knows them (`places`: `file`, `line`, `specifier`), and how to close it: a pass says nothing about that code, so say so and name the files. `accepted_gaps` lists files `uncovered_ok` accepts, with its reason: they are unread too, but by decision. `suppressions` lists each inline suppression the run honoured, at its directive, with the violations it `excused`: they do not fail, but one your change relies on is listed, so name it in your summary. When `cached` is set, the indexes came from the cache; after changing dependencies outside the project without a lock file, run `easement check --no-cache`.
- On exit 2, read the message. For a missing indexer or missing dependencies, run `easement doctor`.
- When your change touches the policy (`easement.yml`, a file it extends, its diagram, the baseline), run `easement review --base <base>` and put what it says in the pull request: the pairs of components newly permitted or rejected, the surfaces widened, the baseline entries and `exclude`, `uncovered_ok` or `warn` entries added. It always exits 0; `check` is the gate.
- `easement impact --diff <base>` lists every file that depends on your change: run the tests among them. When it says `Partial`, files no indexer read or files the change deletes may hide more. `Cached:` means the indexes came from the cache; `--no-cache` indexes again.

## When the check fails

A violation reads like `internal/handlers/books.go:6: handlers -> repository is not allowed (imports internal/repository/books.go) [dependency]`, followed by `because:` with the reason of the rule.

1. Read `because`: it says where the code should go instead.
2. Fix the code: go through the component the rule allows (call the service, not the repository), move the file into the component it belongs to, or turn the dependency around. A `reach` violation lists a chain of imports; remove any one hop of it.
3. Do not edit `easement.yml`, regenerate the baseline or add an `easement:disable` comment to get the check through. If the rule itself looks wrong, stop and ask a human (the `owner` printed with the violation, when there is one), with the violation and why you think the rule is wrong.

Two cases are cleanup, not exceptions:

- **Stale baseline entries**: a violation recorded in the baseline is gone. Run `easement baseline` to drop them. It never writes new violations: it lists them and exits 1, so fix those as above. Legalizing new debt with `easement baseline --accept-new` needs a human.
- **`unused-suppression`**: the excused import is gone, so delete the comment.

Warnings (rules in `warn`) do not fail the check, but they are debt the team tolerates for now: add no new ones, and fix the ones in code you change.

An `unused` violation means no chain of imports from an `entry` reaches the file. Delete it only if your own change added it; otherwise tell a human, because a framework or reflection may load it, and the fix is then an `entry`.

An `excluded` violation means the code imports a file that `exclude` hides, so no rule sees the dependency. Import from a checked component instead; if the file is generated code the project needs, ask a human to move its pattern to `target_only`.

An `unmatched` violation is about the config, not the code: an `in` pattern of a component points at no file, usually because the code moved, or a `part` names no part of the files, usually a typo the message suggests a fix for. It needs a change to `easement.yml`, so tell a human which pattern it is and where the code went.

An `example` violation is about the config too: a policy example in `examples` no longer gets the decision it pins, so a change to the policy undid what it meant (a new grant, a wider `public`), or it names a path no component owns. Do not edit the example to make it pass: undo the change to the policy that flipped it, or ask a human whether the policy is meant to change.

An `unused-allowance` violation, under `easement check --suggest`, is about the config as well: a `may_depend_on` entry no import uses. If your change removed the last import that used it, drop the entry (`easement suggest --patch | git apply`, or by hand in the file `provenance.input` names); otherwise tell a human, who may keep it for code still to come with `warn: [unused-allowance]`.

## Changing easement.yml

Change the config only when a human asks for it or agrees to it.

- A new directory that belongs to no component: add a component with `in`, and a rule with `may_depend_on` and `because`. With `strict: true` such a directory fails as `orphan` until it has a component or is listed in `unowned`.
- Every rule gets a `because` that tells the reader where the code should go instead.
- A prohibition no code exercises yet gets a policy example (`examples: [{ from: <file>, to: <file>, expect: rejected }]`, or `symbol: <name>` in place of `to` for a `forbid_symbol` name), so a later grant cannot undo it unnoticed.
- `easement suggest` lists the allowances no import uses and the imports nothing allows. `easement suggest --patch | git apply` drops the unused allowances and keeps comments and reasons. Leave alone the entries it lists apart as unreliable: they sit next to files no indexer covered.
- Run the full `easement check` after every change to the config.

`easement schema` prints the full format of `easement.yml`; read the [configuration reference](https://github.com/mokevnin/easement/blob/main/docs/configuration.md) when choosing or changing a rule.
