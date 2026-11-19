---
status: accepted
---

# A `forbid_symbol` qualifier may be the end of a module path, in every language

A `forbid_symbol` name is a module and a name, `module.Name`. In Go the module could be written by the end of its path, `tags.DeleteIn` for `example.com/app/tags.DeleteIn`, because the match took any end that followed a `/`. Rust, Python, Java, C# and PHP cut their module names at `::`, `.` and `\`, so the same short form, `client::Client.query` or `client.Client.query`, matched nothing. It did not fail as `unmatched` either ([ADR 0004](0004-a-ban-that-can-never-fire-fails.md)): `client` is not a parent of `app::db::client`, so the item looked like one written before the code it forbids, stayed silent and never fired. One rule read differently by language is what AGENTS.md rules out.

The qualifier now matches the whole module path or any end of it that follows a separator of some language, as the registry lists them (`module_separators`), in every language alike.

## Considered options

- **The whole path only, in every language.** Rejected: it breaks the short form Go projects already write (`tags.DeleteIn`), and the import a Go file writes is the whole path, so the short form is how its code names the package.
- **The separators of the language of the file that uses the name.** Rejected: `never_firing` judges the names the index holds without the file that uses them, so it would cut at every separator while the match cut at the file's own, and an item could look matched there and still never fire here. Both cut at every separator.

## Consequences

- A short end may name several modules (`client.Client.query` covers `app.db.client` and `admin.client`); the item covers each, as it already did in Go.
- A `forbid_symbol` item that was silent in Rust, Python, Java, C# or PHP may now fire: projects that wrote the short form get the violations it always meant.
