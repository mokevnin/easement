# Languages and indexers

[Documentation](README.md) · [Project overview](../README.md)

- [Languages](#languages)
  - [Index cache](#index-cache)
  - [Indexers](#indexers)
- [How it works](#how-it-works)

## Languages

| Language | Extracts | Resolves |
|---|---|---|
| Go | everything `scip-go` resolves | per `go.mod`, as the Go toolchain does |
| TypeScript / JavaScript | everything `scip-typescript` resolves: `import`, `export … from`, `import()`, `typeof import()`, `require()` in JavaScript files | the TypeScript compiler's own rules: `tsconfig` `paths`, package `exports`, workspaces, `.js` → `.ts` |
| Python | everything `scip-python` resolves | the interpreter's rules, as `pyright` sees them: absolute and relative imports, packages, the installed environment |
| Java | everything `scip-java` resolves: a class named in an import or used by its qualified name | the compiler's, through the Maven or Gradle build |
| Rust | everything `rust-analyzer scip` resolves | the crate graph from `cargo metadata`: `crate::`, `self::`, `super::`, workspace members, `mod x;` |
| C# | everything `scip-dotnet` resolves: a type named in a `using` or used by its qualified name | the compiler's (Roslyn), through the project or solution |
| Ruby | everything `scip-ruby` resolves: a class or module named by a constant (`Shop::Domain::Order`) | Sorbet's: constants resolved through the project's own files and its RBIs |
| Kotlin | `import`, `import … as`, `import ….*` | top-level declarations indexed by the `package` of their file, like Java; a file may hold several classes |
| PHP | everything `scip-php` resolves: a class used in a `use` statement or by its qualified name | the Composer autoloader: PSR-4 and classmap, installed packages from `vendor/` |
| Swift | `import` | modules in the SwiftPM layout: `Sources/<Module>` next to a `Package.swift` |
| Svelte | `import`, `export … from` and a literal `import()` in each `<script>` block | relative paths, `package.json` subpath imports, `tsconfig.json` `paths` (and SvelteKit's generated `.svelte-kit/tsconfig.json`), `.js` → `.ts` |

Every language but Kotlin, Swift and Svelte comes from a SCIP index, which is exact but has its own limits.

All indexed languages:

- A dependency is a name a file writes that is defined in another file or in an external package: a module (a Go package, a TypeScript file, a Python or Rust module, a Java, C# or PHP class, a Ruby constant), or a name at the top level of one (a class, a function, a constant). A member reached through a value, a method or a field, is not a dependency on its own: `chunk.make().size()` depends on `chunk`, not on the file that defines `size`. Every reference, members included, is still a use of a name, which `forbid_symbol` and `public_symbols` read.
- A name reached through a re-export (a Rust `pub use`, a TypeScript `export … from`, a Python `__init__` import) is a dependency on the file that defines it, because the index points the use at the original: `chunk::Block`, re-exported from `blob`, depends on `blob`. The re-export is evidence of it too: when the line of the use names a module of a third component that re-exports the name, by the name, whole (`export *`), or through a module of its own it re-exports whole, or when the line names none, the file imports a module of one whole on a line of its own (`use chunk::*`) and that module writes the name itself, the violation names that component (`gw -> blob is not allowed (written as blob/src/lib.rs, re-exported by chunk)`, `reexported_by` in JSON), and its `evidence` holds every line of that component that writes the name or names a module of the component the name is in. So `--diff` reports the use when only the re-export changed, and also when another of those lines did. Through a chain of re-exports (`gw -> chunk -> mid -> blob`) the violation names the component the use names, and its `evidence` also holds the line of every module on the way that re-exports the name; a glob import of a glob re-export (`use chunk::*` of a `pub use blob::*`) names no re-exporter, and a C# `global using Block = Blob.Block;` in `chunk` is found only when the file of the use names a module of `chunk` too, since nothing at the use points at it. Surfaces (`public`, named surfaces) judge such a name at the file by which it enters the component that defines it, so a name a public file re-exports is public. Through a re-export in a third component, that is the first re-exporting module of the defining component on the way, else the file that defines the name, and it must be open to the file that writes the name: `chunk`, open to `blob`'s `core`, re-exporting a name of `core` does not open it to `gw` (`gw -> blob: blob/src/core.rs is blob's core surface, open to chunk (written as blob/src/core.rs, re-exported by chunk)`, `privacy`), while a name `chunk` re-exports from `blob`'s public `api` passes. The way is followed through every module that re-exports the name, by the name or by a glob (`export *`, `pub use blob::*`). A glob re-export of a glob re-export, or one reached by a glob import, leaves no way the index shows, and the name is then not judged by surfaces.
- An alias is a name whose definition the index marks as standing for other names: a type alias (Go `type Block = blob.Block`, Rust `pub type`, TypeScript `export type`) or a constant the index relates to another name (Ruby `Block = Blob::Block`). A use of it depends on the file of the alias and also on the file of every module or top-level name of the project its definition names, aliases among them followed in turn (`type Two = Block`), and it is a use of each of those names too, for `forbid_symbol` and `public_symbols`. Slices, generics and function types count whole, so `type Blocks = []blob.Block` stands for `blob.Block` and `type H = func(a.X) b.Y` for both names. A member the definition names is not followed. The violation names the component that defines the alias the line writes (`gw -> blob is not allowed (written as blob/blob.go, re-exported by chunk)`, `reexported_by` in JSON), and its `evidence` holds the definition of every alias on the way, so `--diff` reports the use when only an alias changed. An alias defined in an excluded file is followed too. Surfaces (`public`, named surfaces) judge a name the file reaches only through aliases at the file by which the aliases enter the name's component: the first alias on the way defined in it, else the file that defines the name, which must be open to the file: an alias in `blob`'s public `api` opens what it stands for, an alias in `chunk` of a file of `blob` outside its surfaces fails as `privacy`. The limits: a variable that holds another name is a value, not an alias, so a TypeScript `export const X = blob.Block` and any Python assignment (`TypeAlias` and `type X = …` included, which the index does not mark) hide what they hold; Kotlin and Swift `typealias` are not followed, since their adapters resolve only imports; and an alias of an external name (`type Ctx = context.Context`) adds nothing to the file that uses it, so it counts against neither `forbid_external` nor `may_use_external` there: the external module is judged at the import in the alias's own file. A C# `global using X = Blob.Block;` needs nothing of this: the index points the use at `Blob.Block` itself.
- Imports computed at run time are not reported.
- Easement says how many source files no index covered (`N files not covered by an indexer`), see [Indexers](#indexers).

TypeScript / JavaScript:

- Only files that a `tsconfig.json` includes are indexed.
- `require()` in a `.ts` file is invisible unless `@types/node` is installed (JavaScript files are fine).
- A violation names the target file (`imports src/db/d.ts`) rather than the specifier as written.
- `vi.mock('./x')` and similar string arguments are not imports to the compiler, so they are not seen.
- A module augmentation (`declare module "@tanstack/react-router" { interface Register … }`) is not a dependency of the files that import the package or write a name it already defines: they depend on the package only. A name that only the augmentation adds, and a module that only an ambient `declare module "virtual:x"` declares, are dependencies on the file that declares them.

Java and C#: a package or namespace named on its own is not a dependency. `new X()` in Java and an attribute `[X]` in C# depend on the class, which their indexers record only as its constructor; a method called on a value whose class the file never writes does not.

PHP:

- Classes (and interfaces, traits, enums) are the modules; a function or constant brought in with `use function` or `use const` is a name at the top level of a namespace, and a dependency on the file that defines it.
- `scip-php` writes `index.scip` into the project directory for a moment, so easement refuses to run when one is already there.

Python:

- Third-party modules have to be installed in the environment the indexer sees, or their imports are not seen. There is no check for that yet, unlike `node_modules`.
- A module reference is matched to its file by the end of its dotted name, so an import that two modules could satisfy is skipped.

Ruby:

- `require` and `require_relative` are strings, so only the constants a file uses count, as a dependency on the file that defines the class (the deepest constant of `Shop::Domain::Order`). `Order.find` and `Order.new` depend on `Order` through the constant they are called on; a method called on a value is a use of its class, not a dependency.
- `scip-ruby` gives the standard library's classes and the project's the same package, and a gem's constants appear only when Sorbet has an RBI for the gem, so `forbid_external` is blind to Ruby. The same holds for `forbid_symbol`: `ActiveRecord.Rollback` is found once the gem's RBI is in `sorbet/rbi` (`tapioca gems`).
- A method call is seen only where Sorbet knows the type of the receiver: a constant, the result of `new`, or a value whose `sig` names its class; a call on an untyped value is not a use. `Order.new` is Sorbet's `Class#new`, so forbid the class, not `new`.
- A method inherited or mixed in is named by where it is defined, not by the receiver: `Post.transaction` in Rails is `ActiveRecord::Transactions::ClassMethods.transaction`, and for `public_symbols` a method of a parent class or a concern is a use of that parent or concern.
- A module that several files reopen (a namespace such as `Shop`) belongs to no one component, so `public_symbols` judges the classes and modules that one file defines. A module function counts as a use of its module: `Shop::Db.helper` uses `Db`.

Rust: the dependency of a line is the deepest module it names, `std`, `core` and `alloc` all count as `std`, and `rust-analyzer` must be installed (`rustup component add rust-analyzer`).

Kotlin is still read with tree-sitter: `scip-java`'s Kotlin plugin fails with a `NoClassDefFoundError` on every Kotlin compiler from 1.9 to 2.3 that I tried, so there is no indexer to use yet. References by fully qualified name without an import are not seen. Swift modules declared with a custom `path:` in `Package.swift` are not found.

Svelte components are read with tree-sitter too: no SCIP indexer reads a `.svelte` file. The imports of its `<script>` blocks resolve as Vite and TypeScript resolve them: a relative path, a subpath import of the nearest `package.json` (`#lib/*`), or a `paths` alias of its `tsconfig.json` and of the `.svelte-kit/tsconfig.json` SvelteKit generates; `./api.js` names `./api.ts`, and a Vite query (`?raw`) names the file before it. A package that depends on `@sveltejs/kit` gets SvelteKit's default `$lib` (`src/lib`) even before `.svelte-kit/` is generated, as on a fresh checkout. A `tsconfig.json` `extends` is not followed, nor `kit.alias` or `kit.files.lib`: an import that looks like project code (relative, `#…`, or a `$…` alias that is not one of SvelteKit's `$app/`, `$env/` and `$service-worker` modules) and names no file is an `unresolved_imports` evidence gap, never an external module; a route's generated `./$types` is the one exception. A TypeScript file's import of a `.svelte` file is a dependency on the component too: the adapter reads the TypeScript files' imports of components, and adds the ones the index did not point at the component.

Adding a language starts with its entry in `src/lang/registry.rs`: its extensions, test files, standard library, whether `unused` can judge it and its manifest, which the rules read from there. An indexed language then names its indexer and the marker file it runs in (`src/indexer.rs`) and maps its symbols to module names (`src/scip/symbol.rs`). A language that has no indexer can still be read with a tree-sitter adapter (`src/lang/`): extract imports from a file, resolve a specifier to a project path or an external module. Rules, baseline and reports are language-agnostic.

### Index cache

Indexers are slow (a second or two for a small project, ten or more for a big one), so easement keeps what they write.

- **Where:** the user's cache directory (`~/Library/Caches/easement` on macOS, `~/.cache/easement` on Linux). Set `EASEMENT_CACHE_DIR` to move it, for example to one directory that every checkout, worktree or container of the project uses.
- **Key:** the content of the project, not where it is: the indexer's content and resolved path (not its install time, so a toolchain restored from a CI cache still finds the indexes), the environment variables that change what it writes (for Go `GO…` without an underscore and `CGO_*`; for Python `PYTHON*` but `PYTHONHASHSEED`, and `PIP_*`; for Java `JAVA_HOME`, `JAVA_OPTS`, `JAVA_TOOL_OPTIONS`, `JDK_JAVA_OPTIONS`, `_JAVA_OPTIONS`, `MAVEN_*`, `GRADLE_OPTS`, `GRADLE_USER_HOME` and `ORG_GRADLE_PROJECT_*`; for C# `DOTNET_*`, `MSBUILD*`, `NUGET_*` and the properties `DefineConstants`, `Configuration`, `Platform`, `TargetFramework(s)`, `RuntimeIdentifier(s)` and `LangVersion`, in any case; for Rust `RUSTFLAGS`, `RUSTC*`, `RUSTDOC*`, `CARGO`, `RUSTUP_TOOLCHAIN`, `RUSTUP_HOME`, `RUST_SRC_PATH` and `CARGO_*` but those Cargo sets for what it runs; none for TypeScript, PHP and Ruby, whose indexers read none that changes the index. MSBuild reads every variable as a property, so a C# project that reads another one in its build needs `--no-cache` after changing it), and the name and content of the project files under the directory of the index that its indexer reads and of the files directly above it (`tsconfig.base.json`, `go.work`, a root `package.json`). For Go these are the `.go` files and `go.mod`, `go.sum`, `go.work`, `go.work.sum` and `modules.txt`, a committed `vendor/` included: it is no code of the project, but scip-go type-checks against it, and a module a `replace` takes from outside the project changes `vendor/` alone; for TypeScript the sources, every JSON file and the lock files; for the other languages, whose build tools read files of their own choosing, every file but those in the directories their toolchain fills with code not the project's own (see [Indexers](#indexers)): build output such as `target/`, made from the files keyed, and an install such as PHP's `vendor/`, made from a lock file. A Go backend and a TypeScript frontend in one root keep apart: a change to the frontend reuses the Go index, a change to the docs reuses both. A fresh worktree of code that was already checked reuses the indexes, a file touched without a change does not miss, and any change to what an indexer reads runs it again.
- **Kept:** the newest 32 indexes of each directory and indexer, and 4 GiB in all: past that the least recently used indexes go, whatever directory they belong to.
- **Not looked at:** dependencies outside the project (`node_modules`, `vendor/`, the Go module cache, the Python environment), and the toolchain an indexer calls on the `PATH` (`go`, `python3`, `dotnet`, the JDK, Maven). After changing them without touching a lock file, run `easement check --no-cache`. An indexer run through rustup or a mise shim is known by the binary that rustup or mise runs in the directory of the index; other proxies (Volta, proto) are not resolved, so after switching a toolchain behind them run `--no-cache` too.

`--format json` reports `stats.indexed` and `stats.from_cache`. When an index came from the cache, the report says so, in `cached` of `--format json` and in a line of the text, with what the key leaves out and that `--no-cache` indexes again. Every command that reads the indexes (`check`, `baseline`, `impact`, `why`, `suggest`, `graph`, `review`, `propose`, `init`) takes `--no-cache` and `--cache-only`. `impact` (`cached` in its JSON), `why`, `suggest` and `baseline` say it the same way; `graph` and `suggest --patch`, whose output is a diagram or a patch, say it on stderr, with `graph` naming there the reasons its graph may be partial too.

`--cache-only` runs no indexer and exits 3 when an index is not cached. A CI job can call it first and set up an indexer's toolchain (the Go module cache, `npm install`) only on a miss; the dependencies an indexer needs are checked only when it runs, since a cached index was written with them in place.

`--jobs N` indexes N languages at once, and `--jobs cpus` one per processor. The default is one: scip-java and rust-analyzer can each take gigabytes. The directories of one language are still indexed one after another, since they share its build state, and the report is the same whatever the number. Build tools of different languages do run at once, though: a directory with both `pom.xml` and `Cargo.toml` has Maven and cargo writing into one `target/`, each in its own subdirectories; keep one job there if a build tool objects.

### Indexers

| Language | Indexer | Needs |
|---|---|---|
| Go | [`scip-go`](https://github.com/scip-code/scip-go) | a `go.mod`; the Go toolchain and the module dependencies |
| TypeScript, JavaScript | [`scip-typescript`](https://github.com/sourcegraph/scip-typescript) | a `tsconfig.json` (with `allowJs` for JavaScript); `npm install` (or pnpm, yarn) done |
| Python | [`scip-python`](https://github.com/sourcegraph/scip-python) | Python and Node; the dependencies installed in the environment |
| Rust | `rust-analyzer scip` | a `Cargo.toml` (the outermost one holds the workspace); `rustup component add rust-analyzer` |
| PHP | [`scip-php`](https://github.com/davidrjenni/scip-php) | a `composer.json` with `composer.lock` and `vendor/` from `composer install`; installed with `composer global require` |
| Java | [`scip-java`](https://github.com/sourcegraph/scip-java) | a `pom.xml` or `build.gradle(.kts)` (the outermost one holds the modules); the JDK and the build tool; it builds the project, and needs bash 4.4 or newer (not the macOS one) |
| C# | [`scip-dotnet`](https://github.com/sourcegraph/scip-dotnet) | a `.csproj` or `.sln`; the .NET SDK; `dotnet tool install --global scip-dotnet`; it restores packages |
| Ruby | [`scip-ruby`](https://github.com/sourcegraph/scip-ruby) | a `Gemfile` or `.gemspec` (the outermost one holds the project); `mise use -g github:sourcegraph/scip-ruby`; releases exist for macOS arm64 and Linux x86_64 only |

```sh
mise use -g go:github.com/scip-code/scip-go/cmd/scip-go npm:@sourcegraph/scip-typescript npm:@sourcegraph/scip-python github:sourcegraph/scip-java github:sourcegraph/scip-ruby
composer global require "davidrjenni/scip-php:dev-main#71a5b117ec4c5dd2af302e363410e604e5df309e"
```

The released `scip-php` (v0.0.2) cannot be installed any more, because every protobuf version it accepts has a security advisory, so the line above pins a commit of its main branch.

`easement check` finds the project files, runs the indexer in each of those directories and reads the result. The project files are:

- Go: `go.mod`; TypeScript: `tsconfig.json`; Rust: `Cargo.toml`;
- Java: `pom.xml`, `build.gradle`; C#: `*.csproj`, `*.sln`; Ruby: `Gemfile`, `*.gemspec`; PHP: `composer.json`;
- Python: `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`, or else the project root.

These are errors (exit code 2), never a quiet gap:

- a missing indexer;
- a failed run (add a directory that is not part of the project to `exclude`);
- a `package.json` with dependencies but no `node_modules`.

Source files that no project file covers are counted in the report as `files not covered by an indexer`. The directories a language's toolchain fills with code that is not the project's own are not the project's code: their files need no indexer, and a marker in them (`go.mod`, `composer.json`) is no project to index. These are `vendor` and `testdata` for Go, `vendor` for PHP and Ruby, `target` and `build` for Java and `bin` and `obj` for C#, wherever they lie; `node_modules` is never walked at all. Without installed packages the indexer cannot see third-party modules, so `forbid_external` would silently pass.

`indexers` holds what an indexer is run with, by the language it reads; an unknown language or setting fails when the policy is loaded, and a change to it is a change to what is scanned. For Go, a file behind a build constraint (`//go:build race`) is built, and so indexed, only for the tags the indexer is given; the others are listed as not covered. `tags` names the tags to index with (they are passed as `-tags` through `GOFLAGS`, and are part of the key the cached index is stored under):

```yaml
indexers:
  go: { tags: [race, e2e] }
```

For TypeScript a file is covered when a `tsconfig.json` includes it, so generated code outside `include` is listed as not covered: add it to `include`. A `tsconfig.json` in a subdirectory is indexed as a project of its own, and an import from there into a file of another project is not seen, so prefer one `tsconfig.json` that includes what should be checked together.

Those files are named in the text report under `Evidence gaps` (the first twenty; all of them in `evidence_gaps` of `--format json`).

A file the indexer read can still miss an import: when its package does not type-check or two packages import each other, scip-go leaves the import out of the index, and the dependency it writes would not be judged. So easement compares the imports a Go file writes with those the index resolved, and each one the index lacks is an `unresolved_imports` evidence gap. Its `places` give the `file`, `line` and `specifier` (the module as the import spells it) of every one of them; the `message` names the first three and counts the rest, and the text report lists them as `` file:line (`specifier`) `` where it lists the files of other gaps. It does not guess the dependency from the text: build the code, fix what the compiler reports, and check again. Other languages are not compared yet.

A report of `check` or `impact` says what its verdict leaves out in three lists. `narrowed` holds the parts a run judges by request, each with its `kind` (`diff`, `component`, `scope`) and what it is `limited_to`: `passed` then says nothing about the rest of the project. `evidence_gaps` holds code or facts the check could not read or judge, each with its `kind` (`uncovered`, `naming_unseen`, `std_unknown`, `manifests`, `parts`, `packages`, `unresolved_imports`, `symbols`, `unused`, and `deleted` for `impact`), its `files` and `count` or `extensions` where they are known, its `places` (`{file, line, specifier}`, as a violation's `evidence` gives a place) where it knows them one by one, a `message` and how to `close` it: a green verdict covers only the code an indexer read, and whoever acts on it has to treat the rest as unchecked. `accepted_gaps` holds the files `uncovered_ok` accepts, by entry, with its `because`. Cache provenance (`cached`) is reported apart: a cache hit is not a gap.

`check --require-evidence` makes an evidence gap fail the run: a run without violations or stale entries exits 4 when it has one, says `passed: false` in JSON, and prints the gaps under `Evidence required` (as errors on GitHub). Narrowing never triggers it, so `--diff --require-evidence` is the combination for CI; neither do accepted gaps nor a cache hit. Every gap counts, not only those in the changed files or in the part `--component` and `--scope` name, since an unread file can hide a cycle or a chain of imports through the code judged: accept a deliberate one in `uncovered_ok`. Violations come first: a run with both exits 1.

Files that are meant to stay outside an indexer (build helpers, generated code) are accepted with `uncovered_ok`, each with the reason. They then no longer count against the verdict of `check`, and `check` lists them under `accepted_gaps` instead of `evidence_gaps`. No indexer read their imports all the same, so `suggest`, `impact`, `why` and `baseline` stay as careful about them as about any uncovered file: `impact` lists them under `accepted_gaps` with the reason, and also under its `uncovered` evidence gap, since dependents through them may be missing:

```yaml
uncovered_ok:
  - { name: "tools/**", because: build helpers with no go.mod on purpose }
  - { name: ["**/race_on.go", "**/embed_none.go"], because: the other half of a build-tag pair }
```

## How it works

1. Walk the project. For Go, TypeScript, Python, Rust, PHP, Java, C# and Ruby run the SCIP indexer and read the index; for Kotlin, Swift and Svelte parse each file with tree-sitter and collect imports.
2. Turn every reference to a module into a dependency on a file inside the project, or on an external module.
3. Map both ends to components and check each edge against the rules.
4. Compare violations with the baseline and report.
