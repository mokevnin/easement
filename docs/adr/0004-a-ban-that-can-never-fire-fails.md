---
status: accepted
---

# A `forbid_symbol` name that can never fire fails as `unmatched`; one never seen stays silent

[#56](https://github.com/mokevnin/easement/issues/56) asked that a `forbid_symbol` name matching nothing fail as `unmatched`, the way an `in` pattern that matches no file does, so that a boundary cannot vanish while the check stays green. Two cases look alike there and are not. A name the index has never seen (`Shop::Db.deploy`) is what a ban on code that does not exist yet looks like, and banning before use is what the rule is for. A name that misses while one of the same name sits deeper under its qualifier (`I18n.locale`, defined as `I18n::Base.locale`) is a misspelled ban that can never fire. The second fails as `unmatched` at its line in the policy, with the name it probably means; the first stays silent. Only a whole index can tell either: with files no indexer covered, nothing is said.

## Considered options

- **Fail on every name that matches nothing**, as #56 first asked. Rejected: it would forbid writing a ban before the code it bans, the main use of the rule.
- **Report a ban that can never fire as a note, not a failure**, as easement did until this decision. Rejected: a note leaves the run green over a boundary that does not hold, which is what `unmatched` exists to prevent for `in`, `part` and `entry`.

## Consequences

- `stats.symbols_unmatched` is gone from `--format json`; the violation carries the message instead.
- Like every `unmatched`, it is about the policy: `--diff` keeps it, the baseline never accepts it, and it cannot be suppressed.
