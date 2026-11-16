# Tool comparison

[Documentation](README.md) · [Project overview](../README.md)

- [Comparison with other tools](#comparison-with-other-tools)
- [Inspired by](#inspired-by)

## Comparison with other tools

Most architecture checkers are tied to one language. `easement` is not a replacement for them where they go deeper; it is for projects that want one model across several languages.

| Tool | Languages | Level of analysis | Rules are written as |
| --- | --- | --- | --- |
| `easement` | Go, TypeScript/JavaScript, Python, Java, Rust, C#, Ruby, Kotlin, PHP, Swift | imports between files and packages, symbols used, type names, declared packages | one YAML file |
| [ArchUnit](https://www.archunit.org/) | Java (and other JVM languages through bytecode) | classes, methods, annotations, inheritance | JUnit tests in code |
| [dependency-cruiser](https://github.com/sverweij/dependency-cruiser) | JavaScript, TypeScript | imports between modules | JS or JSON config |
| [import-linter](https://github.com/seddonym/import-linter) | Python | imports between modules | INI or TOML contracts |
| [Deptrac](https://github.com/deptrac/deptrac) | PHP | classes, collected into layers | YAML |
| [arch-go](https://github.com/arch-go/arch-go) | Go | packages, functions, naming | YAML |
| [depguard](https://github.com/OpenPeeDeeP/depguard) | Go | import allow and deny lists | `golangci-lint` config |

Where `easement` differs:

- **One config, one baseline, one report.** A repository with a Go backend and a TypeScript frontend gets the same component model, the same suppressions and the same CI output (`sarif`, `github`, `junit`) for both.
- **The compiler's view, through SCIP.** Go, TypeScript, Python, Rust, PHP, Java, C# and Ruby are resolved by their SCIP indexers, so aliases, `tsconfig` paths and re-exports are what the compiler sees; Kotlin and Swift are read with tree-sitter and nothing has to build. The price of the indexers is that dependencies have to be installed (and, for Java and C#, the project built), which ArchUnit, working on compiled classes, also asks for.
- **Adoption and review features built in.** A baseline for existing violations, `--diff` to judge only what a pull request adds, `--scope` for a part of the project, and `extends` for shared policy in a monorepo.
- **One static binary.** No runtime to install in CI.

Where the others are stronger:

- **Depth.** `easement` judges imports, the symbols used through them (Go, TypeScript, Python, Rust and Ruby), which component a type name belongs to ([`defines`](configuration.md#names-of-types)), which components may implement or extend the types of a component ([`allow_implementers`](configuration.md#who-may-implement-a-component), from the relations the Java, C#, Python and TypeScript indexes declare) and indirect reach through the methods a call references ([`reach: symbols`](configuration.md#reaching-through-others)). It has no rules about other inheritance, signatures, annotations or other class and member properties (ArchUnit), nor about package contents or function limits (arch-go).
- **Kotlin and Swift.** They are read with tree-sitter, not a compiler, so what a build system would resolve there is approximated.

Using both is reasonable: keep a language-specific tool for the deep rules of one codebase and let `easement` hold the boundaries between components and languages.

## Inspired by

`easement` builds on ideas from these tools, each of which covers one language or ecosystem:

- [ArchUnit](https://www.archunit.org/): architecture rules as tests for Java.
- [dependency-cruiser](https://github.com/sverweij/dependency-cruiser): dependency rules for JavaScript and TypeScript.
- [import-linter](https://github.com/seddonym/import-linter): import contracts between Python modules.
- [Deptrac](https://github.com/deptrac/deptrac): layers and allowed dependencies for PHP.
- [arch-go](https://github.com/arch-go/arch-go): architecture checks for Go.
- [depguard](https://github.com/OpenPeeDeeP/depguard): import allow and deny lists for Go.
- [go-arch-lint](https://github.com/fe3dback/go-arch-lint): components and allowed dependencies for Go in one YAML file.
- [ArchUnitNET](https://github.com/TNG/ArchUnitNET): ArchUnit's rules for .NET.
- [NetArchTest](https://github.com/BenMorris/NetArchTest): architecture rules as tests for .NET.
- [ArchUnitTS](https://github.com/LukasNiessen/ArchUnitTS): architecture rules as tests for TypeScript.
- [eslint-plugin-boundaries](https://github.com/javierbrea/eslint-plugin-boundaries): element types and allowed imports for JavaScript and TypeScript, as ESLint rules.
- [Sheriff](https://github.com/softarc-consulting/sheriff): module boundaries and dependency rules for TypeScript.
- [Tach](https://github.com/tach-org/tach): module boundaries, layers and public interfaces for Python.
- [PyTestArch](https://github.com/zyskarch/pytestarch): architecture rules as tests for Python.
- [PHPArkitect](https://github.com/phparkitect/arkitect): architecture rules for PHP.
- [PHPat](https://github.com/carlosas/phpat): architecture rules for PHP as PHPStan rules.
- [Packwerk](https://github.com/Shopify/packwerk): package boundaries for Ruby.
- [Konsist](https://github.com/LemonAppDev/konsist): architecture and structure tests for Kotlin.
- [Harmonize](https://github.com/perrystreetsoftware/Harmonize): architecture rules as tests for Swift.
- [cargo-pup](https://github.com/DataDog/cargo-pup): architectural lints for Rust.

`easement` puts one component model, one config and one baseline over all of them.
