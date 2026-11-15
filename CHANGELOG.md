# Changelog

## [0.6.4](https://github.com/mokevnin/easement/compare/v0.6.3...v0.6.4) (2026-11-14)


### Bug Fixes

* **scan:** take a name for the copy in the component that imports it ([#114](https://github.com/mokevnin/easement/issues/114)) ([1eeaecd](https://github.com/mokevnin/easement/commit/1eeaecd8451ed08e0f1203542788a77c8d98c71a))

## [0.6.3](https://github.com/mokevnin/easement/compare/v0.6.2...v0.6.3) (2026-11-12)


### Bug Fixes

* take a file an unresolved import may reach as unjudged, not unused ([932d559](https://github.com/mokevnin/easement/commit/932d5593efb735ba8f80040743d9e4bbe50a14de)), closes [#138](https://github.com/mokevnin/easement/issues/138)
* **test:** index the unresolved-import snapshot afresh ([39036c0](https://github.com/mokevnin/easement/commit/39036c06cc34cd951935dd6bf84f1fbae812cf53))

## [0.6.2](https://github.com/mokevnin/easement/compare/v0.6.1...v0.6.2) (2026-11-11)


### Features

* count an import as resolved only when the index takes it to a source target ([c83bf3a](https://github.com/mokevnin/easement/commit/c83bf3ac0d95abb066deb4552d99f870a8641630)), closes [#139](https://github.com/mokevnin/easement/issues/139)

## [0.6.1](https://github.com/mokevnin/easement/compare/v0.6.0...v0.6.1) (2026-11-09)


### Bug Fixes

* follow a re-export through barrels and name the re-exporter from its way ([701190b](https://github.com/mokevnin/easement/commit/701190b08463a2ea01966da1b8b82aeff7cd2044)), closes [#131](https://github.com/mokevnin/easement/issues/131)
* judge a forbidden name by who may use it, not by which list holds it ([af42469](https://github.com/mokevnin/easement/commit/af42469042faa0c53b88f911bda596217c585b6d)), closes [#133](https://github.com/mokevnin/easement/issues/133)
* judge a name reached through another component's re-export where it enters its component ([58f99c1](https://github.com/mokevnin/easement/commit/58f99c10a0d465e9d313b6e19a2b181adf73aff8)), closes [#131](https://github.com/mokevnin/easement/issues/131)
* read a TypeScript module augmentation as the package it augments ([d48f4ab](https://github.com/mokevnin/easement/commit/d48f4ab3b473db803be158f385db71527d9011c0)), closes [#134](https://github.com/mokevnin/easement/issues/134)

## [0.6.0](https://github.com/mokevnin/easement/compare/v0.5.1...v0.6.0) (2026-11-08)


### ⚠ BREAKING CHANGES

* a config whose key next to `<<` drops a merged item no longer loads.
* a use of an alias of another component's name is now a dependency on that component, so a policy that passed through an alias may fail.

### Features

* follow an alias to the names it stands for ([6d897ba](https://github.com/mokevnin/easement/commit/6d897bae7bf62684eb63977decd45383f9f295bf)), closes [#112](https://github.com/mokevnin/easement/issues/112)
* judge a re-export changed alone in --diff and name the re-exporter ([983417a](https://github.com/mokevnin/easement/commit/983417ad359c61f3ebb780f1caa953914ad458f9)), closes [#103](https://github.com/mokevnin/easement/issues/103)
* let a top-level forbid_symbol item name the components that may use it ([e7031a9](https://github.com/mokevnin/easement/commit/e7031a9d91a08c7afbfc172b9b7a44f3f9d4facd)), closes [#127](https://github.com/mokevnin/easement/issues/127)
* list every constraint a policy change loosens or tightens in review ([98c6a00](https://github.com/mokevnin/easement/commit/98c6a003a7ec8c9f7657294f25b6999eca184bb0)), closes [#128](https://github.com/mokevnin/easement/issues/128)
* refuse a key that drops items a `<<` merge brings in ([1bceaf5](https://github.com/mokevnin/easement/commit/1bceaf583ed1426669393dd021dff2c12125ad86)), closes [#129](https://github.com/mokevnin/easement/issues/129)


### Bug Fixes

* a language's foreign directories are one fact of its registry entry ([f5de062](https://github.com/mokevnin/easement/commit/f5de062e8334309182276d5727608a20a14cb576)), closes [#109](https://github.com/mokevnin/easement/issues/109)
* find the namespace a misspelled forbid_symbol name sits under at every language's separators ([9e3b4e4](https://github.com/mokevnin/easement/commit/9e3b4e485d0134dde6de2da93748552b74d98495)), closes [#108](https://github.com/mokevnin/easement/issues/108)
* judge a name reached through aliases alone by the surfaces of its component ([1dd555d](https://github.com/mokevnin/easement/commit/1dd555d57e3eb37df7f23f0b88efc12bfedcd599)), closes [#112](https://github.com/mokevnin/easement/issues/112)
* key vendored Go code again, as the indexer reads it ([5d28ea6](https://github.com/mokevnin/easement/commit/5d28ea6e17d98f4019139c162ef553a73f2dabaa)), closes [#109](https://github.com/mokevnin/easement/issues/109)
* leave the diagram arrows of a removed component out of review ([c9638ec](https://github.com/mokevnin/easement/commit/c9638ec4800c755a5b0c174f6838efc0d2718544))
* merge an anchor that merges another ([ece737d](https://github.com/mokevnin/easement/commit/ece737d652a1e2d997b15c41dcecbf278073c54b))
* refuse a null next to `<<` over a merged list or mapping ([e591edf](https://github.com/mokevnin/easement/commit/e591edf53649f679cca5876d573a0cab66af7707))


### Performance Improvements

* answer `why` from the dependencies at each place, found in one pass ([8a6032f](https://github.com/mokevnin/easement/commit/8a6032f7494daec7949b1ee7f0b57f21cd95a8f6))
* build the Kotlin and Swift adapters from the scan's one walk, only when they have files ([8b1409b](https://github.com/mokevnin/easement/commit/8b1409b019e3bcfdd7d5072434ebb330885e8f2f))
* parse a Go file up to its first declaration to read its head ([9cb5daf](https://github.com/mokevnin/easement/commit/9cb5daf41ad54ff41270f7c341ed93c48bf201ad)), closes [#126](https://github.com/mokevnin/easement/issues/126)
* read and parse each workspace Cargo.toml once per run ([a687b52](https://github.com/mokevnin/easement/commit/a687b523e686b186f6653a19d6e1f5f7df7e780a)), closes [#106](https://github.com/mokevnin/easement/issues/106)
* read each document's own names once and find what a type alias names in sorted order ([054c0b2](https://github.com/mokevnin/easement/commit/054c0b225cc32a32829ac2c55f63b9f974928b64))
* read each SCIP symbol once per run and share who defines what ([318212b](https://github.com/mokevnin/easement/commit/318212b61ccd5fc0cd96e0ab9e2e21fb7c4ba2b1))
* remember a component lookup once when threads miss it together ([b940542](https://github.com/mokevnin/easement/commit/b940542020545761b6832e5413b1d998cc19e517))
* remember which entry owns a path and part for the life of a component map ([9d3b0b2](https://github.com/mokevnin/easement/commit/9d3b0b25362361ad01475b3b31470cf1e943d433))

## [0.5.1](https://github.com/mokevnin/easement/compare/v0.5.0...v0.5.1) (2026-11-07)


### Features

* give the places of an unresolved import as structured fields ([d0cde51](https://github.com/mokevnin/easement/commit/d0cde51903582901c104abefa531189ffe963c00))
* give the places of an unresolved import as structured fields ([772021f](https://github.com/mokevnin/easement/commit/772021fb05e7ff5d5f76663e1a2de6a72e818d56)), closes [#115](https://github.com/mokevnin/easement/issues/115)
* read go.mod, Go imports and JUnit with ready-made crates; a SCIP header's escaped space ([5c350ff](https://github.com/mokevnin/easement/commit/5c350ff69cedca553eec1037e43f26128b1afe7d))
* write the JUnit report with quick-junit, wrapped in &lt;testsuites&gt; ([54498b2](https://github.com/mokevnin/easement/commit/54498b22a6a2b08682694b7b260ad5a7432e6add))


### Bug Fixes

* an absolute path of a file not written yet is relative to the root ([db78d34](https://github.com/mokevnin/easement/commit/db78d34ae57eb5bb6eb37c3e05db26aa8537511c))
* ask git about the project's repository, not the one the environment names ([428fde2](https://github.com/mokevnin/easement/commit/428fde268712478b133d1b59892e21c14ecfe7e0))
* ask git about the project's repository, not the one the environment names ([abf638d](https://github.com/mokevnin/easement/commit/abf638db9f419d3eb8014430ac51ba7a411e4c57))
* keep the index cache within 4 GiB across every directory ([#110](https://github.com/mokevnin/easement/issues/110)) ([5081770](https://github.com/mokevnin/easement/commit/508177023ac0a35cd4c5b03df6510ebae88d82d0))
* leave a go.mod with a directive Go does not know unread, not read past it ([afcc930](https://github.com/mokevnin/easement/commit/afcc930ce80b1a2cf23bd1861b055c0d46a76148))
* leave a go.mod with a path in backticks unread, as Go does ([6ec7c52](https://github.com/mokevnin/easement/commit/6ec7c52665740e10e523f07e04e823eb0eb0b60c))
* leave a go.mod with a path in backticks unread, as Go does ([21fae8c](https://github.com/mokevnin/easement/commit/21fae8cd3739aeacb2ace36fefea2ac3a167d211))
* name a file's module by one rule from the registry, declaration suffixes first ([ee6c885](https://github.com/mokevnin/easement/commit/ee6c8859d754ad3b660c4540ce987cec8922cfc9)), closes [#107](https://github.com/mokevnin/easement/issues/107)
* name the config of suggest --patch by its path in the project ([287e896](https://github.com/mokevnin/easement/commit/287e896368950138788282bac2186e6d2f640202))
* read a doubled space in a SCIP symbol's header as a space in its field ([4bac9da](https://github.com/mokevnin/easement/commit/4bac9da07f82347bf3ea0d7abecfe268bfc91a1b))
* read a Go file that starts with a byte order mark ([6edfe46](https://github.com/mokevnin/easement/commit/6edfe46be68481b3f626f8b02e4ac25d6139a1ef))
* report an import the indexer did not resolve as an evidence gap ([e90a39a](https://github.com/mokevnin/easement/commit/e90a39aff075dc138d97e325c3c6673d5bb1f28d)), closes [#113](https://github.com/mokevnin/easement/issues/113)
* resolve a written import by a module on its own path, not on its line ([cd9ed91](https://github.com/mokevnin/easement/commit/cd9ed91f34db9aac07f4002b88af6c69b909e618))
* resolve no import by a reference whose range is not on one line ([40bcfd7](https://github.com/mokevnin/easement/commit/40bcfd7c1f3f8e78b5b265cd35f0436c1f9cb1f7))
* run the indexers without the repository a parent git names ([737382b](https://github.com/mokevnin/easement/commit/737382b99c414f1fb48ba4f889582a9389943a53))
* take a path not written yet, and the config of a patch, as their paths in the project ([0f6a776](https://github.com/mokevnin/easement/commit/0f6a776bb5559b83fe88a818c40e412569cc20ee))
* tell a Go program by its package clause, not by a line that starts with it ([4e000de](https://github.com/mokevnin/easement/commit/4e000de3ba12d80f5e78fdda1f31fb6c35f4de01))


### Performance Improvements

* match a Python name by the ending of its path among the names that share its last part ([50aac2a](https://github.com/mokevnin/easement/commit/50aac2ada9bf1394b9a52e2a4cce5b414969885b))

## [0.5.0](https://github.com/mokevnin/easement/compare/v0.4.14...v0.5.0) (2026-11-06)


### ⚠ BREAKING CHANGES

* those keys are gone from `stats`; read `evidence_gaps` by `kind` and `narrowed` instead.
* `confidence` and `confidence_reasons` are gone from the JSON reports; a report leaves something out when `narrowed` or `evidence_gaps` is not empty.
* `go: { tags: [...] }` is `indexers: { go: { tags: [...] } }`.
* `stats.symbols_unmatched` is gone from `--format json`; such a ban is an `unmatched` violation.
* violation messages and the text of every command change wording, and the target of a `must_depend_on` violation reads `component <name>` instead of `import <name>`, beside `external <name>` and `file <name>`; baseline entries of such violations are keyed anew.
* under `--component` and `--scope` the counters (`files`, `internal_deps`, `external_deps`, `unmapped_files`) and the evidence gaps are those of the whole project, not of the part.
* verdicts change. Java, C#, PHP and Ruby code that called a method on a value of another component's type no longer depends on it, and loses the external dependencies that only such calls made. Rust, TypeScript and Python code that writes a name re-exported from another component now depends on that component; a Go file that uses a name of its own package defined in another file depends on that file. Baseline entries may need updating.
* `--diff` now fails on unchanged code that a change to the policy or to a manifest makes violate it, and judges the whole project when the change alters what is scanned.
* a project with symbol rules over Java, C#, PHP, Kotlin or Swift files now gets `confidence: partial` with a `symbols` evidence gap, and `check --require-evidence` fails on it. A violation that the unverified Java, C# or PHP member uses produced disappears and is replaced by the gap.
* a project whose policy has a `part` that matches no part now fails `check`; fix the part, or set `optional: true` on its component.

### Features

* `suggest --format json`, with the unread files behind each suggestion ([812773f](https://github.com/mokevnin/easement/commit/812773f8c82acf5fd130f9f4ee82bf5564e2ff37))
* `suggest --patch` edits easement.yml through a lossless YAML tree ([333a72b](https://github.com/mokevnin/easement/commit/333a72b90c4e15fd89ba814419f3555bc9034077))
* a report says what it leaves out in its three lists alone ([6cd4a04](https://github.com/mokevnin/easement/commit/6cd4a04b776fb9078793d4b9a66234c30e9e7463))
* add `check --require-evidence`, which fails a pass that rests on evidence gaps ([2d276ad](https://github.com/mokevnin/easement/commit/2d276adf6e233993d983c1da7ac6ad95a1a60258)), closes [#91](https://github.com/mokevnin/easement/issues/91)
* add `easement propose`, the decision on dependencies before the code exists ([6429131](https://github.com/mokevnin/easement/commit/642913127540958f3705b7cd336431d9fc1758b3)), closes [#93](https://github.com/mokevnin/easement/issues/93)
* add `easement review`, what a change to the policy permits, forbids and accepts ([3a0a5bf](https://github.com/mokevnin/easement/commit/3a0a5bf04498392338ef9e7a0f7bd8ec8001fdd8)), closes [#98](https://github.com/mokevnin/easement/issues/98)
* add named surfaces, paths of a component open only to the consumers they list ([4d07ed5](https://github.com/mokevnin/easement/commit/4d07ed58429710496c8cfbd8c08ee3c5b5e1e7a0)), closes [#96](https://github.com/mokevnin/easement/issues/96)
* add policy examples, decisions `check` pins with no code behind them ([0f8a0dc](https://github.com/mokevnin/easement/commit/0f8a0dc7b7ce57a5467ccc548d569a727c24ca1e)), closes [#94](https://github.com/mokevnin/easement/issues/94)
* answer `explain` and `why` in JSON, and give violations their chain and provenance ([88ef1de](https://github.com/mokevnin/easement/commit/88ef1dea35c2327f2240f91d0ccde29658b545ac)), closes [#95](https://github.com/mokevnin/easement/issues/95)
* define a dependency as a name the code writes, in every language ([7e7722b](https://github.com/mokevnin/easement/commit/7e7722b8f9f3f40cb0096b824c48933afe667aea)), closes [#104](https://github.com/mokevnin/easement/issues/104)
* judge what a change to the policy causes under `--diff` ([e2e46a8](https://github.com/mokevnin/easement/commit/e2e46a824e1aa4f7d06cc8f2c3e5a2268825cb90))
* list the policy examples a change adds, removes or changes in `review` ([7971924](https://github.com/mokevnin/easement/commit/7971924cc893657dd2e6823f4296abbcd54b679c)), closes [#98](https://github.com/mokevnin/easement/issues/98)
* settings of an indexer go under `indexers`, which the core does not read ([7310aec](https://github.com/mokevnin/easement/commit/7310aec03eebaee7eba467aac62181aff9264dea)), closes [#105](https://github.com/mokevnin/easement/issues/105)
* the stats of a report count; the gaps are said once, in `evidence_gaps` ([744b045](https://github.com/mokevnin/easement/commit/744b0451010baff25b70de4cb4b4cec91136e0d0))


### Bug Fixes

* `--diff` reads the git diff with unidiff, so a path with a space is found ([18f32a1](https://github.com/mokevnin/easement/commit/18f32a133410185f57ed2369916278b56ca53126))
* `forbid_symbol` names a member of a type in Go, TypeScript, Python and Rust ([8757f66](https://github.com/mokevnin/easement/commit/8757f66638d30f332447d54d4e9c069ff5fe364f))
* a `forbid_symbol` name that can never fire fails as `unmatched` ([79c2a4a](https://github.com/mokevnin/easement/commit/79c2a4a4a33d42fe1d4deeb47a15df69220d4e28))
* a Cargo workspace dependency is not importable where no package lists it ([96632ee](https://github.com/mokevnin/easement/commit/96632ee9c419caa6f110cd418b02a6617bf4f985))
* a scope narrows the violations reported, never what was read ([59c2938](https://github.com/mokevnin/easement/commit/59c2938684988f3fd8c931f2611610d8cabd7231)), closes [#91](https://github.com/mokevnin/easement/issues/91)
* check again under `--watch` when an `extends` file outside the root changes ([9ebb052](https://github.com/mokevnin/easement/commit/9ebb052eee586c08f84057492ea03060f862d78e)), closes [#92](https://github.com/mokevnin/easement/issues/92)
* count a file whose lines were only deleted as changed under `--diff` ([b452aa0](https://github.com/mokevnin/easement/commit/b452aa0db1ac70ce12c2c5a30126841dd5d720ea)), closes [#97](https://github.com/mokevnin/easement/issues/97)
* cut a module name where the importing file's language cuts it ([ccec7ed](https://github.com/mokevnin/easement/commit/ccec7edca8075fc1416cfc12e7a99064b2004b2c))
* every command says the gaps of its run, in one way ([67a9548](https://github.com/mokevnin/easement/commit/67a9548ca00e335ad0633ce9df4795c222a1bbe8))
* every command that reads the indexes takes `--no-cache` and `--cache-only` ([9dd3b96](https://github.com/mokevnin/easement/commit/9dd3b96088018b908d47ca9b3ca457baf51b55c3))
* fail a `part` that matches no part as `unmatched` ([1f06f53](https://github.com/mokevnin/easement/commit/1f06f53b07747d086cab874354c5f132734ee78a))
* fail under `--diff` what a rule taken out of `warn` now fails ([f796573](https://github.com/mokevnin/easement/commit/f796573c32c91e99904052480c7a14e644682876)), closes [#97](https://github.com/mokevnin/easement/issues/97)
* judge no import of a `target_only` source in `propose` ([a4bd04e](https://github.com/mokevnin/easement/commit/a4bd04eed83c90662203e23b99bc552895ba78fd))
* name the count, not the files, in a symbols gap, and narrow the npm one by scope ([14d07de](https://github.com/mokevnin/easement/commit/14d07dedb88839792ff6d80d1e23e11bdbd1d93a))
* read a quoted name in a bracketed SCIP descriptor ([13e4cba](https://github.com/mokevnin/easement/commit/13e4cba04d1e02e20e07564acc2bfb2cac5c25b2))
* report symbol rules over languages without verified uses as an evidence gap ([3a75f19](https://github.com/mokevnin/easement/commit/3a75f191bfee7ef4fdfa606af88138fd9936bab6))
* SARIF and JUnit say what a run leaves out ([3a69fb8](https://github.com/mokevnin/easement/commit/3a69fb8528f05132eaad11b83f7c8c4daf191864))
* the output says dependency and surface, as GLOSSARY.md does ([59fa067](https://github.com/mokevnin/easement/commit/59fa0673e52740aab587cc41444cb0cad3b89e78))

## [0.4.14](https://github.com/mokevnin/easement/compare/v0.4.13...v0.4.14) (2026-11-05)


### Features

* name the files or extensions of every evidence gap ([2968225](https://github.com/mokevnin/easement/commit/2968225b54aabcdcbfc59753ed35daee87bbe35f)), closes [#88](https://github.com/mokevnin/easement/issues/88)
* report narrowing, evidence gaps and accepted gaps separately ([b84d399](https://github.com/mokevnin/easement/commit/b84d3991ea39e82aa3f757c0b9c616680aef032e)), closes [#88](https://github.com/mokevnin/easement/issues/88)


### Bug Fixes

* judge every dependency through one dependency decision ([4803b1a](https://github.com/mokevnin/easement/commit/4803b1a86bbd19bb7bf19c92a60381b6c412ecb0)), closes [#87](https://github.com/mokevnin/easement/issues/87)
* judge every dependency through one dependency decision ([#100](https://github.com/mokevnin/easement/issues/100)) ([4803b1a](https://github.com/mokevnin/easement/commit/4803b1a86bbd19bb7bf19c92a60381b6c412ecb0))

## [0.4.13](https://github.com/mokevnin/easement/compare/v0.4.12...v0.4.13) (2026-11-04)


### Bug Fixes

* take every module that Rust test code declares as test code ([#82](https://github.com/mokevnin/easement/issues/82)) ([4f16454](https://github.com/mokevnin/easement/commit/4f164542dad69b7461bb395eb9663b72c3c009c0))
* take Rust benchmarks and examples into the tests component ([#81](https://github.com/mokevnin/easement/issues/81)) ([796a519](https://github.com/mokevnin/easement/commit/796a51974234f5a03d5f1468faf5d6ef62ab7a7b))

## [0.4.12](https://github.com/mokevnin/easement/compare/v0.4.11...v0.4.12) (2026-11-04)


### Bug Fixes

* reach through the other files of a Go package ([#79](https://github.com/mokevnin/easement/issues/79)) ([2abdc30](https://github.com/mokevnin/easement/commit/2abdc30e19020c9c1e0529d2502520b8c2fdc6ab))

## [0.4.11](https://github.com/mokevnin/easement/compare/v0.4.10...v0.4.11) (2026-11-04)


### Bug Fixes

* count a suppression only where the directive begins a comment ([#74](https://github.com/mokevnin/easement/issues/74)) ([c1c4a85](https://github.com/mokevnin/easement/commit/c1c4a855a1034f0191dfec709627f7e0896d52ab))
* take Rust integration tests into the tests component ([#75](https://github.com/mokevnin/easement/issues/75)) ([9897eab](https://github.com/mokevnin/easement/commit/9897eab9aee0110506042934489306e292427978))

## [0.4.10](https://github.com/mokevnin/easement/compare/v0.4.9...v0.4.10) (2026-11-03)


### Features

* take a cfg(test) module file, a cfg(test) file and cfg(all(test, ...)) as test code ([f538508](https://github.com/mokevnin/easement/commit/f53850821e9270ec06d6dd0d7ccb5c8df7ef9fd3))


### Bug Fixes

* place test-only modules as Rust does, and say what cannot be placed ([8b60eaa](https://github.com/mokevnin/easement/commit/8b60eaa45c47affc9dc2add602f0e46649034e8f))

## [0.4.9](https://github.com/mokevnin/easement/compare/v0.4.8...v0.4.9) (2026-11-03)


### Features

* check the dependencies a Cargo manifest declares with packages: true ([4292705](https://github.com/mokevnin/easement/commit/429270570f779ce8f1ac554c63b452526e5279fc))
* follow the definitions a call references in `must_not_reach` with `reach: symbols` ([d0d10ae](https://github.com/mokevnin/easement/commit/d0d10ae319c2a3e2d180638063241d8ec2ca60ae)), closes [#58](https://github.com/mokevnin/easement/issues/58)
* let a component own a Cargo feature of a manifest with part ([eeaccdd](https://github.com/mokevnin/easement/commit/eeaccddef4f4e14a79a008e81f16ebf71b9930c9))
* read package.json and go.mod with packages: true ([0a033ed](https://github.com/mokevnin/easement/commit/0a033ed06693cd8b605c2730dc48f08c35cd05a0))
* say which test code of a Rust file tests: auto could not tell apart ([5da4f31](https://github.com/mokevnin/easement/commit/5da4f312be5178ad035ed8164a6bc695b520ce80))
* take #[cfg(test)] code of a Rust file into the tests component of tests: auto ([4b02319](https://github.com/mokevnin/easement/commit/4b023198fb1327ab86d55ac48bc4f6b55ff0b3f2))


### Bug Fixes

* judge the part a dependency reaches, and say a part that nothing has ([38803c4](https://github.com/mokevnin/easement/commit/38803c420a4615d51ffab244c464f1bc918f26ec))
* keep cfg(test) parts to real items and to tests: auto ([4496621](https://github.com/mokevnin/easement/commit/44966218215aa90356b24a496bc8e58aeeb72f6e))
* read package.json and go.mod as the tools do, and say what cannot be read ([0669fbb](https://github.com/mokevnin/easement/commit/0669fbb4dfa856ef3ef4e21601d18bb01060e137))
* say which manifests packages could not read, and keep manifests from judging them ([2d429cc](https://github.com/mokevnin/easement/commit/2d429cca6e5ca2f6a0701484accf0b371ef98ab8))

## [0.4.8](https://github.com/mokevnin/easement/compare/v0.4.7...v0.4.8) (2026-11-03)


### Features

* forbid_symbol at the top level, for every checked file ([8e4d64c](https://github.com/mokevnin/easement/commit/8e4d64cf58c4dd35a7077f2f4aa81d89081b1465)), closes [#57](https://github.com/mokevnin/easement/issues/57)


### Bug Fixes

* say how to judge an npm package under a nameless package.json ([d6e5e98](https://github.com/mokevnin/easement/commit/d6e5e9876369c060c54127b57f2aeb0fa0615a32)), closes [#66](https://github.com/mokevnin/easement/issues/66)

## [0.4.7](https://github.com/mokevnin/easement/compare/v0.4.6...v0.4.7) (2026-11-03)


### Features

* say when a `forbid_symbol` name can never fire because a deeper namespace defines it ([580369f](https://github.com/mokevnin/easement/commit/580369f54e38aa32db37f2b9c8f3b826f5ede352)), closes [#56](https://github.com/mokevnin/easement/issues/56)


### Bug Fixes

* name `unowned` and a component as the way out of an `excluded` import ([eb36493](https://github.com/mokevnin/easement/commit/eb36493a0320bb349e39512ff620199731509616)), closes [#59](https://github.com/mokevnin/easement/issues/59)

## [0.4.6](https://github.com/mokevnin/easement/compare/v0.4.5...v0.4.6) (2026-11-03)


### Bug Fixes

* **deps:** update scip-typescript to 0.6.1 and the cargo lockfile ([5a33e94](https://github.com/mokevnin/easement/commit/5a33e949f3f1c49fc413781bceabe6f69d7e61ca))

## [0.4.5](https://github.com/mokevnin/easement/compare/v0.4.4...v0.4.5) (2026-11-02)


### Bug Fixes

* flag unjudged names only in the components that `forbid_symbol` covers ([e137329](https://github.com/mokevnin/easement/commit/e137329133dc3fb8469e0e9dc8525ac1f30c9771))
* judge a package that a nameless package.json hides by the import that names it ([ebc231c](https://github.com/mokevnin/easement/commit/ebc231c4699e1aa3ab09a286a6b8d3c8188495c4)), closes [#55](https://github.com/mokevnin/easement/issues/55)
* say when `forbid_symbol` could not judge the names of an unnamed npm package ([4953208](https://github.com/mokevnin/easement/commit/49532082a7534244683dbc09bc465972d03d9366))
* take an import into a nameless project for its own code, not a package ([0309dcc](https://github.com/mokevnin/easement/commit/0309dccb98de691315b4cb8cbf26c0905101df0d))

## [0.4.4](https://github.com/mokevnin/easement/compare/v0.4.3...v0.4.4) (2026-11-02)


### Bug Fixes

* hash an indexer again when it is replaced in place during a run ([6d7a8a0](https://github.com/mokevnin/easement/commit/6d7a8a048dc2b4d17dfff89ca3ca5cf0a2029f7e))
* keep suggest, impact, why and baseline careful about files in `uncovered_ok` ([dce2298](https://github.com/mokevnin/easement/commit/dce22982ebad7947b7ddb3f0866c27453bb8f86d))
* keep the reasons for a partial verdict and the cache note in `--format github` ([f5fb41a](https://github.com/mokevnin/easement/commit/f5fb41a30103c492dbc66e19a972afb3a40fb9be))
* key an indexer behind rustup or a mise shim by the binary it runs ([6932249](https://github.com/mokevnin/easement/commit/69322496f39d2acc8ad646642ff5cb15c4be57fc))
* key cached indexes on the environment and say when a report used them ([3e05d01](https://github.com/mokevnin/easement/commit/3e05d01c1e0851012c2accab573e75a1bfa8266c))
* key each indexer on the variables verified to change its index ([8ac1093](https://github.com/mokevnin/easement/commit/8ac1093246805e9e3bf8d3db8a374e1eef4acaee))
* make a run narrowed by design a notice in `--format github`, not a warning ([fbfa525](https://github.com/mokevnin/easement/commit/fbfa525895e507ecbdc9d9fafffd1dab92e79208))
* say in impact, why, suggest, graph and baseline when the indexes came from the cache ([bf1f2c5](https://github.com/mokevnin/easement/commit/bf1f2c5e85ad1f677e26f50fbf3746a6940ac2c4))

## [0.4.3](https://github.com/mokevnin/easement/compare/v0.4.2...v0.4.3) (2026-11-01)


### Features

* check `defines` globs when the config loads, and name every claimant ([afb623b](https://github.com/mokevnin/easement/commit/afb623b2d2c3e5e32c9870ccac975bdb9a135632))
* check the dependencies against a Mermaid diagram with `diagram` ([c6bd32a](https://github.com/mokevnin/easement/commit/c6bd32ae8c0fdb4999812e0d6ea2cd49892b3e2e))
* claim type names for a component with `defines` ([2957d52](https://github.com/mokevnin/easement/commit/2957d520afe48c55b02138e586c63b6d229a8b65))
* fail on an import the nearest manifest does not declare ([ae59f31](https://github.com/mokevnin/easement/commit/ae59f31b97d511cca9c24fb706051c3dce3b1b87))
* judge C# and PHP in `unused`, and know Swift and PHP in `$std` ([fbfdaf0](https://github.com/mokevnin/easement/commit/fbfdaf011981e66a11a3d22008baeb3f62a25447))
* judge only the packages a file names in `manifests` ([fd44bbc](https://github.com/mokevnin/easement/commit/fd44bbcb7b0cdcdf1f6b3bdb13fe4b8e66d5a2f2))
* read every Mermaid link in `diagram`, and leave tests and common out ([c8cf81f](https://github.com/mokevnin/easement/commit/c8cf81fb631f6e889db33d93629607be798de1df))

## [0.4.2](https://github.com/mokevnin/easement/compare/v0.4.1...v0.4.2) (2026-11-01)


### Features

* report files that no entry reaches as `unused` ([52db364](https://github.com/mokevnin/easement/commit/52db3648de5a2dc2f2f95904fca4880e602cc279))

## [0.4.1](https://github.com/mokevnin/easement/compare/v0.4.0...v0.4.1) (2026-11-01)


### Features

* name the path to the field in a config error ([e29c46f](https://github.com/mokevnin/easement/commit/e29c46f0a2ca2fe650e99fd04745c40b71bb6b90))
* suggest the component a mistyped name is likely meant to be ([a9f92b7](https://github.com/mokevnin/easement/commit/a9f92b7072a094f4e30e1ec350124a007223383e))


### Bug Fixes

* key the index cache by BLAKE3 instead of `DefaultHasher` ([cd77bcd](https://github.com/mokevnin/easement/commit/cd77bcd870b939ef5576caf619fb4a545fcc2385))

## [0.4.0](https://github.com/mokevnin/easement/compare/v0.3.7...v0.4.0) (2026-11-01)


### ⚠ BREAKING CHANGES

* a checked file that imports an excluded file now fails with an `excluded` violation. Generated code the project imports belongs in `target_only`, not `exclude`.
* `easement baseline` over an existing baseline no longer records new violations; it lists them and exits 1. Pass `--accept-new` to legalize them.
* a config with a component that matches no file, which used to pass, now fails with an `unmatched` violation. This includes a component whose files are all hidden by `exclude` or `.gitignore`. Such a component fails every `easement check`, `--diff` included, until its pattern is fixed or it is marked `optional: true`.

### Features

* add `$std` for the standard library in external module rules ([d06a495](https://github.com/mokevnin/easement/commit/d06a495c41aa08b6f59588c364fa7960fcb7b37f))
* add `impact`, `why` and `graph --focus` for planning a change ([ea02ea5](https://github.com/mokevnin/easement/commit/ea02ea576367f678a6c7ed5b30db66ef5101865a))
* add `must_not_reach` to forbid a dependency through any chain ([963eacf](https://github.com/mokevnin/easement/commit/963eacf9c3e3711ac9e8fb2ce8eec5ab5d49ca8b))
* add sibling and closed layers ([c4bff86](https://github.com/mokevnin/easement/commit/c4bff861721f7b58e056c6f72dafd98f7dede7dc))
* fail on a component pattern that matches no file ([cf4a776](https://github.com/mokevnin/easement/commit/cf4a776176ac8379bdda3d52502217403ec0be32))
* fail on an import of a file that `exclude` hides ([c099eb6](https://github.com/mokevnin/easement/commit/c099eb6e34bfc4bd870cbbd2b804d3f49ffe6497))
* keep imports within one slice with named placeholders ([5ddfd18](https://github.com/mokevnin/easement/commit/5ddfd1892222df47ba1ee0f8687ff8823a22b143))
* let `$std` cover old Python and the JDK, and say when it cannot judge ([2846b44](https://github.com/mokevnin/easement/commit/2846b4453dd7ff1bc6aabe56b5051eb735aafef0))
* let `easement baseline` only shrink an existing baseline ([f20a4d7](https://github.com/mokevnin/easement/commit/f20a4d76097fc78e74b0145afca4e30cee88116e))
* name an owner for a component to ask about its rules ([2fdc23b](https://github.com/mokevnin/easement/commit/2fdc23b66e20aa6f9b73462eea4f3d6355c67a38))
* report the rules in `warn` as warnings that do not fail ([40e4791](https://github.com/mokevnin/easement/commit/40e47914d372bcbc299ff80e731416c097698d4d))
* say which layer stops a dependency, and show owners in more places ([42b801a](https://github.com/mokevnin/easement/commit/42b801a27bee3e8ed21c1ea1998d0b32fb64f4af))


### Bug Fixes

* key cycle, limit and requirement baseline entries by what they are about ([acf2f81](https://github.com/mokevnin/easement/commit/acf2f81cd9c18d10da6ba2794cac696209a9e559))
* never suggest allowing an import that the config forbids ([e445de2](https://github.com/mokevnin/easement/commit/e445de2fcb176211413c2abb1cf36d24ab59b5b1))

## [0.3.7](https://github.com/mokevnin/easement/compare/v0.3.6...v0.3.7) (2026-10-30)


### Bug Fixes

* report Ruby constants and methods to forbid_symbol and public_symbols ([#44](https://github.com/mokevnin/easement/issues/44)) ([b21f15d](https://github.com/mokevnin/easement/commit/b21f15d8353a520c36c1043c92cb09b033fd03aa))

## [0.3.6](https://github.com/mokevnin/easement/compare/v0.3.5...v0.3.6) (2026-10-29)


### Features

* add check --cache-only, which runs no indexer and exits 3 on a miss ([#42](https://github.com/mokevnin/easement/issues/42)) ([632a2c8](https://github.com/mokevnin/easement/commit/632a2c88b48b1c575197a930a914afdebdf6157c))

## [0.3.5](https://github.com/mokevnin/easement/compare/v0.3.4...v0.3.5) (2026-10-29)


### Bug Fixes

* key the index cache on the indexer's content, not its install time ([#40](https://github.com/mokevnin/easement/issues/40)) ([3067fdd](https://github.com/mokevnin/easement/commit/3067fdd2b75596d44319e51ac6ff6a60c6b74c58))

## [0.3.4](https://github.com/mokevnin/easement/compare/v0.3.3...v0.3.4) (2026-10-29)


### Features

* key each index on the files its indexer reads ([#38](https://github.com/mokevnin/easement/issues/38)) ([49d0ea5](https://github.com/mokevnin/easement/commit/49d0ea530c5eee180af6996540fd199ec948c709))

## [0.3.3](https://github.com/mokevnin/easement/compare/v0.3.2...v0.3.3) (2026-10-28)


### Bug Fixes

* build the Linux packages without the test indexers ([#35](https://github.com/mokevnin/easement/issues/35)) ([25ff897](https://github.com/mokevnin/easement/commit/25ff8979852896fbf2d80bf69dbea734d92c8bf6))

## [0.3.2](https://github.com/mokevnin/easement/compare/v0.3.1...v0.3.2) (2026-10-28)


### Features

* resolve Ruby through the scip-ruby indexer ([#33](https://github.com/mokevnin/easement/issues/33)) ([81ea7ca](https://github.com/mokevnin/easement/commit/81ea7ca5fa06d95d502c203bd8fee02ddd68b32f))

## [0.3.1](https://github.com/mokevnin/easement/compare/v0.3.0...v0.3.1) (2026-10-26)


### Features

* key the index cache on content so other checkouts reuse it ([#30](https://github.com/mokevnin/easement/issues/30)) ([a3aa08a](https://github.com/mokevnin/easement/commit/a3aa08a0ef21647c577a6e691da42ebc1f4a7e44))
* target_only keeps generated files in the graph as targets ([#32](https://github.com/mokevnin/easement/issues/32)) ([480896d](https://github.com/mokevnin/easement/commit/480896d106b680847aec82cb73a3864cc1e65590))
* uncovered_ok takes a list of globs, suggest --check fails on unused allowances ([#29](https://github.com/mokevnin/easement/issues/29)) ([5998f7d](https://github.com/mokevnin/easement/commit/5998f7d73a857b08c09eee7258cb19f0befe6331))

## [0.3.0](https://github.com/mokevnin/easement/compare/v0.2.0...v0.3.0) (2026-10-26)


### ⚠ BREAKING CHANGES

* report every use of a violation, not the first one per file ([#25](https://github.com/mokevnin/easement/issues/25))
* Java and C# projects need scip-java (with a JDK and Maven or Gradle) and scip-dotnet (with the .NET SDK) installed. The `dynamic imports not analysed` count and `stats.dynamic` are gone. Violations name the target file rather than the package or namespace.
* PHP projects need scip-php installed and `composer install` done. Functions and constants brought in with `use function` and `use const` are no longer dependencies. Violations name the target file rather than the specifier as written.

### Features

* easement doctor checks that the indexers can run ([#22](https://github.com/mokevnin/easement/issues/22)) ([b627c8e](https://github.com/mokevnin/easement/commit/b627c8e4cc66d6240da7534c14ac41cfa132d541))
* easement suggest --patch drops unused allowances as a patch ([#27](https://github.com/mokevnin/easement/issues/27)) ([b85a532](https://github.com/mokevnin/easement/commit/b85a532b091dce27537261cc428e9b96cf75d56c))
* index Go files behind build tags with go.tags ([#24](https://github.com/mokevnin/easement/issues/24)) ([a2e818d](https://github.com/mokevnin/easement/commit/a2e818dbbb37e9d8f50b649ed6827a01c0f1d0e1))
* keep the indexers' output between runs ([#20](https://github.com/mokevnin/easement/issues/20)) ([1a82708](https://github.com/mokevnin/easement/commit/1a8270869f5b7e18f35bd0f72932733e66602b4d))
* list the files no indexer covered and mark a partial verdict ([#21](https://github.com/mokevnin/easement/issues/21)) ([c1def55](https://github.com/mokevnin/easement/commit/c1def55eb0946620d5005d0560e8d0a68df9ac93))
* mark a run that judges only a part as partial ([#26](https://github.com/mokevnin/easement/issues/26)) ([231ef3a](https://github.com/mokevnin/easement/commit/231ef3a669d2d44335dccb522e9e9546f0c838dd))
* resolve Java and C# through SCIP indexers ([#18](https://github.com/mokevnin/easement/issues/18)) ([b38442f](https://github.com/mokevnin/easement/commit/b38442fa7c0a585356e77865f4682c688b1b26dd))
* resolve PHP through the scip-php indexer ([#16](https://github.com/mokevnin/easement/issues/16)) ([bbbe3ef](https://github.com/mokevnin/easement/commit/bbbe3ef693738fd5bdb2d4067ea3df69c95a20d6))
* share a part of a rule between components with YAML merge keys ([#23](https://github.com/mokevnin/easement/issues/23)) ([dcdfdcf](https://github.com/mokevnin/easement/commit/dcdfdcfbfb87d49836bfdead32ba4d6e5be7f52c))


### Bug Fixes

* report every use of a violation, not the first one per file ([#25](https://github.com/mokevnin/easement/issues/25)) ([e2e8b12](https://github.com/mokevnin/easement/commit/e2e8b125c7237e87ec37751eb8478cc2a6fb5fd4))
* suggest does not trust a missing import next to unindexed files ([#19](https://github.com/mokevnin/easement/issues/19)) ([a24f93d](https://github.com/mokevnin/easement/commit/a24f93da3c451f49a8010087f0cc1c5720b704ac))

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
