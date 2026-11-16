//! Policy examples: proposed dependencies, and uses of names, that the policy
//! keeps with the decision each must get. Every `check` judges a dependency as
//! `propose` judges a proposal, through [`crate::decision::decide`], and a use
//! of a name as the scan judges one, through [`crate::symbols::judge`], so a
//! change to the policy that flips one fails as `example` at its line although
//! no code changed.

use std::path::Path;

use crate::check::{RuleId, Violation};
use crate::components::ComponentMap;
use crate::config::{Config, Example, Expect};
use crate::decision::{self, Outside};
use crate::policy::Policy;
use crate::propose::{self, Judged, Kind, Proposal, To};
use crate::symbols;

/// One violation for each example of the policy that does not get the
/// decision it expects, at its line in the policy input that writes it.
pub fn check(policy: &Policy, map: &ComponentMap, root: &Path) -> Vec<Violation> {
    let examples = &policy.config.examples;
    let proposals: Vec<Proposal> = examples
        .iter()
        .filter(|example| example.symbol.is_none())
        .map(proposal)
        .collect();
    let mut dependencies = propose::judge(&policy.config, map, &proposals).into_iter();
    examples
        .iter()
        .enumerate()
        .filter_map(|(i, example)| {
            let (judged, why) = match &example.symbol {
                Some(symbol) => {
                    let judged = used(&policy.config, map, example, symbol);
                    let why = unjudged(&judged).or_else(|| undefined(map, example));
                    (judged, why)
                }
                None => {
                    let judged = dependencies
                        .next()
                        .expect("a decision for each example of a dependency");
                    let why = unjudged(&judged);
                    (judged, why)
                }
            };
            let problem = problem(example, &judged, why)?;
            let (file, line) = policy.example_at(i).map_or_else(
                || (String::new(), 1),
                |(input, line)| (crate::diff::input_path(input, root), line),
            );
            Some(Violation {
                file,
                line,
                message: format!("example {}: {problem}", label(example)),
                because: example.because.clone(),
                from: judged.from.clone(),
                to: example.symbol.clone().unwrap_or_else(|| judged.target()),
                target: expected(example),
                ..Violation::of(RuleId::Example)
            })
        })
        .collect()
}

/// The example as reports name it: `web/a.go -> db/b.go`, or for a use of a name
/// `web/a.go uses billing.Charge defined in billing/charge.go`.
pub fn label(example: &Example) -> String {
    let Some(symbol) = &example.symbol else {
        return proposal(example).label();
    };
    let defined = example
        .defined_in
        .as_ref()
        .map(|path| format!(" defined in {path}"))
        .unwrap_or_default();
    format!("{} uses {symbol}{defined}", example.from)
}

/// The decision of the symbol rules on the use of `symbol` that the example
/// proposes, taken from its paths alone, as [`symbols::judge`] takes the scan's.
/// A source no component owns is judged by the top-level `forbid_symbol` alone,
/// and is otherwise `unowned`, as for a dependency.
fn used(config: &Config, map: &ComponentMap, example: &Example, symbol: &str) -> Judged {
    let from = example.from.as_str();
    let undecided = |decision| Judged {
        from: from.to_string(),
        to: None,
        external: None,
        component: None,
        target_component: None,
        decision,
        grant: None,
        rule: None,
        message: None,
        because: None,
        public_surface: Vec::new(),
        named_surfaces: Vec::new(),
        warning: false,
    };
    match decision::unread(map, from) {
        Some(Outside::Excluded) => return undecided(Kind::Excluded),
        Some(Outside::TargetOnly) => return undecided(Kind::TargetOnly),
        _ => {}
    }
    let owner = map.component_of(from);
    let defined_by = example
        .defined_in
        .as_deref()
        .filter(|path| decision::unread(map, path) != Some(Outside::Excluded))
        .and_then(|path| map.component_of(path));
    let Some(use_) = symbols::Use::written(from, owner, defined_by, symbol) else {
        // Loading refuses a name with no qualifier.
        return undecided(Kind::Unowned);
    };
    let judged = symbols::judge(config, map, &use_);
    let component = owner.map(str::to_string);
    let target_component = defined_by.map(str::to_string);
    match judged.found.into_iter().next() {
        Some(found) => Judged {
            component,
            target_component,
            rule: Some(RuleId::Symbol),
            message: Some(found.message),
            because: found.because,
            warning: config.warns(RuleId::Symbol),
            ..undecided(Kind::Rejected)
        },
        None if owner.is_none() => undecided(Kind::Unowned),
        None => Judged {
            component,
            target_component,
            grant: judged.granted,
            ..undecided(Kind::Allowed)
        },
    }
}

/// Why the `defined_in` of the example says nothing of `public_symbols`: no
/// component owns it, `exclude` hiding it among the reasons.
fn undefined(map: &ComponentMap, example: &Example) -> Option<String> {
    let path = example.defined_in.as_deref()?;
    if decision::unread(map, path) == Some(Outside::Excluded) {
        return Some(format!("`exclude` hides {path}"));
    }
    if map.component_of(path).is_none() {
        return Some(format!("{path} belongs to no component"));
    }
    None
}

pub fn proposal(example: &Example) -> Proposal {
    Proposal {
        from: example.from.clone(),
        to: match (&example.to, &example.external) {
            (Some(path), _) => To::Path(path.clone()),
            (None, module) => To::External(module.clone().unwrap_or_default()),
        },
    }
}

/// What the example expects, as a reader says it: `rejected [privacy]`.
pub fn expected(example: &Example) -> String {
    match example.rule {
        Some(rule) => format!("{} [{}]", example.expect.as_str(), rule.id().as_str()),
        None => example.expect.as_str().to_string(),
    }
}

/// What is wrong with the decision on the example, if anything; `why` it
/// cannot be judged, when it cannot.
fn problem(example: &Example, judged: &Judged, why: Option<String>) -> Option<String> {
    if let Some(why) = why {
        return Some(format!(
            "cannot be judged: {why}; point it at a path a component owns, or drop it"
        ));
    }
    let got = match (judged.decision, judged.rule) {
        (Kind::Rejected, Some(rule)) => Some(rule),
        _ => None,
    };
    let holds = match (example.expect, got) {
        (Expect::Allowed, None) => true,
        (Expect::Rejected, Some(rule)) => example.rule.is_none_or(|r| r.id() == rule),
        _ => false,
    };
    if holds {
        return None;
    }
    let decision = match (got, &judged.message) {
        (Some(_), Some(message)) => format!("{}: {message}", judged.verdict()),
        _ => judged.verdict(),
    };
    Some(format!(
        "expects {}, but the policy now decides {decision}; restore what the policy meant, or update the example if the change is meant",
        expected(example)
    ))
}

/// Why no decision on the example says anything of the policy: a source or a
/// target path that no component owns, `exclude` hiding it among the reasons.
fn unjudged(judged: &Judged) -> Option<String> {
    match judged.decision {
        Kind::Excluded => return Some(format!("`exclude` hides {}", judged.from)),
        Kind::TargetOnly => return Some(format!("`target_only` names {}", judged.from)),
        Kind::Unowned => return Some(format!("{} belongs to no component", judged.from)),
        Kind::Allowed | Kind::Rejected => {}
    }
    let to = judged.to.as_ref()?;
    if judged.target_component.is_some() {
        return None;
    }
    Some(if judged.rule == Some(RuleId::Excluded) {
        format!("`exclude` hides {to}")
    } else {
        format!("{to} belongs to no component")
    })
}
