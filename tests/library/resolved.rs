//! `easement policy`: the policy as loaded, read back as data. `extends` is
//! merged, anchors, globs and sets expanded, and each item says where the policy
//! writes it, so that a project checks its own documents against it instead of
//! reading `easement.yml` again by hand.

use std::path::{Path, PathBuf};

use easement::commands;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::project;
use crate::run::{config, error, pinned};

/// A base every service shares: a rule shared through an anchor, a rule the
/// configuration replaces, a top-level name the stores alone may use, and an
/// example of its own.
const BASE: &str = "# What every service shares.
x-transport: &transport
  must_not_reach: [{ name: http, because: the domain knows no transport }]

common: [kernel]

rules:
  domain:
    <<: *transport
    may_depend_on: [shared]
  web: { may_depend_on: [domain] }

forbid_symbol:
  - name: db.Client
    because: only the stores open connections
    allow: ['store-*']
    allow_in: [src/tools/migrate.ts]

examples:
  - { from: src/domain/order.ts, to: src/http/client.ts, expect: rejected, rule: dependency, because: the domain knows no transport }
";

const CONFIG: &str = "version: 1
extends: policy/base.yml

components:
  domain: { in: src/domain }
  shared: { in: src/shared }
  kernel:
    in: src/kernel
    surfaces:
      clock: { in: src/kernel/clock, importers: [web] }
  tools: { in: src/tools }
  features: { in: 'src/features/*', isolated: true }
  fake-db: { in: src/stores/Cargo.toml, part: feature/fake }
  web: { in: src/web, owner: '@acme/web' }
  http:
    in: src/http
    except: src/http/testdata
    public: src/http/api
    allow_importers: ['@drivers']
    because: one client for every outbound call
    surfaces:
      fakes: { in: src/http/fake, importers: [store-orders], because: fakes for the store tests }
  store-orders: { in: src/stores/orders, priority: 1, defines: ['*Repository'] }
  store-users: { in: src/stores/users, optional: true, public_symbols: [users.Find] }

sets:
  drivers: [web, 'store-*']

layers: [web, { name: domain, closed: true }, 'store-*']

rules:
  web:
    may_depend_on: [{ name: http, because: pages fetch through the client }]
    forbid_symbol: [{ name: lodash.merge, because: deep merges hide mutations }]
    because: pages call the client, not the stores
  'store-*': { may_depend_on: [http], forbid_external: [moment] }

forbid_external:
  - { name: axios, allow: [http], because: one HTTP client }
  - left-pad

examples:
  - { from: src/web/page.ts, to: src/http/api/client.ts, expect: allowed }
  - { from: src/web/page.ts, external: axios, expect: rejected, rule: external }
  - { from: src/web/page.ts, symbol: db.Client, defined_in: src/stores/users/db.ts, expect: rejected, rule: symbol }

no_cycles: [domain, web]
strict: true
unowned: [scripts]
tests: auto
entry: [src/web/page.ts]
exclude: ['src/gen/**']
target_only: ['**/*.pb.ts']
accepted_gaps: [{ kind: uncovered, files: 'vendor/**', because: vendored code }]
diagram: docs/architecture.md
manifests: true
packages: true
warn: [naming]
";

/// The diagram the policy names: a policy input, read without the code.
const DIAGRAM: &str = "```mermaid
graph TD
  web --> http
```
";

/// A project of the policy above, which nothing ever indexes.
fn services() -> (TempDir, PathBuf) {
    project(&[
        ("easement.yml", CONFIG),
        ("policy/base.yml", BASE),
        ("docs/architecture.md", DIAGRAM),
        ("src/domain/order.ts", "export const order = 1;\n"),
        ("src/web/page.ts", "export const page = 1;\n"),
        ("src/http/api/client.ts", "export const client = 1;\n"),
        ("src/tools/migrate.ts", "export const migrate = 1;\n"),
    ])
}

/// What `policy` says of the project at `root`, as data.
fn policy_json(root: &Path) -> Value {
    serde_json::to_value(commands::policy(&config(root)).unwrap()).unwrap()
}

/// The entry of the component `name`.
fn component<'a>(policy: &'a Value, name: &str) -> &'a Value {
    policy["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no component {name} in {policy}"))
}

/// The decision on the dependency of a component on `to` in `list`.
fn toward<'a>(entry: &'a Value, list: &str, to: &str) -> &'a Value {
    entry[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["to"] == to)
        .unwrap_or_else(|| panic!("no {to} in the {list} of {entry}"))
}

/// Text and JSON are what agents read, so both are pinned whole.
#[test]
fn policy_prints_the_policy_as_loaded() {
    let (_tmp, root) = services();
    pinned(|| {
        insta::assert_snapshot!(
            "policy_text",
            commands::policy(&config(&root)).unwrap().text()
        );
        insta::assert_json_snapshot!("policy_json", policy_json(&root));
    });
}

/// `extends` is merged and the anchor expanded, and each item names the input
/// that writes it: a rule the configuration replaces is the configuration's.
#[test]
fn each_item_names_the_input_that_writes_it() {
    let (_tmp, root) = services();
    let policy = policy_json(&root);
    assert_eq!(
        policy["inputs"],
        json!([
            { "path": "easement.yml", "kind": "config" },
            { "path": "policy/base.yml", "kind": "extended" },
            { "path": "docs/architecture.md", "kind": "diagram" },
        ])
    );
    let domain = component(&policy, "domain");
    assert_eq!(
        domain["provenance"],
        json!({ "field": "components.domain", "input": "easement.yml" })
    );
    // Through the anchor of the base.
    assert_eq!(
        toward(domain, "prohibitions", "http")["provenance"],
        json!({ "field": "rules.domain.must_not_reach[0]", "input": "policy/base.yml" })
    );
    assert_eq!(
        toward(domain, "grants", "kernel")["provenance"],
        json!({ "field": "common[0]", "input": "policy/base.yml" })
    );
    // The configuration's rule of `web` replaces the base's whole.
    let web = component(&policy, "web");
    assert_eq!(
        toward(web, "grants", "http")["provenance"],
        json!({ "field": "rules.web.may_depend_on[0]", "input": "easement.yml" })
    );
    assert!(
        web["grants"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["grant"] != "may_depend_on" || t["to"] != "domain"),
        "{web}"
    );
    assert_eq!(
        web["rule"]["forbid_symbol"],
        json!([{
            "name": "lodash.merge",
            "because": "deep merges hide mutations",
            "provenance": { "field": "rules.web.forbid_symbol[0]", "input": "easement.yml" },
        }])
    );
    // A glob key gives its rule to each component it covers, each item with it.
    let users = &component(&policy, "store-users")["rule"];
    assert_eq!(
        users["provenance"],
        json!({ "field": "rules.'store-*'", "input": "easement.yml", "through": ["store-*"] })
    );
    assert_eq!(
        users["forbid_external"][0]["provenance"],
        json!({
            "field": "rules.'store-*'.forbid_external[0]",
            "input": "easement.yml",
            "through": ["store-*"],
        })
    );
    // The component `tests: auto` makes is written as `tests`.
    assert_eq!(
        component(&policy, "tests")["provenance"],
        json!({ "field": "tests", "input": "easement.yml" })
    );
}

/// A top-level `forbid_symbol` item keeps the components and the paths that may use the name
/// apart, the components expanded from the glob that names them.
#[test]
fn a_top_level_item_lists_who_may_use_the_name() {
    let (_tmp, root) = services();
    let policy = policy_json(&root);
    assert_eq!(
        policy["forbid_symbol"],
        json!([{
            "name": "db.Client",
            "because": "only the stores open connections",
            "allow": ["store-orders", "store-users"],
            "allow_in": ["src/tools/migrate.ts"],
            "provenance": { "field": "forbid_symbol[0]", "input": "policy/base.yml" },
        }])
    );
    assert_eq!(
        policy["forbid_external"][0]["allow"],
        json!(["http"]),
        "{policy}"
    );
    assert_eq!(
        component(&policy, "http")["allow_importers"],
        json!(["web", "store-orders", "store-users"])
    );
}

/// `extends` joins the examples, the base's first, and each names the input
/// that writes it; `layers` and `common` are expanded too.
#[test]
fn examples_layers_and_common_name_where_they_are_written() {
    let (_tmp, root) = services();
    let policy = policy_json(&root);
    let inputs: Vec<&Value> = policy["examples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["provenance"])
        .collect();
    assert_eq!(
        inputs,
        [
            &json!({ "field": "examples[0]", "input": "policy/base.yml" }),
            &json!({ "field": "examples[1]", "input": "easement.yml" }),
            &json!({ "field": "examples[2]", "input": "easement.yml" }),
            &json!({ "field": "examples[3]", "input": "easement.yml" }),
        ]
    );
    assert_eq!(
        policy["layers"][1],
        json!({
            "components": ["domain"],
            "closed": true,
            "provenance": { "field": "layers[1]", "input": "easement.yml" },
        })
    );
    assert_eq!(
        policy["layers"][2],
        json!({
            "components": ["store-orders", "store-users"],
            "closed": false,
            "provenance": { "field": "layers[2]", "input": "easement.yml", "through": ["store-*"] },
        })
    );
    assert_eq!(
        policy["common"],
        json!([{ "name": "kernel", "provenance": { "field": "common[0]", "input": "policy/base.yml" } }])
    );
}

/// The policy is read without the code: a broken policy is the only error.
#[test]
fn a_policy_that_does_not_load_is_an_error() {
    let (_tmp, root) = project(&[("easement.yml", "version: 1\ncomponents: {}\nrulez: {}\n")]);
    let refused = error(commands::policy(&config(&root)));
    assert!(refused.contains("rulez"), "{refused}");
}
