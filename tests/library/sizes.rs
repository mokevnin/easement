//! The size of an edge: the names of one component that another writes, which
//! `why` reports with every place that writes each and whether the policy lets it
//! in through the public surface of the component that defines it.

use std::path::Path;

use easement::commands;

use crate::common::project;
use crate::run::{cache, config, pinned};

/// What `why` says of `from -> to` in the project at `root`, as text and as data.
fn why(root: &Path, from: &str, to: &str) -> (String, serde_json::Value) {
    let why = commands::why(&config(root), from, to, &mut cache()).unwrap();
    (why.text(), serde_json::to_value(&why).unwrap())
}

/// The names of a `why` answer, each as its name and the places that write it.
fn names(why: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    why["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            let places = name["places"]
                .as_array()
                .unwrap()
                .iter()
                .map(|place| format!("{}:{}", place["file"].as_str().unwrap(), place["line"]))
                .collect();
            (name["name"].as_str().unwrap().to_string(), places)
        })
        .collect()
}

const TWO_NAMES: &[(&str, &str)] = &[
    (
        "easement.yml",
        "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b }\nrules:\n  a: { may_depend_on: [b] }\n",
    ),
    (
        "src/b/model.ts",
        "export const Order = 1;\nexport const Item = 2;\nexport const Unused = 3;\n",
    ),
    (
        "src/a/one.ts",
        "import { Order } from \"../b/model\";\n\nexport const one = () => Order;\n",
    ),
    (
        "src/a/two.ts",
        "import { Item, Order } from \"../b/model\";\n\nexport const two = () => [Item, Order];\n",
    ),
];

/// `a` writes two names of `b` from two files, `Order` in both: the edge has two
/// names, `Order` with every place of both files.
#[test]
fn why_counts_the_names_of_the_target_that_the_source_writes() {
    let (_tmp, root) = project(TWO_NAMES);
    let (text, json) = why(&root, "a", "b");
    assert_eq!(
        names(&json),
        [
            (
                "src/b/model.Item".to_string(),
                vec!["src/a/two.ts:1".to_string(), "src/a/two.ts:3".to_string()]
            ),
            (
                "src/b/model.Order".to_string(),
                vec![
                    "src/a/one.ts:1".to_string(),
                    "src/a/one.ts:3".to_string(),
                    "src/a/two.ts:1".to_string(),
                    "src/a/two.ts:3".to_string(),
                ]
            ),
        ],
        "{json}"
    );
    assert!(text.contains("a writes 2 names of b:\n"), "{text}");
    assert_eq!(json["modules"], serde_json::json!([]), "{json}");
    pinned(|| {
        insta::assert_snapshot!("why_names_text", text);
        insta::assert_json_snapshot!("why_names_json", json);
    });
}

/// Who may use what of `b`: its barrel is its public surface, and `public_symbols`
/// lists `Order` and `Secret`. `a` writes `Order` and `Hidden` through the barrel,
/// both defined in the private `model`, and `Secret` from the private `secret`.
const SURFACE: &[(&str, &str)] = &[
    (
        "easement.yml",
        "version: 1\ncomponents:\n  a: { in: src/a }\n  b: { in: src/b, public: src/b/index.ts, public_symbols: [Order, Secret] }\nrules:\n  a: { may_depend_on: [b] }\n",
    ),
    (
        "src/b/model.ts",
        "export const Order = 1;\nexport const Hidden = 2;\n",
    ),
    (
        "src/b/index.ts",
        "export { Order, Hidden } from \"./model\";\n",
    ),
    ("src/b/secret.ts", "export const Secret = 3;\n"),
    (
        "src/a/one.ts",
        "import { Hidden, Order } from \"../b\";\n\nexport const one = () => [Hidden, Order];\n",
    ),
    (
        "src/a/two.ts",
        "import { Secret } from \"../b/secret\";\n\nexport const two = () => Secret;\n",
    ),
];

/// A name re-exported by a barrel counts once, in the file that defines it; the
/// barrel is no module named bare. Whether each name gets in through the public
/// surface of `b` is what `privacy` and `public_symbols` say of it: `Order` does;
/// `Hidden` is not in `public_symbols`; `Secret` is, but its file is private.
#[test]
fn why_says_whether_each_name_gets_in_through_the_public_surface() {
    let (_tmp, root) = project(SURFACE);
    let (text, json) = why(&root, "a", "b");
    let public: Vec<(&str, &str, &serde_json::Value)> = json["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            (
                name["name"].as_str().unwrap(),
                name["file"].as_str().unwrap(),
                &name["public"],
            )
        })
        .collect();
    assert_eq!(
        public,
        [
            (
                "src/b/model.Hidden",
                "src/b/model.ts",
                &serde_json::json!(false)
            ),
            (
                "src/b/model.Order",
                "src/b/model.ts",
                &serde_json::json!(true)
            ),
            (
                "src/b/secret.Secret",
                "src/b/secret.ts",
                &serde_json::json!(false)
            ),
        ],
        "{json}"
    );
    assert_eq!(json["modules"], serde_json::json!([]), "{json}");
    assert!(
        text.contains("a writes 3 names of b:\n  src/b/model.Hidden (2 places) [not public]\n  src/b/model.Order (2 places)\n  src/b/secret.Secret (2 places) [not public]\n"),
        "{text}"
    );
    // With no public surface, nothing is said of it.
    let (_tmp, root) = project(TWO_NAMES);
    let (_, json) = why(&root, "a", "b");
    assert!(
        json["names"]
            .as_array()
            .unwrap()
            .iter()
            .all(|name| name.get("public").is_none()),
        "{json}"
    );
}

/// A module `a` names without writing any name of it (an import for its effect,
/// a namespace it never reads) is listed apart, with its places, so the edge never
/// looks smaller than the imports behind it. A namespace whose names `a` reads is
/// no such module: its names are.
#[test]
fn why_lists_a_module_named_without_a_name_apart() {
    let mut files = TWO_NAMES.to_vec();
    files.extend([
        (
            "src/b/polyfill.ts",
            "export {};\n(globalThis as { ready?: boolean }).ready = true;\n",
        ),
        ("src/b/unread.ts", "export const Unread = 4;\n"),
        ("src/b/read.ts", "export const Read = 5;\n"),
        (
            "src/a/three.ts",
            "import \"../b/polyfill\";\nimport * as unread from \"../b/unread\";\nimport * as read from \"../b/read\";\n\nexport const three = () => read.Read;\n",
        ),
    ]);
    let (_tmp, root) = project(&files);
    let (text, json) = why(&root, "a", "b");
    assert_eq!(
        json["modules"],
        serde_json::json!([
            { "file": "src/b/polyfill.ts", "places": [{ "file": "src/a/three.ts", "line": 1 }] },
            { "file": "src/b/unread.ts", "places": [{ "file": "src/a/three.ts", "line": 2 }] },
        ]),
        "{json}"
    );
    let written: Vec<String> = names(&json).into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        written,
        ["src/b/model.Item", "src/b/model.Order", "src/b/read.Read"],
        "{json}"
    );
    assert!(
        text.contains("a names 2 modules of b without writing a name of it:\n  src/b/polyfill.ts (1 place)\n  src/b/unread.ts (1 place)\n"),
        "{text}"
    );
}

/// The names of a `why` answer by their last segment, each with its places, and
/// the modules named bare: what must not differ by language.
fn shape(why: &serde_json::Value) -> (Vec<(String, Vec<String>)>, serde_json::Value) {
    let names = names(why)
        .into_iter()
        .map(|(name, places)| {
            let short = name.rsplit(['.', '/']).next().unwrap().to_string();
            (short, places)
        })
        .collect();
    (names, why["modules"].clone())
}

/// The same edge in Go and in Python: `a` writes `Order` from two files and `Item`
/// from one. Go's package, named on a line of its own and defined in a file of `b`
/// other than the names', is no module named bare.
#[test]
fn the_size_of_an_edge_has_the_same_shape_in_go_and_python() {
    let policy = |a: &str, b: &str| {
        format!(
            "version: 1\ncomponents:\n  a: {{ in: {a} }}\n  b: {{ in: {b} }}\nrules:\n  a: {{ may_depend_on: [b] }}\n"
        )
    };
    let go_policy = policy("a", "b");
    let (_tmp, root) = project(&[
        ("easement.yml", &go_policy),
        ("go.mod", "module example.com/app\n\ngo 1.25\n"),
        ("b/doc.go", "// Package b holds orders.\npackage b\n"),
        (
            "b/model.go",
            "package b\n\nconst Order = 1\n\nconst Item = 2\n",
        ),
        (
            "a/one.go",
            "package a\n\nimport \"example.com/app/b\"\n\nfunc One() int { return b.Order }\n",
        ),
        (
            "a/two.go",
            "package a\n\nimport \"example.com/app/b\"\n\nfunc Two() int {\n\treturn b.Item + b.Order\n}\n",
        ),
    ]);
    let (text, json) = why(&root, "a", "b");
    let s = |all: &[&str]| all.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        shape(&json),
        (
            vec![
                ("Item".to_string(), s(&["a/two.go:6"])),
                ("Order".to_string(), s(&["a/one.go:5", "a/two.go:6"])),
            ],
            serde_json::json!([])
        ),
        "{json}"
    );
    assert!(text.contains("a writes 2 names of b:\n"), "{text}");

    let python_policy = policy("app/a", "app/b");
    let (_tmp, root) = project(&[
        ("easement.yml", &python_policy),
        ("app/__init__.py", ""),
        ("app/b/__init__.py", ""),
        (
            "app/b/model.py",
            "class Order:\n    pass\n\n\nclass Item:\n    pass\n",
        ),
        ("app/a/__init__.py", ""),
        (
            "app/a/one.py",
            "from app.b.model import Order\n\n\ndef one():\n    return Order\n",
        ),
        (
            "app/a/two.py",
            "from app.b.model import Item, Order\n\n\ndef two():\n    return [Item, Order]\n",
        ),
    ]);
    let (text, json) = why(&root, "a", "b");
    assert_eq!(
        shape(&json),
        (
            vec![
                ("Item".to_string(), s(&["app/a/two.py:1", "app/a/two.py:5"])),
                (
                    "Order".to_string(),
                    s(&[
                        "app/a/one.py:1",
                        "app/a/one.py:5",
                        "app/a/two.py:1",
                        "app/a/two.py:5"
                    ])
                ),
            ],
            serde_json::json!([])
        ),
        "{json}"
    );
    assert!(text.contains("a writes 2 names of b:\n"), "{text}");
}

/// The `symbols` evidence gaps of a `why` answer, each as its files and whether
/// its message names `language`.
fn name_gaps(why: &serde_json::Value, language: &str) -> Vec<(serde_json::Value, bool)> {
    why["evidence_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|gap| gap["kind"] == "symbols")
        .map(|gap| {
            let message = gap["message"].as_str().unwrap();
            (gap["files"].clone(), message.contains(language))
        })
        .collect()
}

/// A file in a language whose uses of names easement does not read (the registry's
/// `symbols`) gives an evidence gap, not an empty and clean list of names: its
/// imports are known, the names it writes are not. Kotlin has no index; Java has
/// one, whose names are not verified.
#[test]
fn the_names_a_file_writes_in_a_language_without_verified_uses_are_a_gap() {
    let (_tmp, root) = crate::reach::reach_project(&[]);
    let (text, json) = why(&root, "util", "http");
    assert_eq!(json["names"], serde_json::json!([]), "{json}");
    assert_eq!(json["modules"], serde_json::json!([]), "{json}");
    assert_eq!(
        name_gaps(&json, "kotlin"),
        [(serde_json::json!(["src/util/Retry.kt"]), true)],
        "{json}"
    );
    pinned(|| insta::assert_snapshot!("why_names_unverified_text", text));
    // With no direct dependency, only a chain, the direct edge has no size.
    let (_, json) = why(&root, "domain", "http");
    assert!(json["chain"].is_object(), "{json}");
    assert_eq!(json["names"], serde_json::json!([]), "{json}");
    assert_eq!(name_gaps(&json, "kotlin"), [], "{json}");

    let (_tmp, root) = crate::languages::java_three();
    let (_, json) = why(&root, "chunk", "blob");
    assert_eq!(json["names"], serde_json::json!([]), "{json}");
    assert_eq!(json["modules"], serde_json::json!([]), "{json}");
    assert_eq!(
        name_gaps(&json, "java"),
        [(
            serde_json::json!(["src/main/java/fx/chunk/Chunk.java"]),
            true
        )],
        "{json}"
    );
}
