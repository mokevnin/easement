# Changelog

## [0.2.0](https://github.com/mokevnin/easement/compare/v0.1.2...v0.2.0) (2026-10-25)


### ⚠ BREAKING CHANGES

* Python and Rust projects need scip-python and rust-analyzer installed, and third-party Python modules must be installed in the environment. The `python` key of easement.yml is removed. Violations name the target file rather than the specifier as written.
* Go and TypeScript projects need scip-go and scip-typescript installed (and node_modules for TypeScript). Only files a tsconfig.json includes are checked. require() in .ts files, vi.mock() and similar string arguments are no longer imports, and a violation names the target file rather than the specifier as written.

### Features

* file and directive rules, Go symbol rules, tests: auto, suggest and diff summary ([#11](https://github.com/mokevnin/easement/issues/11)) ([597bb5c](https://github.com/mokevnin/easement/commit/597bb5c7b87907fc4a025c9c265e104ac10f671c))
* resolve Go and TypeScript through SCIP indexers ([#13](https://github.com/mokevnin/easement/issues/13)) ([3602fc8](https://github.com/mokevnin/easement/commit/3602fc86ae98ec964b02ab7209a5a5425d9c2554))
* resolve Python and Rust through SCIP indexers ([#14](https://github.com/mokevnin/easement/issues/14)) ([b25d74f](https://github.com/mokevnin/easement/commit/b25d74f5fa63b9daceaf49aae8755a15f3a6f007))
* shell completions and coloured output ([#15](https://github.com/mokevnin/easement/issues/15)) ([92d4472](https://github.com/mokevnin/easement/commit/92d4472fd952c2535c8f95a27bfa788fe14e9744))

## [0.1.2](https://github.com/mokevnin/easement/compare/v0.1.1...v0.1.2) (2026-10-25)


### Features

* isolated instances, importer lists, priority, scopes and required imports ([#8](https://github.com/mokevnin/easement/issues/8)) ([afa3221](https://github.com/mokevnin/easement/commit/afa3221b22d11eeba075478ce286d4215f8b0a3b))

## [0.1.1](https://github.com/mokevnin/easement/compare/v0.1.0...v0.1.1) (2026-10-25)


### Features

* external, layer and metric rules, five more languages, CI formats and tooling ([#4](https://github.com/mokevnin/easement/issues/4)) ([abafd9d](https://github.com/mokevnin/easement/commit/abafd9d3d440ca7f07da06ab731b2a37e8380708))

## 0.1.0 (2026-10-25)


### Features

* diff mode for pull requests ([b79cd2f](https://github.com/mokevnin/easement/commit/b79cd2fb0fb9c97d774e58d2f75ef550464f8c55))
* install script and .deb/.rpm packages ([#2](https://github.com/mokevnin/easement/issues/2)) ([1c5a1df](https://github.com/mokevnin/easement/commit/1c5a1dfb9ad14086de629b479401e7dd5e2e0af6))
* public API, cycle detection, rule reasons and inline suppressions ([abfa525](https://github.com/mokevnin/easement/commit/abfa52578faac44d0f569ae67a7f4988c92b7d03))


### Bug Fixes

* **deps:** replace deprecated serde_yaml with serde_norway ([5ac7456](https://github.com/mokevnin/easement/commit/5ac7456586fa18b9f0f99b8272cbb77528ce20fa))
