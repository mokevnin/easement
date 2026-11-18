---
status: accepted
---

# A narrowed verdict and an evidence gap are reported separately

A verdict can fall short of the whole project for two unrelated reasons. One is that the caller asked for a part (`--diff`, `--component`, `--scope`): a narrowed verdict. The other is that the check could not read or judge some code: an evidence gap. One `confidence: partial` value plus a flat list of prose reasons hid which of the two applied, so an agent could neither gate on missing evidence without rejecting every deliberate diff run, nor see accepted gaps (`uncovered_ok`), which were dropped from the output. So reports carry narrowing, evidence gaps and accepted gaps as separate typed lists. Cache provenance is a fourth, independent note: a cache hit is not a gap. `confidence` and `confidence_reasons`, kept a while for agents that read them, are gone: the three lists are the one place that says what a verdict leaves out.

Every output format keeps the three apart, not only `--format json`: each note a run makes of itself has one kind (`narrowed`, `evidence-gap`, `accepted-gap`, beside `cache`), which the GitHub annotation title, the SARIF notification descriptor and the text and JUnit prefix all name, and the word "partial" names none of them. An accepted gap settles a verdict, not an answer about the dependency graph (`impact`, `why`, `suggest`, `graph`): nobody saw the dependencies of an accepted file, so such an answer lists it both as an evidence gap and as an accepted gap.

## Considered options

- **Keep one `confidence` value and add a gate flag that filters its reasons.** Rejected because it treats the symptom: the gate would have to re-parse prose, and every other consumer would still be unable to tell the two cases apart.
- **Keep `confidence` beside the new lists.** First chosen, since agents read it; dropped in a breaking release, as two answers to one question invite reading the vaguer one.
- **Leave an accepted file out of the evidence gaps of a graph answer, as `check` does.** Rejected because the dependencies in it are missing from the graph whatever the policy accepts: an `impact` or a drawing without them would look complete.
