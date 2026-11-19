---
status: accepted
---

# Diff mode attributes the policy delta by default

`--diff` promises to judge a pull request. A change to the policy is part of the pull request, so a violation also belongs to the change when it is in the policy delta: it fails under the head policy and not under the base policy, over the same current graph. This is the default, not a flag, because an opt-in would leave the hole open for everyone who does not know about it. The price is a full judgment, with the reason stated, when the two policies differ in what gets scanned, since the code scanned now cannot stand in for the code the base policy would have scanned, or when the base policy cannot be known (ADR-0006). A changed manifest is no policy input, but the rules that read a manifest can be wrong about an unchanged line: `manifests` and `packages` read it whole, not a line of it, and `unused` takes entries from it (what a package runs or exports, the files a framework it lists loads), so a manifest edit can leave a file unreachable without touching it. When one changed, those rules are judged in full and every other rule still by the change. Teams that rely only on `--diff` will then see old violations that the baseline does not cover, for what was judged in full only.

## Considered options

- **A separate flag or command for policy changes.** Rejected: it treats the symptom, which is the README's advice to "run a full check for rule changes", by hand.
- **Always judge in full when a policy input changed.** Rejected: unrelated old debt would surface on every policy edit, although the delta can say exactly what the edit caused.
- **Judge every rule in full when a manifest changed**, as this ADR first said. Rejected for the same reason: only the rules that read a manifest can be wrong about an unchanged line, and the others would surface unrelated old debt on every dependency bump.

## Consequences

- A diff run that changes a manifest with `entry` set reports every unused file, old ones too, and `diff.in_full` names `unused` with the manifest, whether or not `manifests` and `packages` are on.
- A file left dead by removing its last import is still not reported: no changed line points at it and no manifest changed, so the configuration docs tell to catch it with a full check.
