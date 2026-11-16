# Volatility of a component

easement has no notion of how often a component is expected to change: no `volatility` or `subdomain` key on a component, and no churn measured from `git log`.

## Why this is out of scope

In Khononov's Balanced Coupling model, depending on the internals of a volatile component is the edge that hurts. What a gate can judge from that is where the dependency enters the volatile component, and easement already judges it. Declaring `public` (or named surfaces) on the component makes every import from outside the surface fail as `privacy`:

```yaml
components:
  pricing:
    in: src/pricing
    public: src/pricing/api
    because: pricing rules change every quarter; depend on the API only
```

The one rule proposed for a `volatility` key, "a volatile component may be depended on only through its public surface", is this configuration under another name. A key that changes no judgement is a label, and the reason a component is closed belongs in its `because`, which every violation already prints.

Measuring volatility from history does not fit either:

- It is not a written name. easement's facts are the names code writes (ADR 0003). Commit frequency and co-change are statistics about the past, and they never show up as an import.
- Khononov himself calls commit frequency an inaccurate way to estimate volatility, and estimates it from subdomains instead ([coupling.dev, Volatility](https://coupling.dev/posts/dimensions-of-coupling/volatility/)).
- In CI it depends on the clone. `actions/checkout` fetches one commit by default, so a shallow clone would read as "stable". The cache key would also have to include the history window.

## What is still open

"Depend in the direction of stability" (Martin's principle, judged on the same component edges as `max_instability`) is a different rule. It needs its own issue under the one-rule-for-every-language shape if a team wants it.

## Prior requests

- #195: "Volatility: declare how often a component is expected to change, or measure it from git". Research: `docs/research/coupling-and-agent-edit-checks.md` (§1, §2, §5 item 6).
