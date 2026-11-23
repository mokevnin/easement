---
status: accepted
---

# An accepted gap is a reasoned entry of one kind, and one that accepts nothing fails

`uncovered_ok` accepted files no indexer read, by glob, with an optional reason, and an entry that matched nothing was silently ignored. No other gap could be accepted, so a project with one reviewed computed import (`import.meta.glob('./locales/*.json')`, `` import(`./plugins/${name}.js`) ``) could never pass `check --require-evidence` ([#253](https://github.com/mokevnin/easement/issues/253)). There are no users to keep compatible with, so the shape follows the rest of easement rather than the history of `uncovered_ok`:

1. **One list, keyed like the report.** `uncovered_ok` is replaced by `accepted_gaps`. Each entry names the evidence-gap `kind` it accepts, with the name of the report's `evidence_gaps[].kind` (`uncovered`, `unresolved_imports`). Input and output share one vocabulary, and the report's `accepted_gaps` list (ADR-0001) has one source. A sibling key per kind of gap would add a top-level key for every kind.
2. **Only a gap that is a decision about the code can be accepted:** `uncovered` files, and `unresolved_imports` whose target is computed at run time. Every other cause of an unresolved import (no target, an unwalked file, a wildcard declaration, a side-effect import, an import call) has a fix in the code that its `close` names, and every other kind (`unknown_scheme`, `std_unknown`, unreadable manifests, …) is a fault of the tool or the environment. Accepting any of them would cement a broken setup, so an entry of another `kind` fails to load.
3. **A reason is required.** `because` is mandatory and non-empty in every entry, and the plain-glob form is gone, as Clippy's `allow_attributes_without_reason` asks of Rust's `allow` and `expect`.
4. **An entry that accepts nothing fails as `unmatched`** at its line, like an `in`, `entry` or `allow_in` pattern (and a `forbid_symbol` name, ADR-0004), on the usual terms: `--diff` keeps it, the baseline never accepts it, and nothing suppresses it. Rust's `#[expect]` warns when its lint is not emitted (`unfulfilled_lint_expectations`), and import-linter's `unmatched_ignore_imports_alerting` defaults to `error`. An entry whose files are all accepted by an earlier entry still matches them, so it is not stale.
5. **A gap that depends on the platform is no policy.** A file uncovered only because its indexer cannot run on this platform (`cannot_run`) is never accepted: the policy describes the code, not the machine. An `uncovered` entry whose globs reach such files is not judged stale on that platform, since an incomplete index cannot tell (as ADR-0004 says nothing with files no indexer covered).
6. **An import is matched by `file` and `specifier`, exactly.** The `file` is the importing file's path, with no glob; the `specifier` is the text the report gives in `places[].specifier`. Every computed import with that text in that file is accepted, and no line is involved, so the entry survives edits around it. Without a glob, a future computed import is never accepted unreviewed.
7. **An entry that meets an import of another cause** (its `specifier` is in the file but unresolved for another cause, say no target) fails as `unmatched` with that cause's `close`.
8. **Graph answers are unchanged:** `impact`, `why`, `suggest` and `graph` still list an accepted gap as an evidence gap too (ADR-0001).

Each item of the report's `accepted_gaps` carries the `kind`, the `because`, where the policy writes the entry (`provenance` and its `line`) and what an evidence gap of that kind would hold (`files`, or `places` for imports). Every format names the entry by where it is written, `accepted_gaps[0] at easement.yml:12`.

## Considered options

- **A key of its own for computed imports beside `uncovered_ok`** (`computed_ok: [{ file, specifier, reason }]`). Rejected: a new top-level key for every kind of gap, each with its own spelling of the kind the report already names.
- **Accept any cause of an unresolved import.** Rejected: those with a fix in the code would hide a real gap behind a reason.
- **Keep an entry that matches nothing silent**, as `uncovered_ok` did. Rejected: a stale acceptance reads as a decision nobody can see is gone, which is what `unmatched` exists to prevent.
- **Match an import by glob or by line.** Rejected: a glob accepts imports nobody reviewed, and a line breaks on every edit above it.

## Consequences

- `uncovered_ok` fails to load, with a message that names `accepted_gaps`. So does a merge-base policy that still writes it: `check --diff` and `review` across the change that replaces it fail to load the old policy, and the change is checked in full instead.
- On Windows, the files of an indexer that cannot run there stay an evidence gap: they are checked on Linux, macOS or WSL.
- `review` lists what `accepted_gaps` accepts now and did not before under "Debt and trust", each glob apart; an added entry loosens.
