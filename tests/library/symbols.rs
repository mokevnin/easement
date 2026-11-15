//! The rules judged on the names code uses rather than the files it imports:
//! `forbid_symbol`, `public_symbols` and `reach: symbols`, with Ruby's constants
//! and methods, and the evidence gap a language whose uses easement does not
//! read (or does not verify) leaves in their place.

use std::fs;
use std::path::PathBuf;

use easement::CacheUse;
use easement::commands::{self, Outcome};
use tempfile::TempDir;

use crate::common::{fixture, project};
use crate::languages::{csharp_three, java_three, judged};
use crate::run::{
    check, check_in, check_with, composer_install, config, error, json, leaves_out,
    no_scip_ruby_here, pinned, text, try_check_with,
};

/// The `symbols` evidence gaps of a JSON report.
fn symbol_gaps(report: &serde_json::Value) -> Vec<&serde_json::Value> {
    report["evidence_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|gap| gap["kind"] == "symbols")
        .collect()
}

#[test]
fn a_banned_java_member_is_an_evidence_gap_not_a_violation() {
    // scip-java reports member uses, but none is verified, so the rule does not judge them.
    let (_tmp, root) = fixture("java");
    fs::write(
        root.join("src/main/java/com/acme/shop/infra/OrderRepository.java"),
        "package com.acme.shop.infra;\n\nimport com.acme.shop.domain.Order;\n\npublic class OrderRepository {\n    public Order find(String id) { return Order.empty(); }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("easement.yml"),
        "version: 1\ncomponents:\n  domain: { in: src/main/java/com/acme/shop/domain }\n  infra: { in: src/main/java/com/acme/shop/infra }\n  web: { in: src/main/java/com/acme/shop/web }\nrules:\n  infra: { may_depend_on: [domain], forbid_symbol: [com.acme.shop.domain.Order.empty, com.acme.shop.domain.empty, Order.empty] }\n  web: { may_depend_on: [domain, infra] }\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert_eq!(report["violations"], serde_json::json!([]), "{report}");
    assert!(leaves_out(&report), "{report}");
    let gaps = symbol_gaps(&report);
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(
        gaps[0]["files"],
        serde_json::json!(["src/main/java/com/acme/shop/infra/OrderRepository.java"])
    );
    assert!(
        gaps[0]["message"].as_str().unwrap().contains("java"),
        "{report}"
    );
}

#[test]
fn a_java_file_that_reaches_public_symbols_through_a_constructor_or_a_member_is_a_gap() {
    let (_tmp, root) = java_three();
    let base = "src/main/java/fx";
    fs::write(
        root.join("easement.yml"),
        format!("version: 1\ncomponents:\n  blob: {{ in: {base}/blob, public_symbols: [Block] }}\n  chunk: {{ in: {base}/chunk }}\n  gw: {{ in: {base}/gw }}\nrules:\n  gw: {{ may_depend_on: [chunk, blob] }}\n  chunk: {{ may_depend_on: [blob] }}\n"),
    )
    .unwrap();
    let (found, report) = judged(&root);
    assert_eq!(found, [], "{report}");
    let gaps = symbol_gaps(&report);
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(
        gaps[0]["files"],
        serde_json::json!([
            "src/main/java/fx/chunk/Chunk.java",
            "src/main/java/fx/gw/Gw.java",
            "src/main/java/fx/gw/Qualified.java",
            "src/main/java/fx/gw/Wild.java"
        ]),
        "{report}"
    );
}

/// C# uses of names are not verified yet, so a file that reaches a `public_symbols`
/// component, through an attribute too, is an evidence gap, never a pass.
#[test]
fn a_csharp_file_that_reaches_public_symbols_through_an_attribute_is_a_gap() {
    let (_tmp, root) = csharp_three();
    fs::write(
        root.join("easement.yml"),
        "version: 1\ncomponents:\n  blob: { in: Blob, public_symbols: [Block] }\n  chunk: { in: Chunk }\n  gw: { in: Gw }\nrules:\n  gw: { may_depend_on: [chunk, blob] }\n  chunk: { may_depend_on: [blob] }\n",
    )
    .unwrap();
    let (found, report) = judged(&root);
    assert_eq!(found, [], "{report}");
    let gaps = symbol_gaps(&report);
    assert_eq!(gaps.len(), 1, "{report}");
    let files = gaps[0]["files"].as_array().unwrap();
    for file in ["Chunk/Chunk.cs", "Gw/Marked.cs"] {
        assert!(files.contains(&serde_json::json!(file)), "{file}: {report}");
    }
}

/// The text of this gap is pinned in `tests/cli.rs`
/// (`the_text_of_a_symbols_gap_is_pinned`).
#[test]
fn a_symbol_rule_over_kotlin_is_an_evidence_gap() {
    // An adapter reports no symbol uses, so the rule passes without judging anything.
    let (_tmp, root) = fixture("kotlin");
    let config = root.join("easement.yml");
    // The fixture's one violation (web -> infra) is allowed, so only the gap is left.
    let yaml = fs::read_to_string(&config).unwrap().replace(
        "web: { may_depend_on: [domain] }",
        "web: { may_depend_on: [domain, infra] }",
    ) + "\nforbid_symbol: [com.acme.shop.domain.Order]\n";
    fs::write(&config, yaml).unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Passed);
    let report = json(&checked);
    assert!(leaves_out(&report), "{report}");
    let gaps = symbol_gaps(&report);
    assert_eq!(gaps.len(), 1, "{report}");
    assert_eq!(
        gaps[0]["files"],
        serde_json::json!([
            "src/main/kotlin/com/acme/shop/domain/Order.kt",
            "src/main/kotlin/com/acme/shop/infra/OrderRepository.kt",
            "src/main/kotlin/com/acme/shop/web/OrderController.kt",
        ])
    );
    let message = gaps[0]["message"].as_str().unwrap();
    assert!(
        message.contains("`forbid_symbol`") && message.contains("kotlin"),
        "{message}"
    );
    let checked = check_with(&root, |o| o.require_evidence = true);
    assert_eq!(checked.outcome(), Outcome::EvidenceRequired);
}

#[test]
fn symbols_are_checked_through_imports() {
    let (_tmp, root) = fixture("symbols");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    for said in [
        "internal/handlers/books.go:9: handler must not use db.Client [symbol]",
        "because: use db.Scoped so every query is tenant-bound",
        "internal/handlers/books.go:12: handler -> db: Raw is not in the public symbols of db (Scoped, Tx)",
        "because: handlers get a scoped client, never the raw one",
        "internal/handlers/books.go:9: handler -> db: Client is not in the public symbols of db",
    ] {
        assert!(report.contains(said), "{said}: {report}");
    }
    // Scoped and Tx are public; the Raw on line 10 is suppressed.
    assert!(!report.contains("books.go:10"), "{report}");
    assert!(report.contains("3 violation(s)"), "{report}");
}

/// A member of a type (a method here) is named after the type it is used through,
/// in every language whose uses of names easement reads: the ban fires on that
/// member and on no other of the type.
#[test]
fn forbid_symbol_names_a_member_of_a_type_in_each_language() {
    let policy = |db: &str, web: &str, banned: &str| {
        format!(
            "version: 1\ncomponents:\n  db: {{ in: {db} }}\n  web: {{ in: {web} }}\nrules:\n  web: {{ may_depend_on: [db], forbid_symbol: [{banned}] }}\n"
        )
    };
    let go = project(&[
        ("go.mod", "module example.com/app\n\ngo 1.25\n"),
        (
            "internal/db/db.go",
            "package db\n\ntype Client struct{}\n\nfunc (c *Client) Query() int { return 1 }\n\nfunc (c *Client) Ping() int { return 0 }\n\nfunc New() *Client { return &Client{} }\n",
        ),
        (
            "internal/web/web.go",
            "package web\n\nimport \"example.com/app/internal/db\"\n\nfunc Run() int {\n\tc := db.New()\n\treturn c.Query() + c.Ping()\n}\n",
        ),
        (
            "easement.yml",
            &policy("internal/db", "internal/web", "db.Client.Query"),
        ),
    ]);
    let ts = project(&[
        ("package.json", "{\"name\":\"app\",\"version\":\"1.0.0\"}\n"),
        ("tsconfig.json", "{\"compilerOptions\":{\"strict\":true}}\n"),
        (
            "src/db/client.ts",
            "export class Client {\n  query(): number { return 1; }\n  ping(): number { return 0; }\n}\n",
        ),
        (
            "src/web/web.ts",
            "import { Client } from \"../db/client\";\n\nexport function run(): number {\n  const c = new Client();\n  return c.query() + c.ping();\n}\n",
        ),
        (
            "easement.yml",
            &policy("src/db", "src/web", "src/db/client.Client.query"),
        ),
    ]);
    let py = project(&[
        ("app/__init__.py", ""),
        ("app/db/__init__.py", ""),
        ("app/web/__init__.py", ""),
        (
            "app/db/client.py",
            "class Client:\n    def query(self) -> int:\n        return 1\n\n    def ping(self) -> int:\n        return 0\n",
        ),
        (
            "app/web/web.py",
            "from app.db.client import Client\n\n\ndef run() -> int:\n    c = Client()\n    return c.query() + c.ping()\n",
        ),
        (
            "easement.yml",
            &policy("app/db", "app/web", "app.db.client.Client.query"),
        ),
    ]);
    for ((_tmp, root), (file, line, banned)) in [
        (go, ("internal/web/web.go", 7, "db.Client.Query")),
        (ts, ("src/web/web.ts", 5, "src/db/client.Client.query")),
        (py, ("app/web/web.py", 6, "app.db.client.Client.query")),
    ] {
        let checked = check(&root);
        assert_eq!(checked.outcome(), Outcome::Failed, "{file}");
        let report = json(&checked);
        let found: Vec<(String, u64, String)> = report["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["file"].as_str().unwrap().to_string(),
                    v["line"].as_u64().unwrap(),
                    v["message"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [(file.to_string(), line, format!("web must not use {banned}"))],
            "{report}"
        );
    }
}

/// A TypeScript project whose `app` uses `Foo`, declared in `src/types/types.d.ts`,
/// and whose policy bans `banned` to `app`.
fn declared_type_project(banned: &str) -> (TempDir, PathBuf) {
    project(&[
        ("package.json", "{\"name\":\"app\",\"version\":\"1.0.0\"}\n"),
        ("tsconfig.json", "{\"compilerOptions\":{\"strict\":true}}\n"),
        (
            "src/types/types.d.ts",
            "export interface Foo {\n  x: number;\n}\n",
        ),
        (
            "src/app/main.ts",
            "import type { Foo } from \"../types/types\";\n\nexport const foo: Foo = { x: 1 };\n",
        ),
        (
            "easement.yml",
            &format!(
                "version: 1\ncomponents:\n  types: {{ in: src/types }}\n  app: {{ in: src/app }}\nrules:\n  app:\n    may_depend_on: [types]\n    forbid_symbol: [{banned}]\n"
            ),
        ),
    ])
}

/// A declaration file is the module its name says without `.d.ts`, as a source
/// file is without `.ts`: `src/types/types.d.ts` is `src/types/types` (#107).
#[test]
fn forbid_symbol_names_a_type_of_a_declaration_file_by_its_module() {
    let (_tmp, root) = declared_type_project("src/types/types.Foo");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains("src/app/main.ts:1: app must not use src/types/types.Foo [symbol]"),
        "{report}"
    );
}

/// The spelling the declaration file used to force, `types.d.Foo`, names nothing
/// now: it fails as `unmatched` with the name it means, never passing in silence
/// (ADR 0004).
#[test]
fn forbid_symbol_spelled_with_a_declaration_suffix_fails_as_unmatched() {
    let (_tmp, root) = declared_type_project("src/types/types.d.Foo");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains(
            "easement.yml:8: `forbid_symbol` `src/types/types.d.Foo` of app matches no symbol of the index, so it can never fire; did you mean `src/types/types.Foo`? [unmatched]"
        ),
        "{report}"
    );
    assert!(!report.contains("[symbol]"), "{report}");
}

#[test]
fn a_top_level_forbid_symbol_covers_every_file_unowned_included() {
    let (_tmp, root) = fixture("symbols_global");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    // A component gets the global ban and its own.
    assert!(
        report.contains("internal/handlers/books.go:5: handler must not use db.Client [symbol]"),
        "{report}"
    );
    assert!(
        report.contains("internal/handlers/books.go:6: handler must not use db.Raw [symbol]"),
        "{report}"
    );
    // A file no component owns is judged too, and an inline suppression holds there.
    assert!(
        report.contains("internal/lib/util.go:6: unowned files must not use db.Client [symbol]"),
        "{report}"
    );
    assert!(!report.contains("internal/lib/util.go:3"), "{report}");
    // db.Raw is only forbidden to the handler.
    assert!(!report.contains("internal/lib/util.go:11"), "{report}");
    assert!(
        report.contains("because: use db.Scoped so every query is tenant-bound"),
        "{report}"
    );
}

#[test]
fn a_top_level_forbidden_name_may_be_used_only_by_the_components_it_allows() {
    let (_tmp, root) = fixture("symbols_allow");
    // Indexed afresh, so that no other run decides whether the output says it is cached.
    let own = TempDir::new().unwrap();
    let checked = check_in(&root, own.path(), CacheUse::Off).unwrap();
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    let banned = "example.com/app/internal/domain/tags.DeleteIn";
    for said in [
        // Another component.
        format!("internal/web/handler.go:6: web must not use {banned}"),
        // A file of no component.
        format!("internal/lib/lib.go:6: unowned files must not use {banned}"),
        // The component that defines the name: a sibling package, and another file
        // of the defining package.
        format!("internal/domain/contacts/contacts.go:6: domain must not use {banned}"),
        format!("internal/domain/tags/sweep.go:4: domain must not use {banned}"),
    ] {
        assert!(report.contains(&said), "{said}: {report}");
    }
    // An instance of the isolated component it allows, and the file that defines the name.
    assert!(
        !report.contains("internal/usecase/tagdeletion/delete.go"),
        "{report}"
    );
    assert!(!report.contains("internal/domain/tags/tags.go"), "{report}");
    pinned(|| insta::assert_snapshot!("forbid_symbol_allow_text", report));
}

#[test]
fn a_top_level_forbidden_name_may_be_used_in_the_paths_it_allows() {
    let (_tmp, root) = fixture("symbols_allow_in");
    let own = TempDir::new().unwrap();
    let checked = check_in(&root, own.path(), CacheUse::Off).unwrap();
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    // Another package of the component whose two packages the paths let in.
    assert!(
        report.contains(
            "internal/modules/reports/report.go:5: modules must not use example.com/app/internal/store.Client (only wiring, internal/modules/billing, internal/modules/mailing/*.go may)"
        ),
        "{report}"
    );
    // A directory, its test code included, and a glob of files; and the component allowed.
    for granted in [
        "internal/modules/billing/billing.go",
        "internal/modules/billing/billing_test.go",
        "internal/modules/mailing/send.go",
        "cmd/app/main.go",
    ] {
        assert!(!report.contains(granted), "{granted}: {report}");
    }
    pinned(|| insta::assert_snapshot!("forbid_symbol_allow_in_text", report));
}

#[test]
fn explain_names_the_forbidden_names_a_path_is_allowed() {
    let (_tmp, root) = fixture("symbols_allow_in");
    let explained = |path: &str| commands::explain(&config(&root), &root.join(path)).unwrap();
    let mailing = explained("internal/modules/mailing/send.go");
    assert_eq!(
        serde_json::to_value(&mailing).unwrap()["symbol_grants"],
        serde_json::json!([{
            "name": "example.com/app/internal/store.Client",
            "allow_in": "internal/modules/mailing/*.go",
            "because": "a module takes the scoped client it is given",
            "provenance": { "field": "forbid_symbol[0].allow_in[1]", "input": "easement.yml" },
        }])
    );
    let said = mailing.text().unwrap();
    assert!(
        said.contains(
            "  may use, although forbid_symbol forbids it: example.com/app/internal/store.Client (allow_in `internal/modules/mailing/*.go`: a module takes the scoped client it is given)\n"
        ),
        "{said}"
    );
    // Another package of the component: no grant to show.
    let reports = explained("internal/modules/reports/report.go");
    assert!(
        serde_json::to_value(&reports)
            .unwrap()
            .get("symbol_grants")
            .is_none()
    );
    assert!(!reports.text().unwrap().contains("forbid_symbol"));
}

#[test]
fn a_path_allowed_a_forbidden_name_that_matches_no_file_is_unmatched() {
    let (_tmp, root) = fixture("symbols_allow_in");
    let policy = fs::read_to_string(config(&root)).unwrap().replace(
        "internal/modules/mailing/*.go",
        "internal/modules/mailer/*.go",
    );
    fs::write(config(&root), policy).unwrap();
    let report = text(&check(&root));
    assert!(
        report.contains(
            "easement.yml:12: `forbid_symbol` `example.com/app/internal/store.Client` is allowed in `internal/modules/mailer/*.go`, which matches no file; fix the pattern or drop it (files that `exclude` or .gitignore hide do not count) [unmatched]"
        ),
        "{report}"
    );
}

#[test]
fn a_path_cannot_be_allowed_a_name_its_component_forbids() {
    let (_tmp, root) = fixture("symbols_allow_in");
    let policy = fs::read_to_string(config(&root)).unwrap().replace(
        "may_depend_on: [store]",
        "may_depend_on: [store]\n    forbid_symbol: [store.Client]",
    );
    fs::write(config(&root), policy).unwrap();
    assert_eq!(
        error(try_check_with(&root, |_| {})),
        "forbid_symbol[0].allow_in lets `internal/modules/billing` use `example.com/app/internal/store.Client`, but internal/modules/billing/billing.go there belongs to `modules`, whose rules.modules.forbid_symbol forbids it as `store.Client`"
    );
}

#[test]
fn a_misspelled_name_with_allow_still_can_never_fire() {
    let (_tmp, root) = fixture("symbols_allow");
    // The package is `tags`, under `domain`: the name sits deeper under its qualifier.
    let policy = fs::read_to_string(config(&root)).unwrap().replace(
        "example.com/app/internal/domain/tags.DeleteIn",
        "example.com/app/internal/domain.DeleteIn",
    );
    fs::write(config(&root), policy).unwrap();
    let report = text(&check(&root));
    assert!(
        report.contains(
            "easement.yml:9: `forbid_symbol` `example.com/app/internal/domain.DeleteIn` of the top level matches no symbol of the index, so it can never fire; did you mean `example.com/app/internal/domain/tags.DeleteIn`? [unmatched]"
        ),
        "{report}"
    );
}

#[test]
fn forbid_symbol_catches_every_way_to_use_a_type() {
    let (_tmp, root) = fixture("symbol_uses");
    // One file per way of using db.Client, and a file that uses it twice: every use is reported.
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    for said in [
        "internal/handlers/field.go:6: handler must not use db.Client",
        "internal/handlers/param.go:5: handler must not use db.Client",
        "internal/handlers/local.go:6: handler must not use db.Client",
        // A method call on a value of the type, without the type being spelled out.
        "internal/handlers/chained.go:6: handler must not use db.Client",
        "internal/handlers/held.go:7: handler must not use db.Client",
        "internal/handlers/twice.go:6: handler must not use db.Client",
        "internal/handlers/twice.go:7: handler must not use db.Client",
    ] {
        assert!(report.contains(said), "{said}: {report}");
    }
    assert!(!report.contains("allowed.go"), "{report}");
    assert!(report.contains("7 violation(s)"), "{report}");
}

#[test]
fn reach_by_symbols_follows_the_method_a_call_names_not_the_whole_file() {
    if no_scip_ruby_here() {
        return;
    }
    // `Pure` calls `Document.host`, which only parses; `Document.fetch` and `both` reach `Http`.
    let (_tmp, root) = fixture("ruby_reach");
    assert_eq!(check(&root).outcome(), Outcome::Passed);
    // By files, the same call reaches what `fetch` uses.
    let config = root.join("easement.yml");
    let policy = fs::read_to_string(&config).unwrap();
    fs::write(&config, policy.replace("reach: symbols\n", "")).unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains("app/models/pure.rb:4: models reaches external"),
        "{report}"
    );
}

#[test]
fn reach_by_symbols_keeps_a_chain_through_the_method_that_reaches_it() {
    if no_scip_ruby_here() {
        return;
    }
    let (_tmp, root) = fixture("ruby_reach");
    fs::write(
        root.join("app/models/leaky.rb"),
        "# typed: true\nclass Leaky\n  def run(uid)\n    Document.both(uid)\n  end\nend\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains(
            "app/models/leaky.rb:4: models reaches external through no component: app/models/leaky.rb:4 -> app/lib/document.rb:12 -> app/external/http.rb [reach]"
        ),
        "{report}"
    );
    assert!(!report.contains("pure.rb"), "{report}");
}

#[test]
fn reach_by_symbols_keeps_a_chain_when_no_method_is_named() {
    if no_scip_ruby_here() {
        return;
    }
    // The receiver is opaque to Sorbet, so the index has the class but not the method: any
    // method of it may be what runs.
    let (_tmp, root) = fixture("ruby_reach");
    fs::write(
        root.join("app/models/opaque.rb"),
        "# typed: true\nclass Opaque\n  def run(uid)\n    T.unsafe(Document).fetch(uid)\n  end\nend\n",
    )
    .unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains("app/models/opaque.rb:4: models reaches external through no component"),
        "{report}"
    );
}

#[test]
fn ruby_symbols_are_constants_and_methods() {
    if no_scip_ruby_here() {
        return;
    }
    let (_tmp, root) = fixture("ruby_symbols");
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    for said in [
        // A gem's constant, which Sorbet knows from the RBI in sorbet/rbi.
        "lib/shop/mutators/publish.rb:8: mutators must not use ActiveRecord.Rollback [symbol]",
        "because: a mutator returns its failure, it does not roll back",
        // One method of a class, called on the class.
        "lib/shop/web/controller.rb:18: web must not use Shop::Db::Client.connect [symbol]",
        // The class in a signature, a method called on a value of it, and a class method.
        "lib/shop/web/controller.rb:11: web -> db: Client is not in the public symbols of db (Scoped)",
        "lib/shop/web/controller.rb:13: web -> db: Client is not in the public symbols of db (Scoped)",
        "lib/shop/web/controller.rb:18: web -> db: Client is not in the public symbols of db (Scoped)",
    ] {
        assert!(report.contains(said), "{said}: {report}");
    }
    // Scoped is public, Shop::Db is only the namespace, and web may raise a rollback.
    assert!(!report.contains("controller.rb:17"), "{report}");
    assert!(!report.contains("controller.rb:19"), "{report}");
    assert!(report.contains("5 violation(s)"), "{report}");
}

/// The hint finds the namespace a name sits under at every separator a language
/// cuts its modules at, PHP's `\` among them, not only `::`, `.` and `/` (#108).
#[test]
fn a_forbid_symbol_name_under_a_deeper_php_namespace_fails_as_unmatched() {
    let (_tmp, root) = fixture("php");
    composer_install(&root);
    let config = config(&root);
    let policy = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{policy}forbid_symbol: [App.Order]\n")).unwrap();
    let line = policy.lines().count() + 1;
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains(&format!(
            "easement.yml:{line}: `forbid_symbol` `App.Order` of the top level matches no symbol of the index, so it can never fire; did you mean `App\\Domain.Order`? [unmatched]"
        )),
        "{report}"
    );
}

/// A ban that a deeper namespace shows to be misspelled can never fire: like an
/// `in` pattern that matches no file, it fails as `unmatched` at its line, so a
/// boundary cannot vanish while the check stays green (#56). A name the index has
/// never seen stays silent: banning what the code does not use yet is the point.
#[test]
fn a_forbid_symbol_name_that_only_a_deeper_namespace_defines_fails_as_unmatched() {
    if no_scip_ruby_here() {
        return;
    }
    let (_tmp, root) = fixture("ruby_symbols");
    let config = root.join("easement.yml");
    let policy = fs::read_to_string(&config).unwrap();
    // `connect` is a method of `Shop::Db::Client`, not of `Shop::Db`; `deploy` is
    // defined nowhere, which is what a ban on what the code does not use yet looks like.
    let policy = policy.replace(
        "      - { name: \"Shop::Db::Client.connect\"",
        "      - { name: \"Shop::Db.connect\" }\n      - { name: \"Shop::Db.deploy\" }\n      - { name: \"Shop::Db::Client.connect\"",
    );
    let line = policy
        .lines()
        .position(|l| l.contains("Shop::Db.connect"))
        .unwrap()
        + 1;
    fs::write(&config, policy).unwrap();
    let checked = check(&root);
    assert_eq!(checked.outcome(), Outcome::Failed);
    let report = text(&checked);
    assert!(
        report.contains(&format!(
            "easement.yml:{line}: `forbid_symbol` `Shop::Db.connect` of web matches no symbol of the index, so it can never fire; did you mean `Shop::Db::Client.connect`? [unmatched]"
        )),
        "{report}"
    );
    assert!(!report.contains("Shop::Db.deploy"), "{report}");
    assert!(report.contains("6 violation(s)"), "{report}");
    let report = json(&checked);
    assert!(
        report["stats"].get("symbols_unmatched").is_none(),
        "{report}"
    );
}
