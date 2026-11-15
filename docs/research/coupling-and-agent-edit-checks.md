# Coupling metrics and agent-edit checks: what easement can take from cargo-coupling and repopilot

Assessed on 2026-11-15 against easement at `2c61967`. Sections 1–4 hold facts: Khononov's own material, the source and documentation of the two tools, and easement's code and two small indexer probes. Section 5 holds the recommendations and is the only place they appear. Anything not observed or read is marked **unverified**. This note extends [feature-opportunities.md](feature-opportunities.md), [feature-feasibility.md](feature-feasibility.md) and [feature-candidates.md](feature-candidates.md). It relies on [gate-or-linter.md](gate-or-linter.md) §3 for what SCIP gives about kinds, signatures and relationships, and on [symbol-use-dependencies.md](symbol-use-dependencies.md) for which written names and member uses each indexer emits. It does not repeat that work.

## The question

Which product ideas could easement take from two tools:

- **cargo-coupling**, which scores coupling with Vlad Khononov's *Balancing Coupling in Software Design* model (integration strength, distance, volatility, balance);
- **repopilot**, which reviews the changes a coding agent made: callers broken by a change, and risky boundary changes.

For each idea: does easement already have it, what is missing, can it be built in easement's shape (one rule for every language, judged on the index, with the registry supplying what differs, and output that says what not to trust), which index facts it needs, and whether it is a gate (pass/fail) or a report.

## Method

- **Khononov.** The pages of [coupling.dev](https://coupling.dev/) on integration strength, distance, volatility, balance and temporal coupling were fetched with `curl` on 2026-11-15, and every quote below was matched against the page text. The book was not read, so its numeric model is **unverified**.
- **cargo-coupling.** The crate's repository is [nwiizo/cargo-coupling](https://github.com/nwiizo/cargo-coupling), the `repository` field of the crate on crates.io (latest version 0.4.0). It was read at `8ef1441` (2026-10-09), which is 13 commits past the `v0.4.0` tag (`d3fe7b8`) and changes 59 files. Links point to `8ef1441`, so the published 0.4.0 crate may differ in detail. The README, `docs/design-analysis.md` and `src/{analyzer,classification,volatility,manifest}.rs` were read. The tool was not run.
- **repopilot.** `gh search repos repopilot` returns about 20 repositories with that name. Most are AI-agent or repository-insight projects (for example `Shinraya-Tech-Labs/repopilot`, an autonomous fixing agent, and `AZEN-jc/repopilot`, a release gate for public repositories). The one that checks agent edits for broken callers and risky boundary changes is [MykytaStel/repopilot](https://github.com/MykytaStel/repopilot): its description is "Local, deterministic review of Git changes: weakened tests and CI gates, security boundaries, blast radius", its roadmap names release 0.25 "Quiet fallbacks and broken callers", and the `repository` fields of the crates.io and npm packages named `repopilot` (both 0.27.0) point to it. The other repositories were identified from their descriptions only and not read. It was read at `ad0a445` (2026-11-11), one commit past `v0.27.0` (`921d87f`). The README, `docs/` and the review-signal and API-contract sources were read. The tool was not run.
- **easement.** The source at `2c61967` was read and built. Two throwaway probes, one in TypeScript (scip-typescript 0.6.1) and one in Go (scip-go 0.2.9), were checked with `easement check`, `check --diff`, `impact --diff` and `review`. The indexes were kept with `EASEMENT_CACHE_DIR` and printed with `scip snapshot` and `scip print --json` (scip 0.10.0). The probes are not in the repository.
- **Issues.** The bodies of #93, #95, #97, #98, #128 and #153 were searched for suppressions, routes, chains and plans, and `gh issue list --search suppress` was run over all issues. No issue proposes or rejects listing added suppressions or a route per `impact` dependent. Nothing was created, commented on or edited.

## 1. Facts: Khononov's model

From [coupling.dev, Integration Strength](https://coupling.dev/posts/dimensions-of-coupling/integration-strength/), strongest to weakest:

| Level | Definition (verbatim) |
| --- | --- |
| Intrusive | "Intrusive coupling occurs when private interfaces are used for integration." |
| Functional | "Functional coupling occurs when multiple components share knowledge of their functional requirements." Its extreme case is a business rule duplicated in two components. |
| Model | "Model coupling occurs when components share knowledge of a model." |
| Contract | "The goal of an integration contract is to encapsulate all knowledge about implementation details, functional requirements, and business models, thus making integration contract explicit and stable." Examples given: Façade, Open-host service / Published language, Anti-corruption layer, DTOs. |

The other dimensions and the rule:

- **Balance.** [Balance](https://coupling.dev/posts/core-concepts/balance/): "BALANCE = (STRENGTH XOR DISTANCE) OR NOT VOLATILITY". The page uses only high and low values. On volatility it says: "If a component shares a large amount of knowledge but is not expected to change —for example, a legacy system that is no longer evolving—the effects of shared knowledge are balanced by its low volatility."
- **Distance.** [Distance](https://coupling.dev/posts/dimensions-of-coupling/distance/) orders the levels as methods, objects, namespaces/packages, (micro)services, systems, and adds: "Distance is a socio-technical dimension: it is influenced not only by the components' level of abstraction but also by the organizational structure."
- **Volatility.** [Volatility](https://coupling.dev/posts/dimensions-of-coupling/volatility/) "reflects the probability of components needing to change in the first place". It is estimated mainly from DDD subdomains (core, supporting, generic). On history it says: "As software engineers, we are naturally inclined to infer volatility by examining historical commit frequency. However, this method is prone to inaccuracies." Design causes change too: "ineffective management of integration strength and distance can result in accidental volatility."
- **Temporal coupling.** [Temporal coupling](https://coupling.dev/posts/related-topics/temporal-coupling/) is runtime ordering: "Temporal coupling, also known as sequential coupling, occurs when components must be executed in a specific order." It is not co-change in version control.

## 2. Facts: cargo-coupling

**Scope and mechanics.** It covers Rust only and parses with `syn` 3.0 ([Cargo.toml L26](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/Cargo.toml#L26)). Its own limits: "Syntax is not Rust type checking: macro expansion, active `cfg` selection, dynamic dispatch and full re-export resolution remain incomplete" ([design-analysis.md L185-L223](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/docs/design-analysis.md#L185-L223)). The README opens with an "Experimental Project" banner: "The scoring algorithms, thresholds, and detected patterns are subject to change".

**Strength is assigned from syntax.** The usage context decides the level ([analyzer.rs L155-L176](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/analyzer.rs#L155-L176)):

- field access, struct literal or inherent `impl` → Intrusive;
- method call, function call, parameter type or return type → Functional;
- type parameter or `use` → Model;
- trait bound → Contract.

Field access and construction of a `pub` type are lowered to Model ([classification.rs L83-L97](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/classification.rs#L83-L97)). This is a proxy, not Khononov's definition. A method call is "Functional" here, while §1 defines functional coupling as shared knowledge of functional requirements, which no syntax shows. The tool's own blind-spot list says so ("implicit-functional-coupling", [manifest.rs L72-L79](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/manifest.rs#L72-L79)).

**Distance** comes from the module tree: same module, or adjacent in it, 0.25; another subtree of the crate 0.50; an external crate 1.00 ([README L333-L345](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L333-L345)).

**Volatility** comes from Git:

- `git log --pretty=format: --name-only --diff-filter=AMRC --since=<N> months ago -- *.rs` counts the commits that touch each file ([volatility.rs L95-L104](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/volatility.rs#L95-L104)), with 6 months by default.
- 0–2 commits is Low, 3–10 Medium, 11+ High ([L35-L41](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/volatility.rs#L35-L41)).
- `.coupling.toml` can override it per path (`[volatility] high/low`) or classify paths as `[subdomains] core/supporting/generic`. A supporting or generic module that churns is reported as "Accidental Volatility" ([README L276-L306](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L276-L306)).

**Balance score.** `max(|strength − distance|, 1 − volatility)` on values normalized to [0, 1], named `khononov-compensation-v2` ([README L524-L538](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L524-L538)).

**"Hidden Coupling" (co-change).**

- A pair of files counts when it changed together in at least 3 commits ([volatility.rs L159-L264](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/volatility.rs#L159-L264)). Commits that touch more than 50 files are skipped (L211), and the threshold is applied at L239.
- The ratio is the number of shared commits over the commits of the less-changed file. A ratio of 0.5 or more is "strong" (L314).
- It is reported when the two files have no code dependency. The README calls this "Temporal Coupling Detection", which is not what coupling.dev means by temporal coupling (§1).

**Issues and gate.**

- Detected issues include circular dependencies, Global Complexity (strong coupling over a long distance), Cascading Change Risk, Hidden Coupling, God Module, High Efferent and Afferent Coupling, Inappropriate Intimacy and Accidental Volatility ([README L708-L730](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L708-L730)).
- Grades S–F come from issue densities ([L606-L619](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L606-L619)).
- `--check` fails on `--min-grade`, `--max-critical`, `--max-circular` or `--fail-on <severity>`. `--check --baseline <ref>` fails only on new or worsened findings, keyed by `(issue_type, source, target)` ([L83-L96](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L83-L96), [L896-L921](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/README.md#L896-L921)).

**Agent interface and trust.** `--ai` prints text meant to be pasted into an agent, and `--json` prints the same data. A blind-spot manifest declares what was not analysed, for example dynamic connascence, distance across team boundaries, and macros or inactive `cfg` code. It also notes run-specific gaps, such as no Git history or unresolved modules ([manifest.rs L60-L170](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/src/manifest.rs#L60-L170)). `grep -ri mcp src` finds no MCP server. `--changed-since <ref>` lists changed items, multi-hop impact paths and candidate tests, and says the reachability is conservative ([design-analysis.md L1-L36](https://github.com/nwiizo/cargo-coupling/blob/8ef1441a387230ff3a3ec973b83df45a81a287e9/docs/design-analysis.md#L1-L36)).

## 3. Facts: repopilot

**Mechanics.** It parses with tree-sitter (Rust, JavaScript, TypeScript, Python, Go, Java, C#, Kotlin; [Cargo.toml L34-L42](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/Cargo.toml#L34-L42)) and builds a static import graph. No compiler, LSP or SCIP is involved ([architecture.md](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/architecture.md)). A review reads the Git diff (`--base <ref>`, the working tree, or `--since-snapshot`) and gives one decision: `PASS`, `REVIEW`, `BLOCK` or `NOT ASSESSED`. Its signals are sorted into confidence tiers, and `--fail-on-review definitely` makes the top tier a gate ([README L49-L136](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/README.md#L49-L136)).

**"Boundary" means a security boundary, not an architectural one.** A changed file is put in a category by its path and name, or else by an AST pass over imports, decorators, annotations and macros ([signals/mod.rs L1-L13, L46-L59](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/src/review/signals/mod.rs#L1-L59); [classify.rs L22-L55](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/src/review/signals/classify.rs#L22-L55)). The categories are access control, request trust (CORS, CSP, headers), deploy surface (CI, Dockerfiles, IaC), supply chain (manifests and lockfiles), secret config (committed `.env`) and custom globs. Architecture checks are separate rules:

- `architecture.layer-violation` (opt-in, ordered layers by path);
- `architecture.package-boundary-violation` (on by default in a detected workspace; flags imports past a package's `index.ts`, `mod.rs` or `lib.rs`; [configuration.md L257-L290](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/configuration.md#L257-L290));
- per-file `architecture.excessive-fan-out` and `architecture.high-instability-hub`, and `architecture.circular-dependency` ([rules-reference.md L91-L125](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/rules-reference.md#L91-L125)).

**Broken callers.** These checks compare the exporter's code before and after the change, so they need the past side of the diff, not only the current code.

- **Removed export (JS/TS named and default exports; per the docs also a removed Rust `pub fn`, whose code was not read).** For each modified exporter, the tree-sitter facts at the base and at the head are compared to find removed export names. Each importer the graph links to that exporter is then searched for an import of a removed name that still resolves to the exporter. Such an import becomes `behavioral.removed-export-still-imported`, with the caller's line and the exporter ([api_contract/javascript.rs L8-L80](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/src/analysis/api_contract/javascript.rs#L8-L80)).
- **Changed arity (Rust).** The parameter count of each uniquely named `pub fn` is taken before and after. A direct, resolved call that still passes the old count becomes `behavioral.rust-public-function-arity-changed` ([rust_arity.rs L72-L138](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/src/analysis/api_contract/rust_arity.rs#L72-L138)).
- **Removed field (Rust).** A removed or privatized public struct field is reported where a struct literal uses it. This is from the documentation only; `rust_fields.rs` was not read.
- **Limits.** Aliases, re-exports, macros, `cfg`, namespace and dynamic imports are outside what it claims ([language-support.md L37-L60](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/language-support.md#L37-L60), [reports.md L325-L340](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/reports.md#L325-L340)). The 0.25 release notes say no detector "has independently labeled real-repository precision yet".

**Blast radius.** A review lists the direct and transitive dependents of each changed file. Since 0.27 it also gives "a deterministic shortest static import path" from the changed file to each dependent, as `impact_paths.files[].routes` ([reports.md L301-L305](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/reports.md#L301-L305)).

**Checks the change weakened** (`integrity` family): tests skipped, focused or removed; assertions removed or weakened; lint, type or coverage suppressions added; CI gates relaxed; and repopilot's own suppressions added (`integrity.review-suppression-added`, [reports.md L289-L295](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/reports.md#L289-L295)).

**Agent loop.**

- **Snapshot.** `repopilot snapshot` records HEAD and, when the tree is dirty, pins a commit of the uncommitted files. `review --since-snapshot` then covers only what the session changed.
- **Hooks.** The Claude Code and Codex plugins take the snapshot on `SessionStart`. On `Stop` they block the stop once when the session weakened a check or crossed a security signal ([agent-guardrail.md L57-L116](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/agent-guardrail.md#L57-L116)).
- **MCP.** A local stdio server exposes `repopilot_review_change`, `repopilot_scan` and the explain tools ([mcp.md L77-L82](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/mcp.md#L77-L82)).
- **Intent file.** An optional intent file declares the paths and contract families a change is meant to touch. The review reports `intent_drift` as `within-scope` or `drifted` and never suppresses a finding ([configuration.md L175-L201](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/configuration.md#L175-L201), [reports.md L316-L323](https://github.com/MykytaStel/repopilot/blob/ad0a44532991420a1e36cbb02a6e78b6fa7938cd/docs/reports.md#L316-L323)).

## 4. Facts: what easement has, and what its index gives

### Already in easement

| Idea | easement today |
| --- | --- |
| Fan-out and instability | `max_fan_out` and `max_instability` per component, over component edges ([check.rs L808-L866](../../src/check.rs#L808-L866), [configuration.md](../configuration.md#coupling-limits)). There is no fan-in limit and no check that dependencies point toward more stable components. |
| Package-boundary and layer rules | Covered by `public`, named surfaces, `layers`, `must_not_reach` and `isolated` ([configuration.md L91-L101](../configuration.md#public-api)). |
| Cycles | Component cycles with witnesses ([configuration.md](../configuration.md#cycles)). |
| Intrusive vs contract, as policy | A component with `public` or surfaces lets other components import only those paths, and anything past them is a `privacy` violation ([configuration.md L101](../configuration.md#public-api)). `public_symbols` does the same for names. This is Khononov's "private interfaces are used for integration" check, declared by the team rather than inferred. Which components may implement a component's types is the open [#137](https://github.com/mokevnin/easement/issues/137), and per [ADR 0008](../adr/0008-an-implementation-is-not-a-dependency.md) implementations are not dependencies. |
| Baseline ratchet | Keyed by rule, file, target and components; stale entries fail ([adoption.md](../adoption.md#baseline-adopting-on-an-existing-project)). |
| New component edges in a change | `check --diff` lists `stats.new_dependencies`: the component pairs whose every import is on a changed line ([check.rs L793-L805](../../src/check.rs#L793-L805), [commands.rs L139-L140](../../src/commands.rs#L139-L140), [adoption.md](../adoption.md#diff-mode-judging-a-pull-request)). |
| Blast radius | `impact --diff <base>` lists every file that depends on a changed file, directly or through others, grouped by component, with evidence gaps and a `Deleted` gap ([impact.rs L57-L110](../../src/impact.rs#L57-L110)). It gives no route per dependent. `shortest_chain` exists ([chains.rs L361-L420](../../src/chains.rs#L361-L420)) and `why` already prints chains. |
| Policy loosened by the change | `review` lists permissions, surfaces, constraints, the violation delta, moved ownership, examples and debt. Debt covers the baseline, `uncovered_ok`, `exclude`, `target_only` and `warn` ([review.rs L699-L718](../../src/review.rs#L699-L718)). Inline `easement:disable` directives are not in it. |
| Judging an edge before writing it | `propose`, with `--plan` for several edges at once ([commands.md](../commands.md#propose)). |
| Trust | Evidence gaps, `--require-evidence`, cache flags. The kinds of gap include `UnresolvedImports` and `Deleted` ([report.rs L108-L134](../../src/report.rs#L108-L134)). |

### What the index can and cannot supply

- **Visibility and modifiers.** These are not in SCIP. `SymbolInformation.kind` is `UnspecifiedKind` for every symbol in scip-dotnet, scip-php, scip-python and scip-ruby, and no indexer filled `Signature.occurrences` ([gate-or-linter.md §3](gate-or-linter.md#3-facts-what-scip-and-the-indexers-give)). Telling Contract from Model from Functional by the kind or visibility of the target therefore cannot be done the same way in every language. "Is this an interface", and "is this `pub(crate)`" in particular, come only from display text, unevenly.
- **Which names an edge carries.** Every SCIP dependency keeps the lines that refer to its target and the symbol written there: `Dep.mentions[].symbol` ([lang/mod.rs L152-L182, L235-L239](../../src/lang/mod.rs#L152-L182)). A dependency is a name the code writes, at the top level of a module ([ADR 0003](../adr/0003-a-dependency-is-a-written-name.md)), and ADR 0003 chose that definition because it resolves reliably in every SCIP language. The descriptor shape (a top-level name versus a member under a type) is the one signal common to every observed indexer. A re-exported name points at its original in Rust, TypeScript and Python, and aliases are followed since #112 ([symbol-use-dependencies.md §1, §2](symbol-use-dependencies.md#2-facts-members-on-values-of-inferred-type)). Member uses depend on how much each indexer infers (Ruby emits one only where Sorbet knows the type), so a count of members is not comparable across languages. Kotlin and Swift adapters resolve imports only. The registry's `symbols` flag records which languages have verified name uses ([registry.rs L157-L160](../../src/lang/registry.rs#L157-L160)).
- **Distance.** The components are what the team declares. The index has nothing about teams or deployment. Distance in the module tree could be computed from paths, but every edge easement judges already crosses components.
- **Volatility.** Nothing in the index. easement already runs `git diff` and `git ls-files` for `--diff` ([diff.rs L35-L74](../../src/diff.rs#L35-L74)), but never reads the log.
- **Definition ranges.** `reach: symbols` already attributes references to the method or function that holds them ([flow.rs](../../src/flow.rs)). Enclosing ranges are missing on every definition in scip-dotnet and scip-php ([gate-or-linter.md §3](gate-or-linter.md#observed-where-a-reference-stands)), so for those languages any definition-level answer falls back to whole files.

### Probes (throwaway, 2026-11-15)

**TypeScript, scip-typescript 0.6.1.** Component `a` imports `{ foo, bar }` from `src/b/api.ts`. The change deletes `foo` and gives `bar` a new parameter.

- `easement check` passes with no evidence gap, and `check --diff HEAD` passes as well.
- `impact --diff HEAD` lists `src/a/use.ts` as a dependent of `src/b/api.ts`.
- In the index, the import of `foo` becomes `reference local 0`, a local symbol with no definition. `bar` still resolves to `src/b/api.ts/bar().`, since arity is not in the index. No occurrence has `diagnostics`.

**Go, scip-go 0.2.9.** `a.V()` calls `b.Foo(1)`, which does not exist in `b`, and `b.Bar()` with the wrong arity.

- `easement check` passes.
- The index has only the package reference at `b.Foo`, with no occurrence for `Foo`, and `Bar` still resolves to `b/Bar().`.

So a name that does not exist leaves no uniform signal: a dangling local in TypeScript, nothing in Go. The other indexers are **unverified**.

**Deleted file.** When `src/b/api.ts` itself is deleted, `check` reports an `unresolved_imports` evidence gap at `src/a/use.ts:1` (`../b/api`). This is not a violation.

**Added suppression.** The base commit has no `a -> b` import, and the policy forbids it. The change adds both the import and an `// easement:disable-next-line dependency -- …` directive above it.

- `check --diff HEAD` passes. JSON has `stats.suppressed: 1` and `stats.new_dependencies: ["a -> b"]`; text ends "1 suppressed". Neither says where the directive is or which rule it silences.
- `easement review --base HEAD` lists nothing: its debt and delta are empty.
- `easement check --help` has no flag that lists suppressions.

## 5. Recommendations

Each candidate has a bounded change, a verdict on gate or report, and the issue it overlaps. Under AGENTS.md and CONTRIBUTING.md, a new rule, config key or output contract starts as an issue that settles its shape. The earlier notes decided not to build "a general metrics dashboard" ([feature-opportunities.md](feature-opportunities.md#do-not-recommend-as-the-next-increment), [feature-candidates.md](feature-candidates.md#existing-work-and-scope-choices)). Nothing found here changes that: cargo-coupling labels its own scores and grades experimental heuristics (§2), so none of the candidates below is a score or a grade.

### Ranked candidates

1. **List the inline suppressions a change adds.** *Source: repopilot's `integrity.*-suppression-added`.* The agent instructions easement ships say an agent must never add `easement:disable` without asking a human. Yet a diff that adds one shows only as a count (probe above), and `review` does not list it. The bounded change: in `--diff`, list each directive on a changed line, with its rule, line and reason, next to `stats.new_dependencies` in text, JSON and `--format github`. Optionally count added directives under `review` debt. Facts needed: the directive lines easement already parses, and `Changes::contains`. No index facts, same for every language. **Report**; making it fail is a later policy choice. Size: small. Overlap: none open; it extends the debt view of [#98](https://github.com/mokevnin/easement/issues/98) and [#128](https://github.com/mokevnin/easement/issues/128) (closed).
2. **A route for each dependent in `impact`.** *Source: repopilot 0.27 `impact_paths.files[].routes`.* Today `impact` says that a file depends on the change but not through which imports. The bounded change: each `Dependent` gets the shortest chain from a changed file, as `why` chains are typed today, reusing `chains::shortest_chain` over the same file graph. Facts needed: the existing `Dep` file and line. **Report.** Size: small. Overlap: [#95](https://github.com/mokevnin/easement/issues/95) (closed: witness chains in JSON); [feature-candidates.md #7](feature-candidates.md#7-definition-level-impact-and-path-queries).
3. **The callers of the definitions a change touched.** *Source: repopilot's broken callers, without the "broken" claim.* The bounded change:
   - `impact --diff` maps the changed line ranges of each file onto the definition ranges in the head index;
   - it lists the references to those definitions from other components, as caller places (file:line), each with its component;
   - definitions the index gives no range fall back to the whole file, and the report says so as an evidence gap naming the language (C#, PHP, Kotlin, Swift).

   It must not claim a caller is broken: arity and signatures are not in the index (gate-or-linter.md §3), and a removed name leaves no uniform trace (probes). Detecting removals needs the base revision's index. The earlier notes left that two-graph design out: it "requires two graphs and reproducible indexing environments" ([feature-opportunities.md §2](feature-opportunities.md#2-review-what-a-policy-change-actually-legalizes-or-breaks), [feature-feasibility.md #8](feature-feasibility.md#8-definition-aware-why-and-impact)). The build and the type checker are the authority for broken calls in SCIP languages, which require an installed or built project anyway. **Report**, for an agent choosing what to re-check. Size: medium (`Flow` is built only for `reach: symbols` today). Overlap: [feature-candidates.md #7](feature-candidates.md#7-definition-level-impact-and-path-queries); no open issue.
4. **How many names each component edge carries, in `why --format json`.** *Source: cargo-coupling's strength distribution, reduced to what the index supports.* For `a -> b`, report:
   - the distinct top-level names of `b` that `a` writes (from `Dep.mentions[].symbol`), which is exactly what ADR 0003 counts as a dependency, re-exports and aliases resolved to the original;
   - the files and lines that write them;
   - whether they enter through `b`'s public surface.

   `graph` only draws Mermaid or DOT today, so a field on `why` is the smaller change. Call it what it is, the size of the edge. It is not Khononov's integration strength: functional and model coupling cannot be told apart from the index (§1, §4). Kotlin and Swift give modules only, which is an evidence gap. This is a field on existing queries, not a dashboard. A limit rule (for example "at most N names of `b`") needs its own issue and evidence that a team wants it. **Report** now, a gate only after that issue. Size: small to medium. Overlap: none.
5. **Check a change against its plan.** *Source: repopilot's intent drift.* An agent that ran `propose --plan plan.json` could pass the same plan to `check --diff`. The report would list the `new_dependencies` the plan did not name ("drift"), without changing the verdict. Facts needed: the existing plan format and new-edge list. External modules new in the change would be a new fact: `new_dependencies` holds component edges only. **Report.** Size: small to medium. Overlap: [#93](https://github.com/mokevnin/easement/issues/93) (closed, `propose`).

### Spike at most

6. **Volatility.** If it comes, it should be declared, not mined. Khononov ranks subdomain classification first and calls commit frequency "prone to inaccuracies" (§1). cargo-coupling itself overrides Git counts with `[subdomains]` (§2). Before any rule, an issue would have to settle two choices:
   - a per-component declaration (`volatility` or `subdomain`), letting a rule such as "a component declared stable must not depend on a volatile one through its internals" reuse the existing dependency decision;
   - or a churn report from `git log`, which needs full history. A shallow CI clone (`actions/checkout` defaults to depth 1) would be an evidence gap, and any cached result must key on the history window. AGENTS.md says to miss the cache when in doubt.

   Co-change between components with no edge ("hidden coupling") is a statistical hint. It can be a report but never a gate, because it is not a written name (ADR 0003).
7. **Dependencies toward stability.** A component should not depend on one less stable than itself (Martin's principle, not Khononov's). It is computable from the same edges as `max_instability`, the same in every language, and could be a gate. It comes from neither tool examined here, though repopilot's `high-instability-hub` is next to it. It needs its own issue if wanted.

### Not to adopt

- **Integration-strength levels inferred from the index.** Visibility, modifiers and signatures are text or absent, unevenly by indexer (§4). Functional coupling is not syntactic even in cargo-coupling (§2). The intrusive-vs-contract half is already a policy (`public`, surfaces, `public_symbols` → `privacy`/`symbol`). The implementation half is [#137](https://github.com/mokevnin/easement/issues/137).
- **Grades and balance scores as a gate.** These are heuristic thresholds, labelled experimental by their author (§2).
- **Security-boundary classification of changed files** (repopilot's `boundary.*`). It classifies paths by keyword, not where a written name lands, so it is outside what easement judges ([.out-of-scope/declaration-style-rules.md](../../.out-of-scope/declaration-style-rules.md) states that test).
- **Test-weakening, arity and removed-export detectors.** They belong to a review tool, a linter or the compiler. For easement, a removed definition needs a second index (candidate 3).
- **Session snapshots.** `--diff HEAD` already covers uncommitted and untracked work. Only a tree that was already dirty when the session began is not separated, and committing or a worktree handles that.
- **Adjacent to the product, not ranked.** repopilot's Stop hook, which blocks an agent from finishing on a failing check, is distribution rather than a library feature. easement's Claude Code plugin ships only the skill ([agent-setup.md](../agent-setup.md#agent-skill)). A `Stop` hook that runs `easement check` would be the analogue.
