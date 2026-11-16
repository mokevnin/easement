//! The `implementation` rule: which components may hold a type that implements
//! or extends a type of a component (`allow_implementers`), read from the
//! relations the indexes declare and never from dependencies, and the evidence
//! gaps of the languages whose indexes declare none.

use std::fs;

use easement::GapKind;
use easement::commands::{self, Checked, Outcome};

use crate::common::{fixture, on_feature, project};
use crate::diff::diff_main;
use crate::run::{
    by_rule, cache, cannot_index_here, check, check_with, config, json, parse, pinned, text,
};

/// The `implementation` violations of a report as `file:line message`.
fn implementations(report: &serde_json::Value) -> Vec<String> {
    let mut found: Vec<String> = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["rule"] == "implementation")
        .map(|v| {
            format!(
                "{}:{} {}",
                v["file"].as_str().unwrap(),
                v["line"],
                v["message"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found
}

/// Only a type in a component `allow_implementers` does not list fails, at its
/// definition: one that implements the port, one that extends a class that does,
/// and an interface that extends it. The port's own component and `tests` pass.
#[test]
fn java_implementations_outside_the_listed_components_fail_at_their_definition() {
    if cannot_index_here(&["java"]) {
        return;
    }
    let (_tmp, root) = fixture("implementations_java");
    let checked = check(&root);
    let report = json(&checked);
    let java = "src/main/java/com/acme";
    let may = "ports may be implemented only in adapters, tests";
    assert_eq!(
        implementations(&report),
        [
            format!(
                "{java}/cache/CachedRepo.java:5 cache -> ports: CachedRepo implements ports' OrderRepository; {may}"
            ),
            format!(
                "{java}/cache/ReadOnly.java:5 cache -> ports: ReadOnly implements ports' OrderRepository; {may}"
            ),
            format!(
                "{java}/sql/SqlRepo.java:5 sql -> ports: SqlRepo implements ports' OrderRepository; {may}"
            ),
        ],
        "{report}"
    );
    // An implementation is no dependency: `sql` writes only `adapters`, and
    // nothing else fails.
    let others: Vec<_> = by_rule(&report)
        .into_iter()
        .filter(|(_, _, rule)| rule != "implementation")
        .collect();
    assert!(others.is_empty(), "{report}");
    let said = text(&checked);
    assert!(
        said.contains("because: ports are implemented only by adapters and test doubles"),
        "{said}"
    );
    // An anonymous class has no type in the index: a gap, at its method.
    assert_eq!(
        implementation_gaps(&checked),
        [(
            "`implementation` cannot judge 1 java declarations that write a name of ports outside the components that may implement it: scip-java gives an anonymous class no type, only its methods relations to the methods they implement, so whether they implement or extend a type of ports was not checked".to_string(),
            vec![format!("{java}/cache/Anonymous.java")],
        )]
    );
    pinned(|| insta::assert_snapshot!("check_implementations_text", said));
}

/// The violations of `implementation` in the fixture `name`, and whether any
/// other rule failed there.
fn implementations_in(name: &str) -> (Vec<String>, Vec<(String, u64, String)>) {
    let (_tmp, root) = fixture(name);
    let report = json(&check(&root));
    let others = by_rule(&report)
        .into_iter()
        .filter(|(_, _, rule)| rule != "implementation")
        .collect();
    (implementations(&report), others)
}

/// The message of an implementation of the port outside `adapters` and `tests`.
fn outside(from: &str, sub: &str, port: &str) -> String {
    format!(
        "{from} -> ports: {sub} implements ports' {port}; ports may be implemented only in adapters, tests"
    )
}

/// C# as Java: an interface that extends the port fails too.
#[test]
fn csharp_implementations_outside_the_listed_components_fail_at_their_definition() {
    let port = "IOrderRepository";
    let (found, others) = implementations_in("implementations_csharp");
    assert_eq!(
        found,
        [
            format!(
                "Cache/CachedRepo.cs:10 {}",
                outside("cache", "IReadOnly", port)
            ),
            format!(
                "Cache/CachedRepo.cs:5 {}",
                outside("cache", "CachedRepo", port)
            ),
            format!("Sql/SqlRepo.cs:5 {}", outside("sql", "SqlRepo", port)),
        ]
    );
    assert!(others.is_empty(), "{others:?}");
}

/// TypeScript classes, whose relations scip-typescript gives through their
/// heritage clauses.
#[test]
fn typescript_classes_outside_the_listed_components_fail_at_their_definition() {
    let port = "OrderRepository";
    let (found, others) = implementations_in("implementations_typescript");
    assert_eq!(
        found,
        [
            format!(
                "src/cache/cached-repo.ts:3 {}",
                outside("cache", "CachedRepo", port)
            ),
            // A class inside a function, which the index names locally.
            format!("src/cache/factory.ts:4 {}", outside("cache", "Inner", port)),
            format!("src/sql/sql-repo.ts:3 {}", outside("sql", "SqlRepo", port)),
        ]
    );
    assert!(others.is_empty(), "{others:?}");
}

/// Python, whose index gives the direct base classes alone: easement closes them,
/// so a class that extends a base of `adapters` fails too.
#[test]
fn python_classes_outside_the_listed_components_fail_through_their_bases() {
    if cannot_index_here(&["python"]) {
        return;
    }
    let port = "OrderRepository";
    let (found, others) = implementations_in("implementations_python");
    assert_eq!(
        found,
        [
            format!(
                "src/acme/cache/cached.py:4 {}",
                outside("cache", "CachedRepo", port)
            ),
            format!("src/acme/sql/repo.py:4 {}", outside("sql", "SqlRepo", port)),
        ]
    );
    assert!(others.is_empty(), "{others:?}");
}

/// `[]` leaves only the component itself, as the same-component grant says it:
/// another instance of an isolated component is not itself.
#[test]
fn an_empty_list_lets_only_the_same_instance_implement() {
    let port = "export interface Port {\n  run(): void;\n}\n";
    let implementing = |from: &str, name: &str| {
        format!(
            "import {{ Port }} from \"{from}\";\n\nexport class {name} implements Port {{\n  run() {{}}\n}}\n"
        )
    };
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  ports: { in: 'src/ports/*', isolated: true }\n  adapters: { in: src/adapters }\nrules:\n  ports: { allow_implementers: [] }\n  adapters: { may_depend_on: [ports] }\n",
        ),
        ("src/ports/a/port.ts", port),
        ("src/ports/a/mem.ts", &implementing("./port", "Mem")),
        ("src/ports/b/other.ts", &implementing("../a/port", "Other")),
        ("src/adapters/pg.ts", &implementing("../ports/a/port", "Pg")),
    ]);
    let report = json(&check(&root));
    assert_eq!(
        implementations(&report),
        [
            "src/adapters/pg.ts:3 adapters -> ports: Pg implements ports' Port; ports may be implemented only in ports",
            "src/ports/b/other.ts:3 ports -> ports: Other implements ports' Port; ports may be implemented only in ports",
        ],
        "{report}"
    );
}

/// The evidence gaps of `implementation` in a check, as their messages and files.
fn implementation_gaps(checked: &Checked) -> Vec<(String, Vec<String>)> {
    checked
        .coverage
        .evidence_gaps
        .iter()
        .filter(|gap| gap.kind == GapKind::Implementation)
        .map(|gap| (gap.message.clone(), gap.files.clone()))
        .collect()
}

/// The policy of the gap projects: `ports` may be implemented by `adapters`.
const GAP_POLICY: &str = "version: 1\ncomponents:\n  ports: { in: ports }\n  adapters: { in: adapters }\n  cache: { in: cache }\n  other: { in: other, optional: true }\nrules:\n  ports: { allow_implementers: [adapters] }\n  adapters: { may_depend_on: [ports] }\n  cache: { may_depend_on: [ports, adapters] }\n";

/// What the index cannot tell is no clean verdict: a Go type implements an
/// interface by its methods, which declares nothing, so every Go file outside
/// the allowed components is a gap once `ports` defines Go types, whether it
/// names `ports` or not; and `--require-evidence` fails on it.
#[test]
fn go_types_outside_the_allowed_components_are_an_evidence_gap() {
    let (_tmp, root) = project(&[
        ("easement.yml", GAP_POLICY),
        ("go.mod", "module ex.com/m\n\ngo 1.22\n"),
        (
            "ports/port.go",
            "package ports\n\ntype Port interface {\n\tRun()\n}\n",
        ),
        (
            "adapters/pg.go",
            "package adapters\n\ntype Pg struct{}\n\nfunc (Pg) Run() {}\n",
        ),
        (
            "cache/c.go",
            "package cache\n\ntype Cached struct{}\n\nfunc (Cached) Run() {}\n",
        ),
        ("other/o.go", "package other\n\ntype Other struct{}\n"),
    ]);
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    assert_eq!(
        implementation_gaps(&checked),
        [(
            "`implementation` cannot judge 2 go files outside the components that may implement ports: a go type implements an interface by having its methods, which declares nothing, so whether a type in them implements a type of ports was not checked".to_string(),
            vec!["cache/c.go".to_string(), "other/o.go".to_string()],
        )]
    );
    assert!(implementations(&json(&checked)).is_empty());
}

/// A language whose index declares no implementations: implementing needs the
/// supertype's name there, so a file that depends on `ports` outside the
/// allowed components is a gap, and one that does not is none.
#[test]
fn rust_files_that_depend_on_the_component_are_an_evidence_gap() {
    let (_tmp, root) = project(&[
        ("easement.yml", GAP_POLICY),
        (
            "Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"lib.rs\"\n",
        ),
        (
            "lib.rs",
            "pub mod adapters;\npub mod cache;\npub mod other;\npub mod ports;\n",
        ),
        ("ports/mod.rs", "pub trait Port {\n    fn run(&self);\n}\n"),
        (
            "adapters/mod.rs",
            "use crate::ports::Port;\n\npub struct Pg;\n\nimpl Port for Pg {\n    fn run(&self) {}\n}\n",
        ),
        (
            "cache/mod.rs",
            "use crate::ports::Port;\n\npub struct Cached;\n\nimpl Port for Cached {\n    fn run(&self) {}\n}\n",
        ),
        ("other/mod.rs", "pub struct Other;\n"),
    ]);
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
    assert_eq!(
        implementation_gaps(&checked),
        [(
            "`implementation` cannot judge 1 rust files that depend on ports outside the components that may implement it: the rust index declares no implementations, so whether a type in them implements a type of ports was not checked".to_string(),
            vec!["cache/mod.rs".to_string()],
        )]
    );
}

/// A part of a file is judged as the component it belongs to: a Rust test module
/// in `adapters` is `tests`, which may not implement `ports`.
#[test]
fn a_rust_test_part_of_an_allowed_file_is_an_evidence_gap() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  ports: { in: ports }\n  adapters: { in: adapters }\nrules:\n  ports: { allow_implementers: [adapters] }\n  adapters: { may_depend_on: [ports] }\n  tests: { may_depend_on: [ports, adapters] }\ntests: auto\n",
        ),
        (
            "Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"lib.rs\"\n",
        ),
        ("lib.rs", "pub mod adapters;\npub mod ports;\n"),
        ("ports/mod.rs", "pub trait Port {\n    fn run(&self);\n}\n"),
        (
            "adapters/mod.rs",
            "pub struct Pg;\n\nimpl crate::ports::Port for Pg {\n    fn run(&self) {}\n}\n\n#[cfg(test)]\nmod tests {\n    use crate::ports::Port;\n\n    struct Fake;\n\n    impl Port for Fake {\n        fn run(&self) {}\n    }\n}\n",
        ),
    ]);
    let checked = check(&root);
    assert_eq!(
        implementation_gaps(&checked),
        [(
            "`implementation` cannot judge 1 rust files that depend on ports outside the components that may implement it: the rust index declares no implementations, so whether a type in them implements a type of ports was not checked".to_string(),
            vec!["adapters/mod.rs".to_string()],
        )],
        "{}",
        text(&checked)
    );
}

/// scip-typescript gives an interface declaration, and a class that implements a
/// type alias, no relations: one that writes a name of `ports`, directly or
/// through an alias, outside the allowed components is a gap at its line.
#[test]
fn a_typescript_interface_that_writes_the_component_is_an_evidence_gap() {
    let (_tmp, root) = project(&[
        ("easement.yml", GAP_POLICY),
        (
            "ports/port.ts",
            "export interface Port {\n  run(): void;\n}\n\nexport type PortAlias = Port;\n",
        ),
        (
            "cache/read-only.ts",
            "import { Port } from \"../ports/port\";\n\nexport interface Unrelated {\n  size: number;\n}\n\nexport interface ReadOnly extends Port {\n  size(): number;\n}\n",
        ),
        (
            "adapters/pg.ts",
            "import { Port } from \"../ports/port\";\n\nexport interface PgPort extends Port {}\n\nexport type Wrapped = Port;\n",
        ),
        // `implements` of a type alias, of `ports` or of another component that
        // stands for a type of `ports`.
        (
            "cache/alias.ts",
            "import { PortAlias } from \"../ports/port\";\nimport { Wrapped } from \"../adapters/pg\";\n\nexport class ViaAlias implements PortAlias {\n  run() {}\n}\n\nexport class ViaWrapped implements Wrapped {\n  run() {}\n}\n",
        ),
    ]);
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(
        checked.outcome(),
        Outcome::EvidenceRequired,
        "{}",
        text(&checked)
    );
    let gap = checked
        .coverage
        .evidence_gaps
        .iter()
        .find(|gap| gap.kind == GapKind::Implementation)
        .expect("an implementation gap");
    assert_eq!(
        gap.message,
        "`implementation` cannot judge 3 typescript declarations that write a name of ports outside the components that may implement it: scip-typescript gives an interface declaration, a class expression or a class that implements a type alias no relations, so whether they implement or extend a type of ports was not checked",
        "{gap:?}"
    );
    pinned(|| insta::assert_json_snapshot!("implementation_gap", gap));
    assert_eq!(gap.files, ["cache/alias.ts", "cache/read-only.ts"]);
    assert_eq!(
        gap.places
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "cache/alias.ts:4",
            "cache/alias.ts:8",
            "cache/read-only.ts:7"
        ]
    );
}

/// `--diff` keeps an implementation when a definition on its way to the
/// supertype changed: a base class that comes to implement the port fails its
/// subclass, whose own file the change does not touch. The baseline records the
/// violation and an inline comment suppresses it, as for any rule.
#[test]
fn diff_baseline_and_suppression_hold_implementations() {
    let policy = "version: 1\ncomponents:\n  ports: { in: ports }\n  adapters: { in: adapters }\n  sql: { in: sql }\nrules:\n  ports: { allow_implementers: [adapters] }\n  adapters: { may_depend_on: [ports] }\n  sql: { may_depend_on: [adapters] }\n";
    let sql = "import { Base } from \"../adapters/base\";\n\nexport class SqlRepo extends Base {\n  run() {}\n}\n";
    let (_tmp, root) = project(&[
        ("easement.yml", policy),
        (
            "ports/port.ts",
            "export interface Port {\n  run(): void;\n}\n",
        ),
        ("adapters/base.ts", "export abstract class Base {}\n"),
        ("sql/sql.ts", sql),
    ]);
    on_feature(&root);
    assert_eq!(check(&root).outcome(), Outcome::Passed);
    fs::write(
        root.join("adapters/base.ts"),
        "import { Port } from \"../ports/port\";\n\nexport abstract class Base implements Port {\n  abstract run(): void;\n}\n",
    )
    .unwrap();
    let checked = check_with(&root, diff_main);
    assert_eq!(checked.outcome(), Outcome::Failed, "{}", text(&checked));
    let report = json(&checked);
    assert_eq!(
        implementations(&report),
        [
            "sql/sql.ts:3 sql -> ports: SqlRepo implements ports' Port; ports may be implemented only in adapters"
        ],
        "{report}"
    );
    let violation = &report["violations"][0];
    assert_eq!(violation["target"], "SqlRepo implements Port", "{report}");
    assert_eq!(
        violation["provenance"],
        serde_json::json!({ "field": "rules.ports.allow_implementers", "input": "easement.yml" })
    );
    assert_eq!(
        violation["evidence"],
        serde_json::json!([
            { "file": "sql/sql.ts", "line": 3 },
            { "file": "adapters/base.ts", "line": 3 },
            { "file": "ports/port.ts", "line": 1 },
        ])
    );

    // An inline comment at the definition suppresses it.
    fs::write(
        root.join("sql/sql.ts"),
        sql.replace(
            "export class",
            "// easement:disable-next-line implementation -- moves to adapters with #1\nexport class",
        ),
    )
    .unwrap();
    assert_eq!(check(&root).outcome(), Outcome::Passed);

    // The baseline records it as debt.
    fs::write(root.join("sql/sql.ts"), sql).unwrap();
    assert_eq!(check(&root).outcome(), Outcome::Failed);
    let recorded = commands::baseline(&config(&root), true, &mut cache()).unwrap();
    assert!(
        fs::read_to_string(root.join("easement.baseline.yml"))
            .unwrap()
            .contains("rule: implementation"),
        "{}",
        recorded.text()
    );
    assert_eq!(check(&root).outcome(), Outcome::Passed);
}

/// A component added to `allow_implementers` loosens the rule, and the key set
/// tightens it; `explain` of a file shows who may implement its component.
#[test]
fn review_and_explain_show_allow_implementers() {
    let policy = "version: 1\ncomponents:\n  ports: { in: src/ports }\n  adapters: { in: src/adapters }\n  tests: { in: src/tests }\nrules:\n  ports: { allow_implementers: [adapters], because: adapters implement ports }\n";
    let (_tmp, root) = project(&[
        ("easement.yml", policy),
        ("src/ports/port.ts", "export interface Port {}\n"),
        ("src/adapters/pg.ts", "export const pg = 1;\n"),
        ("src/tests/fake.ts", "export const fake = 1;\n"),
    ]);
    on_feature(&root);
    fs::write(
        config(&root),
        policy
            .replace("[adapters], because", "[adapters, tests], because")
            .replace(
                "rules:\n",
                "rules:\n  adapters: { allow_implementers: [] }\n",
            ),
    )
    .unwrap();
    let reviewed = commands::review(&config(&root), "main", &mut cache()).unwrap();
    let report = parse(&reviewed.json());
    assert_eq!(
        report["constraints"],
        serde_json::json!([
            {
                "component": "adapters",
                "key": "allow_implementers",
                "item": null,
                "before": null,
                "after": [],
                "change": "tightened",
                "provenance": { "field": "rules.adapters.allow_implementers", "input": "easement.yml" },
            },
            {
                "component": "ports",
                "key": "allow_implementers",
                "item": "tests",
                "edit": "added",
                "change": "loosened",
                "because": "adapters implement ports",
                "provenance": { "field": "rules.ports.allow_implementers[1]", "input": "easement.yml" },
            },
        ]),
        "{report}"
    );
    let said = reviewed.text();
    assert!(
        said.contains("  ports.allow_implementers: added `tests` (loosened)\n"),
        "{said}"
    );
    let explained = commands::explain(&config(&root), &root.join("src/ports/port.ts"))
        .unwrap()
        .text()
        .unwrap();
    assert!(
        explained.contains("allow_implementers: adapters, tests (and itself)"),
        "{explained}"
    );
}

/// An implementation never enters the graph: `c` holds a subclass of a base of
/// `b` that implements a port of `a`, and writes only `b`, so `c` needs no
/// grant to `a` and keeps a fan-out of one; only the implementation fails.
#[test]
fn an_implementation_is_no_dependency() {
    let (_tmp, root) = project(&[
        (
            "easement.yml",
            "version: 1\ncomponents:\n  a: { in: a }\n  b: { in: b }\n  c: { in: c }\nrules:\n  a: { allow_implementers: [b] }\n  b: { may_depend_on: [a] }\n  c: { may_depend_on: [b], max_fan_out: 1 }\n",
        ),
        ("a/port.ts", "export interface Port {\n  run(): void;\n}\n"),
        (
            "b/base.ts",
            "import { Port } from \"../a/port\";\n\nexport abstract class Base implements Port {\n  abstract run(): void;\n}\n",
        ),
        (
            "c/sub.ts",
            "import { Base } from \"../b/base\";\n\nexport class Sub extends Base {\n  run() {}\n}\n",
        ),
    ]);
    let report = json(&check(&root));
    assert_eq!(
        by_rule(&report),
        [("c/sub.ts".to_string(), 3, "implementation".to_string())],
        "{report}"
    );
    // `c` has no dependency on `a` of its own.
    let why = serde_json::to_value(commands::why(&config(&root), "c", "a", &mut cache()).unwrap())
        .unwrap();
    assert_eq!(why["dependencies"], serde_json::json!([]), "{why}");
}
