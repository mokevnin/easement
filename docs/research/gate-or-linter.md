# Architecture gate or linter: the seven rules of issue #85

Assessed on 2026-11-06 for [issue #85](https://github.com/mokevnin/easement/issues/85), against easement at `5081770`. Sections 1–3 hold facts: linter and tool documentation or source, the SCIP specification, and indexer output observed on fixtures. Section 4 holds the recommendations and is the only place they appear. Anything not observed or read is marked **unverified**. This extends [symbol-use-dependencies.md](symbol-use-dependencies.md), which covered occurrences and roles; this document covers `SymbolInformation`: kinds, relationships and signatures.

## The question

Issue #85 moves seven experimental rules out of the Dart PR [#71](https://github.com/mokevnin/easement/pull/71) and asks which belong in an architecture gate and which belong in a linter:

| Experimental rule | Policy |
| --- | --- |
| `dart-repository-declaration` | a repository contract must be declared `abstract interface` |
| `dart-contract-api-types` | no DTO type in a contract's public API: returns, parameters, fields, aliases, inherited signatures |
| `dart-repository-implementation` | the implementation class implements the contract, with the expected generic arguments |
| `dart-use-case-dependencies` | a use case depends on the contract, not on the concrete repository |
| `dart-domain-type-boundaries` | a domain declaration does not expose a Flutter `Widget` or another infrastructure type |
| `dart-one-widget-per-file` | a file declares at most one widget class |
| `dart-no-widget-helpers` | no named widget-returning helper (`Widget header()`) outside `build` |

Three things decide the answer: whether Dart linters already ship the rule, whether other architecture tools treat the property as architecture, and whether the facts a rule needs are in the SCIP indexes easement reads, for every language and not only Dart.

## Method

A fixture of one shape was written in each indexed language: a DTO `OrderDto`, an entity `Order`, a generic contract `Repository<T>`, a contract `OrderRepository` whose methods return a generic over `OrderDto` and take one as a parameter (plus a field and an alias where the language has them), an abstract base class, `SqlOrderRepository` extending the base and implementing `Repository<Order>` and `OrderRepository`, a use case `PlaceOrder` whose constructor takes the contract and whose body constructs an `OrderDto`, and two classes in one file. Each was indexed with the command easement runs ([`INDEXERS`](../../src/indexer.rs#L179)); scip-php needed `"classmap"` autoloading because its PSR-4 scan only finds a class in a file of the class's name. The indexes were dumped with a throwaway decoder on the `scip` 0.10 crate easement uses, printing each `SymbolInformation` (`kind`, `display_name`, `signature_documentation`, `documentation`, `enclosing_symbol`, relationships with all four flags) and each occurrence with its typed ranges. A second TypeScript fixture split into `domain`, `data` and `usecase` directories was checked with `easement check` built from `5081770`. Fixtures and decoder were throwaway and are not in the repository.

| Indexer | Version observed | Pinned in `mise.toml` |
| --- | --- | --- |
| scip-typescript | 0.6.1 | 0.6.1 |
| scip-java | `tool_info` says `0.0.0-SNAPSHOT` | 0.13.2 |
| scip-dotnet | `tool_info` says `0.1.0-SNAPSHOT` | installed by `dotnet tool` |
| scip-php | `0.0.1`, git `71a5b117` | installed by `composer global` |
| rust-analyzer | 1.99.1 (58cf8f9c 2026-10-13) | `rust` 1.99.1 |
| scip-go | 0.2.9 | 0.2.9 |
| scip-python | 0.6.8 | 0.6.8 |
| scip-ruby | 0.5.1 | `scip-ruby-v0.5.1` |
| scip_dart | not run: no Dart SDK here | 1.8.1 in PR #71's `tools/scip-dart/pubspec.yaml` |

scip_dart was read at tag [1.8.1 (`fef29da`)](https://github.com/Workiva/scip-dart/tree/fef29da4c46efa3672056ef267ec32a7ea34f41a), not run, so every statement about its output is **unverified** against an index. PR #71 registers Dart with `symbols: true` ([registry at `59576d4`](https://github.com/Bayrampro/easement/blob/59576d43c93c8a2712276a8a357905b35c2b99bf/src/lang/registry.rs#L231-L244)), so once it lands Dart is read like the other SCIP languages; nothing below assumes it has landed.

## 1. Facts: Dart and Flutter linters

Both widget rules exist, but not in the official linter.

| Rule | Tool | Rule id | Availability |
| --- | --- | --- | --- |
| one widget per file | DCM | [`prefer-single-widget-per-file`](https://dcm.dev/docs/rules/flutter/prefer-single-widget-per-file/): "Warns when a file contains more than one widget." Options `ignore-private-widgets`, `ignore-visible-for-testing`. | commercial; badge "free+", version badge v1.6.0 |
| widget helpers | DCM | [`avoid-returning-widgets`](https://dcm.dev/docs/rules/flutter/avoid-returning-widgets/): "Warns when a method, function or getter returns a Widget or subclass of a Widget." Options `ignored-names`, `ignored-annotations`, `allow-nullable`. | commercial; badge "free+", version badge v1.6.0 |
| both | dart_code_linter 4.4.4 (Bancolombia) | same ids, [`prefer_single_widget_per_file`](https://github.com/bancolombia/dart-code-linter/tree/b75341d4092b915a64a757585d7e941548b4da59/lib/src/analyzers/lint_analyzer/rules/rules_list/prefer_single_widget_per_file) | MIT; its CHANGELOG says "Fork: Dart code metrics 5.7.3" |
| both | many_lints 1.2.0 | [`prefer_single_widget_per_file`](https://github.com/Nikoro/many_lints/blob/6ea19f6358975df0d2cae5d30a0e3b691f6e3873/lib/src/rules/prefer_single_widget_per_file.dart), [`avoid_returning_widgets`](https://github.com/Nikoro/many_lints/blob/6ea19f6358975df0d2cae5d30a0e3b691f6e3873/lib/src/rules/avoid_returning_widgets.dart) | MIT |
| both | saropa_lints (17.0.1-beta.9 at HEAD) | [`avoid_returning_widgets` L258, `prefer_single_widget_per_file` L553](https://github.com/saropa/saropa_lints/blob/754a5d80eac8afbe7682906656efdbb1aab3a335/lib/src/rules/widget/widget_patterns_avoid_prefer_rules.dart#L258) | MIT |
| widget helpers | solid_lints (3.0.0 at HEAD) | [`avoid_returning_widgets`](https://github.com/solid-software/solid_lints/tree/67c329e6bad066a660792c65b95e05b94b2acf58/lib/src/lints/avoid_returning_widgets), option `ignored_types` | MIT |
| widget helpers | pyramid_lint | `avoid_returning_widgets` existed in 2.x and was **removed** in [3.0.0 (2026-06-03)](https://github.com/charlescyt/pyramid_lint/blob/5140d63b4f540b902e47428b0399ea73e2bfb389/CHANGELOG.md) | MIT |
| either | official linter, `lints` 6.1.0 ([core](https://github.com/dart-lang/core/blob/lints-v6.1.0/pkgs/lints/lib/core.yaml), [recommended](https://github.com/dart-lang/core/blob/lints-v6.1.0/pkgs/lints/lib/recommended.yaml)), [`flutter_lints` 6.0.0](https://github.com/flutter/packages/blob/flutter_lints-v6.0.0/packages/flutter_lints/lib/flutter.yaml), [very_good_analysis 11.0.0](https://github.com/VeryGoodOpenSource/very_good_analysis/blob/v11.0.0/lib/analysis_options.11.0.0.yaml) | none; the only widget-named lints in the [rule index](https://dart.dev/tools/linter-rules) are `use_key_in_widget_constructors` and `migrate_design_widgets` | BSD |

Availability:

- [dart_code_metrics](https://pub.dev/packages/dart_code_metrics) is marked "discontinued" on pub.dev; its open-source tag 5.7.0 is [MIT](https://github.com/dart-code-checker/dart-code-metrics/blob/5.7.0/LICENSE) and already contains both rules ([`avoid_returning_widgets`](https://github.com/dart-code-checker/dart-code-metrics/tree/5.7.0/lib/src/analyzers/lint_analyzer/rules/rules_list/avoid_returning_widgets), [`prefer_single_widget_per_file`](https://github.com/dart-code-checker/dart-code-metrics/tree/5.7.0/lib/src/analyzers/lint_analyzer/rules/rules_list/prefer_single_widget_per_file)). DCM itself is commercial. Its [pricing page](https://dcm.dev/pricing) lists a Free plan "For 1 seat … 100 unique lint rules" and puts "Rules configurability" under Pro. Whether the Free plan lets a team set these rules' options, and whether both rules are among the Free plan's 100, is **unverified** beyond the "free+" badge.
- [custom_lint](https://github.com/invertase/dart_custom_lint/blob/0e5def033bb3c034c0ca6b99f8689e744a3685b0/README.md) says: "This package is no longer under active development … The official `analysis_server_plugin` is now the recommended approach for building custom lints." Dart's [analyzer plugins page](https://dart.dev/tools/analyzer-plugins) says support "was added in Dart 3.10". The open-source packages above all run on it, which pyramid_lint's 3.0.0 entry ("Migrate from `custom_lint` to Dart analyzer plugins") documents for one of them.

Related rules, for the semantic proposals:

- The official [`library_private_types_in_public_api`](https://dart.dev/tools/linter-rules/library_private_types_in_public_api) ("AVOID using library private types in public APIs") is the only official rule about types in a public signature, and it concerns privacy, not layers.
- DCM's `avoid-banned-types`, `avoid-banned-imports` and `banned-usage` are "pro+" ([rules](https://dcm.dev/docs/rules/)).
- saropa_lints has [`prefer_interface_class`](https://github.com/saropa/saropa_lints/blob/754a5d80eac8afbe7682906656efdbb1aab3a335/lib/src/rules/core/class_constructor_rules.dart#L1532) ("Abstract class with only abstract members could be interface") and [`avoid_ui_in_domain_layer`](https://github.com/saropa/saropa_lints/blob/754a5d80eac8afbe7682906656efdbb1aab3a335/lib/src/rules/architecture/architecture_rules.dart#L444). The latter selects "models, entities, or domain directory" files by path and matches a fixed set of type names (`Widget`, `BuildContext`, `State`, `Color`, `TextStyle`, …).
- No linter checked has a rule that a class matching a name pattern must be `abstract interface`. `prefer_interface_class` points the other way: from the shape to the modifier.

## 2. Facts: architecture tools

The capabilities the five semantic rules need, in the tools that most often stand for "architecture testing". Each tool was read at the tag named, from its source unless stated. **DSL** is a built-in operator of a test-code API, **config** a declarative setting, **select** means the property can put a class into a layer but cannot be asserted, **custom** means writing a predicate, and **not verified** that no evidence was found either way (not proof of absence).

| Tool | (a) modifiers: interface, abstract, final | (b) implements / extends, and generic arguments | (c) role by name or annotation | (d) types in a public signature | (e) one declaration per file |
| --- | --- | --- | --- | --- | --- |
| ArchUnit 1.5.1 | DSL: [`haveModifier`, `beInterfaces`](https://github.com/TNG/ArchUnit/blob/v1.5.1/archunit/src/main/java/com/tngtech/archunit/lang/syntax/elements/ClassesShould.java#L278) | DSL: [`implement(X)`](https://github.com/TNG/ArchUnit/blob/v1.5.1/archunit/src/main/java/com/tngtech/archunit/lang/syntax/elements/ClassesThat.java#L438) then `resideInAPackage`, `beAssignableTo`; raw types only, arguments through [`getActualTypeArguments()`](https://github.com/TNG/ArchUnit/blob/v1.5.1/archunit/src/main/java/com/tngtech/archunit/core/domain/JavaParameterizedType.java#L37) in custom code | DSL: `haveSimpleNameEndingWith`, `areAnnotatedWith` | DSL per member: [`haveRawReturnType`, `haveRawParameterTypes`](https://github.com/TNG/ArchUnit/blob/v1.5.1/archunit/src/main/java/com/tngtech/archunit/lang/syntax/elements/CodeUnitsShould.java#L51) after `methods().that().arePublic()`, `fields()…haveRawType` | custom: `getSource()` → `getFileName()` |
| ArchUnitNET 0.13.5 | DSL: [`BeAbstract()`, `BeSealed()`](https://github.com/TNG/ArchUnitNET/blob/0.13.5/ArchUnitNET/Fluent/Syntax/Elements/Types/Classes/IAddClassCondition.cs#L9-L10) | DSL: [`ImplementInterface`](https://github.com/TNG/ArchUnitNET/blob/0.13.5/ArchUnitNET/Fluent/Syntax/Elements/Types/IAddTypeCondition.cs#L28), `ResideInNamespace`; arguments ignored, reachable as `TargetGenericArguments` in custom code | DSL: `HaveNameEndingWith`, `HaveAnyAttributes` | DSL: [`HaveReturnType`](https://github.com/TNG/ArchUnitNET/blob/0.13.5/ArchUnitNET/Fluent/Syntax/Elements/Members/MethodMembers/IAddMethodMemberCondition.cs#L29); no parameter or field condition found (**not verified** beyond a source search) | not verified |
| NetArchTest 1.3.2 (2021) | DSL: [`BeInterfaces`, `BeSealed`](https://github.com/BenMorris/NetArchTest/blob/v1.3.2/src/NetArchTest.Rules/Conditions.cs#L331) | DSL: `ImplementInterface`, `Inherit`; compares the [resolved full name](https://github.com/BenMorris/NetArchTest/blob/v1.3.2/src/NetArchTest.Rules/FunctionDelegates.cs#L144), so arguments are dropped | DSL: `HaveNameEndingWith`, `HaveCustomAttribute` | custom (`MeetCustomRule`); `HaveDependencyOn` is whole-type | not verified |
| PHPat 0.12.5 | DSL: [`beFinal`, `beAbstract`, `beInterface`](https://github.com/carlosas/phpat/blob/0.12.5/src/Test/Builder/ShouldStep.php#L42-L84) | DSL: [`Selector::implements`](https://github.com/carlosas/phpat/blob/0.12.5/src/Selector/SelectorPrimitive.php#L91), `should()->implement()`; generic `@implements` **not verified** | DSL: `Selector::classname($re, true)`, `appliesAttribute` | no: parameter, return and property types all feed one [`dependOn`](https://github.com/carlosas/phpat/tree/0.12.5/src/Rule/Assertion/Relation/Depend) | not verified |
| Deptrac 4.7.2 | select: `class`, `interface`, `trait` collectors | select: [`implements`, `extends`, `inherits`](https://github.com/deptrac/deptrac/blob/4.7.2/docs/collectors.md) put a class into a layer | config: `classNameRegex`, `attribute` | no: it records [`returntype` and `parameter`](https://github.com/deptrac/deptrac/blob/4.7.2/src/Contract/Ast/AstMap/DependencyType.php#L17-L18) dependencies, but a rule cannot select them | not verified |
| import-linter 2.16 | none of the [five contract types](https://github.com/seddonym/import-linter/blob/v2.16/src/importlinter/application/use_cases.py#L400-L404) | none | config: module names with wildcards | none | none |
| dependency-cruiser 18.7.2 | not verified | not verified | config: path regex | module level only: [`dependencyTypes`](https://github.com/sverweij/dependency-cruiser/blob/v18.7.2/doc/rules-reference.md) `type-only`, `export` | not verified |
| go-arch-lint 1.19.1 | n/a | config: [`deepScan`](https://github.com/fe3dback/go-arch-lint/blob/v1.19.1/docs/syntax/README.md) judges the component of an implementation injected into an interface parameter | config: package path globs | not verified | not verified |
| ArchUnitTS 2.5.4 / ts-arch 5.4.1 | not verified | not verified | file name and folder only | not verified | ArchUnitTS: [`metrics().count().classes().shouldBeBelowOrEqual(1)`](https://github.com/LukasNiessen/ArchUnitTS/blob/v2.5.4/src/metrics/fluentapi/metrics/count-metrics.ts#L154-L156) |
| Konsist 0.18.1 (Kotlin) | DSL predicates: [`hasAbstractModifier`](https://github.com/LemonAppDev/konsist/blob/v0.18.1/lib/src/main/kotlin/com/lemonappdev/konsist/api/provider/modifier/KoAbstractModifierProvider.kt#L14), `hasSealedModifier`, `hasFinalModifier` | DSL: `hasParentOf(X::class)`; a parent declaration is a [`KoTypeArgumentProvider`](https://github.com/LemonAppDev/konsist/blob/v0.18.1/lib/src/main/kotlin/com/lemonappdev/konsist/api/declaration/KoParentDeclaration.kt#L30), so `hasTypeArgumentOf(Order::class)` asserts the argument | DSL: `withNameEndingWith`, `withAnnotationOf` | DSL: `returnType`, parameters and properties, with a visibility filter | DSL predicate: `files.assertTrue { it.classes().size == 1 }` |
| Sonargraph (docs unversioned) | select: [`JavaIsInterface`, `CSharpIsInterface`](https://eclipse.hello2morrow.com/doc/standalone/content/architecture_predefined_attribute_retrievers.html) | select: `JavaImplementsInterface`, `TypescriptImplementsInterface` | config: `include "JavaHasAnnotation: **.Component"` | not verified; [dependency types](https://eclipse.hello2morrow.com/doc/standalone/content/architecture_restrict_dependency.html) are NEW, EXTENDS, IMPLEMENTS, CALL, READ, WRITE, USES | not verified |
| heimdall_test 0.10.3 (Dart) | DSL: `beFinal`, `beSealed`, `beBase`, `beInterfaces` | DSL: `beAssignableTo('Repository<Order>')`, from a syntactic parse (`parseString`), not a resolved one | DSL: `areAnnotatedWith`, name suffix | DSL: `haveReturnType…`, `notReceiveParameterTypeName…` | DSL: `haveAtMostOnePublicClass()` |
| arch_dart 1.0.0 (Dart) | DSL: `shouldBeFinal`, `shouldBeSealed`, `shouldBeInterface`, `shouldBeAbstract` | DSL: `shouldImplement`, `shouldExtend` | DSL: `withNameEndingWith`, `withAnnotation` | DSL: `shouldReturnType` on functions | not verified |

The two Dart rows come from the [pub.dev archives](https://pub.dev/packages/heimdall_test/versions/0.10.3) of those versions. Other Dart packages are narrower: dart_arch_test 0.3.1 selects by `extending()`, `implementing()` and `withAnnotation()` for layer rules; arch_test 1.0.0 offers custom predicates over the analyzer's `ClassElement`; lakos 2.0.7 draws the import graph and finds cycles. The four Dart architecture-test packages were last pushed between January and September 2026 and have 0 to 6 GitHub stars each.

Taken together:

- **(a), (b), (c) and (d) are the core of the class-level tools** (ArchUnit, ArchUnitNET, NetArchTest, PHPat, Konsist, and the young Dart ports). All of them read one language's compiled or parsed model, and all are test-code APIs, not a policy file.
- **The policy-file tools** (Deptrac, Sonargraph, import-linter, dependency-cruiser, go-arch-lint) do not assert (a), (b) or (d). Deptrac and Sonargraph use (a) and (b) only to *select* what belongs to a layer, then judge dependencies between layers. go-arch-lint's `deepScan` is the one place an implementation relationship feeds a dependency verdict.
- **Generic arguments** are asserted directly only by Konsist (and, syntactically, heimdall_test). ArchUnit and ArchUnitNET expose them to custom code; NetArchTest and NetArchTest.eNhancedEdition drop them.
- **One declaration per file** is a test predicate in Konsist, a metric in ArchUnitTS and heimdall_test, and custom code in ArchUnit. No policy-file tool has it.

## 3. Facts: what SCIP and the indexers give

### The specification

[`scip.proto` at `bbec534`](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto) offers four places for the facts these rules need:

- **`SymbolInformation.kind`** ([L284-L430](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L284-L430)) has `Class`, `Interface`, `Struct`, `Trait`, `Protocol`, `Mixin` ("For Dart"), `TypeAlias`, and for members `AbstractMethod`, `MethodSpecification`, `TraitMethod`, `PureVirtualMethod`. It has no abstract, final or sealed class and no modifier field; `Modifier` is a Solidity kind. A declaration's modifiers can only come from text.
- **`Relationship`** ([L465-L517](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L465-L517)) is a `symbol` string and four booleans. The spec's own example: `Dog#` has `{symbol: "Animal#", is_implementation: true}`, and `Dog#sound()` has `is_implementation` and `is_reference` to `Animal#sound()`. There is no field for type arguments: a relationship can say "implements `Repository#`", never "implements `Repository<Order>`".
- **`signature_documentation`** ([L437-L444](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L437-L444), message `Signature` [L232-L249](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L232-L249)) is display text, "as it's displayed in API documentation or in hover tooltips", with optional `occurrences` "to support hyperlinking referenced symbols in the signature". Those occurrences would be the resolved types of a signature. `documentation` ([L257-L263](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L257-L263)) says indexers "may include signature documentation in this field by rendering markdown code blocks" for historical reasons.
- **`enclosing_symbol`** ([L445-L462](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L445-L462)) is only for local symbols. The typed enclosing range of a definition occurrence ([L746-L805](https://github.com/scip-code/scip/blob/bbec5345bfae602b31286a9ec74e2c2042713f50/scip.proto#L746-L805)) is "the entire definition AST node, including documentation": for a method it covers the signature and the body together. No field marks where a signature ends and a body begins.

easement reads none of `SymbolInformation` today: [`src/scip/mod.rs`](../../src/scip/mod.rs#L125-L145) takes occurrences only, keeping definitions of types (for `defines`) and the enclosing ranges of methods and functions (for `reach`). Since [#112](https://github.com/mokevnin/easement/issues/112) landed, after this assessment, easement reads `kind == TypeAlias` and `is_reference` relationships to follow aliases ([declaration-relationships.md §2](declaration-relationships.md#2-facts-corrections-to-gate-or-lintermd)).

### Observed: kinds, modifiers and signatures

| Indexer | Kind of interface / abstract class / class | Where a modifier or a signature can be read | Signature `occurrences` |
| --- | --- | --- | --- |
| scip-typescript | `Interface` / `Class` / `Class` | `documentation` markdown only: ``class BaseRepo`` for an abstract class (no `abstract`); `(method) findAll(): Promise<OrderDto[]>` | none |
| scip-java | `Interface` / `Class` / `Class`; interface methods `AbstractMethod` | `signature_documentation`: `abstract class BaseRepo`, `final class SqlOrderRepository extends BaseRepo implements Repository<Order>, OrderRepository`, `public abstract List<OrderDto> findAll()` | none |
| scip-dotnet | `UnspecifiedKind` for all | `documentation` markdown: `class BaseRepo` and `class SqlOrderRepository` (no `abstract`, no `sealed`), `interface IRepository<T>`; members do carry it: `public abstract string BaseRepo.Name()` | none |
| scip-php | `UnspecifiedKind` for all | `documentation` markdown: `abstract class BaseRepo`, `final class SqlOrderRepository extends App\BaseRepo implements \App\Repository` (generic argument only in the docblock `@implements Repository<Order>`) | none |
| rust-analyzer | `Trait` / n/a / `Struct`; trait methods `TraitMethod` | `signature_documentation`: `pub trait Repository<T>`, `pub fn find_all(&self) -> Vec<OrderDto>` | none |
| scip-go | `Interface` / n/a / `Struct`; interface methods `MethodSpecification` | `signature_documentation`: `type OrderRepository interface {…}`, `func (OrderRepository).FindAll() []OrderDto` | none |
| scip-python | `UnspecifiedKind` for all | `documentation` markdown: `class SqlOrderRepository(OrderRepository, Repository[Order]):`, `@abstractmethod\ndef find_all(…) -> list[OrderDto]:` | none |
| scip-ruby | `UnspecifiedKind` for all | `documentation` markdown: `class Fx::BaseRepo` (no superclass), `sig { returns(T.untyped) }` | none |
| scip_dart (source) | `Class` for every class, `Mixin`, `Enum` ([kind_generator.dart L13-L16](https://github.com/Workiva/scip-dart/blob/fef29da4c46efa3672056ef267ec32a7ea34f41a/lib/src/kind_generator.dart#L13-L16)) | `signature_documentation` is the analyzer's `displayString` ([metadata.dart L48, L73-L77](https://github.com/Workiva/scip-dart/blob/fef29da4c46efa3672056ef267ec32a7ea34f41a/lib/src/metadata.dart#L48)); the analyzer writes `sealed`/`abstract`, `base`/`interface`/`final`, `mixin`, then `class Name<T> extends … with … implements …` ([display_string_builder.dart L44-L68](https://github.com/dart-lang/sdk/blob/9f8a33f3b4dc3adb80c444b5cf9d8c2dc138f977/pkg/analyzer/lib/src/dart/element/display_string_builder.dart#L44-L68), SDK `main`; the analyzer version scip_dart 1.8.1 resolves to is **unverified**) | none set in the source read |

None of the eight indexes carried a single `Signature.occurrences` entry. The signature is text, with names as written (`OrderDto`, `\App\OrderDto`, `Promise<OrderDto[]>`), not symbols.

### Observed: implementation relationships

| Indexer | `SqlOrderRepository` → supertypes | Method → the method it implements | Generic argument |
| --- | --- | --- | --- |
| scip-typescript | `is_implementation` to `BaseRepo#`, `Repository#`, `OrderRepository#` | `is_implementation` + `is_reference` | dropped (`Repository#`); `Order#` is a separate occurrence on the `implements` line |
| scip-java | `is_implementation` to all three | `is_implementation` + `is_reference`, and the abstract method also carries one back to the implementing method | dropped; in the signature text |
| scip-dotnet | `is_implementation` to all three | `is_implementation` + `is_reference` | dropped; not in the hover text of the class |
| scip-php | **none** (0 relationships in the index) | none | docblock text only |
| rust-analyzer | **none** (0 relationships) | none, but the method's own symbol names the impl: ``impl#[SqlOrderRepository][`Repository<Order>`]get().`` | in the descriptor of each method of the impl |
| scip-go | `is_implementation` to `OrderRepository#` (implicit satisfaction); **none** to the generic `Repository[T]`, although `var _ Repository[Order] = (*SqlOrderRepository)(nil)` compiles | `is_implementation` to `OrderRepository#FindAll.`, none to `Repository#Get.` | n/a |
| scip-python | `is_implementation` to each direct base, including `abc/ABC#` and `typing/Protocol#` on the contracts | `is_implementation` only | dropped; in the hover text |
| scip-ruby | **none**, for `< BaseRepo` or `include OrderRepository` | none | n/a |
| scip_dart (source) | `is_implementation` to **every** supertype but `Object` (`element.allSupertypes`, transitive) ([relationship_generator.dart L14-L27](https://github.com/Workiva/scip-dart/blob/fef29da4c46efa3672056ef267ec32a7ea34f41a/lib/src/relationship_generator.dart#L14-L27)) | `is_implementation` + `is_reference` for methods, fields and accessors ([L32-L70](https://github.com/Workiva/scip-dart/blob/fef29da4c46efa3672056ef267ec32a7ea34f41a/lib/src/relationship_generator.dart#L32-L70)) | dropped: the symbol is of `type.element` |

`is_type_definition` and `is_definition` were set by no indexer in these fixtures, and `enclosing_symbol` only on locals (scip-java on parameters and variables, rust-analyzer on parameters, variables and type parameters), as the spec intends.

### Observed: where a reference stands

- **Enclosing ranges on type definitions** were present for the contract, the base class and the implementation in scip-typescript, scip-java, scip-go, scip-python, rust-analyzer and scip-ruby, and absent on every definition in scip-dotnet and scip-php. rust-analyzer alone also set them on references (33 of them), but each is the referenced symbol's own definition range, not one enclosing the reference ([declaration-relationships.md §2](declaration-relationships.md#2-facts-corrections-to-gate-or-lintermd)).
- **Signature or body.** A method's enclosing range covers its signature and body, so a reference to `OrderDto` in `get(id): Order { const x = new OrderDto(); … }` and one in `findAll(): Promise<OrderDto[]>` look alike: a plain reference inside a method's range. scip-typescript gives the members of an interface no enclosing range at all, only the interface itself.
- **A contract without bodies is the exception.** In a contract that declares only signatures, every reference inside the type's enclosing range is a reference from its declarations: return types, parameters, fields, type parameters and the `implements` clause. That holds in the six indexers that range type definitions. It is true of every TypeScript and Go interface, but not in general: a Java `default` method, a Rust default trait method, a concrete member of a Dart `abstract interface class` and a concrete method of a Python ABC all have bodies, and a reference in one looks the same as one in a signature. It does not cover inherited signatures (a supertype's method returning a DTO is not written in the contract) or an alias defined elsewhere (the reference is to the alias, as [symbol-use-dependencies.md](symbol-use-dependencies.md) found).
- `syntax_kind` was set only by scip-php (`IdentifierType`, `IdentifierParameter`, `Identifier`). It is a highlighting class and does not say whether a type is in a signature.

### Observed: what easement already judges

The layered TypeScript fixture (contract and entity in `src/domain`, `OrderDto` and `SqlOrderRepository` in `src/data`, `PlaceOrder` in `src/usecase`, whose `make()` does `new PlaceOrder(new SqlOrderRepository())`) with this policy:

```yaml
version: 1
components:
  domain:
    in: src/domain
    defines: ["*Repository"]
    because: repository contracts live in the domain
  data:
    in: src/data
    defines: ["*Dto"]
  usecase:
    in: src/usecase
rules:
  domain:
    may_depend_on: []
    because: the domain, its repository contracts included, names no DTO
  data:
    may_depend_on: [domain]
  usecase:
    may_depend_on: [domain]
    because: a use case depends on the repository contract, never on its implementation
```

gave exactly three violations:

```text
src/data/sql_order_repository.ts:4: SqlOrderRepository is defined in data, but types named like *Repository belong in domain [naming]
src/domain/order_repository.ts:2: domain -> data is not allowed (written as src/data/order_dto.ts) [dependency]
src/usecase/place_order.ts:2: usecase -> data is not allowed (written as src/data/sql_order_repository.ts) [dependency]
```

With `usecase` allowed `data` and `forbid_symbol: [{ name: "src/data/sql_order_repository.SqlOrderRepository" }]` instead, the use case failed as `symbol` at the import (line 2) and at the construction (line 5). The `naming` violation shows the limit of a name glob: `*Repository` claims the contract and its implementation alike, and only a naming convention (`Sql*Repository`, `*RepositoryImpl`) or the declaration's kind can tell them apart.

## 4. Recommendations

Everything below is a recommendation drawn from the facts above, not a fact. Judgment calls are marked as such.

**A dividing line.** easement judges *where a written name points*: which component's file a dependency or a symbol use lands in ([ADR 0003](../adr/0003-a-dependency-is-a-written-name.md)). Two of the seven rules are that already. Three ask about the *shape of a declaration*: its modifiers, its supertypes, the position of a type inside it. SCIP carries that shape unevenly (section 3), and the policy-file tools closest to easement (Deptrac, Sonargraph, import-linter, dependency-cruiser, go-arch-lint) do not assert it either; the tools that do are per-language test APIs. Two are code style inside a file, which Dart linters already ship. Judgment call: a rule belongs in easement when it names components and judges where a reference lands, and only reads a declaration's shape where that decides which component something belongs to.

### `dart-use-case-dependencies`: already expressible

Both forms worked on the TypeScript fixture (section 3): the import and the `new SqlOrderRepository()` failed. A constructor parameter typed with the concrete class is a written name too, so by [ADR 0003](../adr/0003-a-dependency-is-a-written-name.md) it counts as well (not run here):

```yaml
rules:
  usecase:
    may_depend_on: [domain]          # the contracts live in domain; data holds SqlOrderRepository
    because: a use case depends on the repository contract, never on its implementation
```

Or, where use cases may use `data` for other things, forbid the implementation by name: `forbid_symbol: [{ name: "lib/data/sql_order_repository.SqlOrderRepository" }]`, in the Dart spelling PR #71's README gives (an internal file without `.dart`). For Dart both depend on PR #71 landing with `symbols: true`, and neither was run on a Dart index here (**unverified**).

### `dart-contract-api-types`: already expressible at component level; position-aware form is a spike

A component that holds the contracts, and may not depend on the component that `defines` the DTOs, fails a DTO in a return type, a parameter, a field, a type argument or an `implements` clause. Where a contract declares only signatures, that is exactly its API (section 3, "A contract without bodies is the exception"); where it has bodies (a Java `default` method, a Rust default method, a concrete member of a Dart `abstract interface class`), a DTO used only in a body fails too. That errs toward reporting more, which suits a gate:

```yaml
components:
  entities:
    in: lib/domain/entities
  contracts:
    in: lib/domain/repositories
  dto:
    in: lib/data/dto
    defines: ["*Dto"]
    because: a DTO lives with the data layer
rules:
  contracts:
    may_depend_on: [entities]
    because: a repository contract speaks in entities, never in DTOs
```

Run on a TypeScript tree with these paths (a contract returning `Promise<OrderDto[]>`, an alias `Orders = OrderDto[]` beside it, and a second contract naming only `Order`), it gave one violation, `lib/domain/repositories/order_repository.ts:2: contracts -> dto is not allowed (written as lib/data/dto/order_dto.ts) [dependency]`, and passed the clean contract. The violation stands on the first line that names the DTO's file, the import, not on each signature: the report says which file leaks, not which method.

What it does not catch, and why that is acceptable or not:

- an alias declared in `contracts` (`typedef Orders = List<OrderDto>`) fails at the alias, since the alias writes `OrderDto`; an alias declared in another component hides the DTO until aliases are followed ([#112](https://github.com/mokevnin/easement/issues/112)), as [ADR 0003](../adr/0003-a-dependency-is-a-written-name.md) records for every language;
- an inherited signature: a supertype in `entities` whose method returns a DTO fails in `entities` if it, too, may not depend on `dto`. The rule judges where the DTO is written, not every type that inherits it. Judgment call: that is the right place to report it.

The finer rule, "a type may be used in the body but not in the public signature" ([feature-candidates.md §10](feature-candidates.md#10-public-api-type-boundaries)), needs facts no indexer gives: `Signature.occurrences` was empty in all eight, a method's enclosing range covers signature and body together, and visibility is not a field. It would need a per-language extraction beyond SCIP, which the experiment's analyzer companion was ([`tools/dart-indexer/bin/semantics.dart` at `5458bef`](https://github.com/Bayrampro/easement/tree/5458bef/tools/dart-indexer/bin)) and which AGENTS.md says needs a mechanism that serves every language. Keep §10 a spike; do not build it for Dart.

### `dart-domain-type-boundaries`: expressible at component level; position-aware form is the same spike

"The domain does not use Flutter" is `forbid_external` (or a `may_use_external` allow-list) on the domain component, and "the domain does not name `Widget`" is `forbid_symbol`. How PR #71 spells a Pub package in `forbid_external` is not shown in its README (**unverified**); its symbol form is `package:dio/dio.dart.Dio`. This is the same reach as saropa_lints' `avoid_ui_in_domain_layer`, which selects domain files by path and matches a fixed list of type names. "Exposes a `Widget` in a forbidden position, but may use one internally" is the §10 spike above. For a domain layer, forbidding `Widget` to the whole component is the usual policy; judgment call: the position-aware form is worth little here.

### `dart-repository-implementation`: candidate general capability, without generic arguments

What is portable is "implements": section 3 shows `is_implementation` from a class to its supertypes in scip-typescript, scip-java, scip-dotnet, scip-python, scip-go (implicit satisfaction, but not for a generic interface) and scip_dart (source; every supertype, transitively), and the impl in rust-analyzer's method descriptors. scip-php and scip-ruby emit no relationships. A rule built on it would be one general rule with an evidence gap for PHP, Ruby, Kotlin and Swift, and for Go generic interfaces. Two more gaps would be silent unless the rule names them:

- rust-analyzer shows an impl only through the symbols of its methods, so an `impl Trait for T {}` that defines no method (a marker trait, or one using only default methods) would leave no trace (**unverified**: the fixture's impls all defined methods);
- scip-go's relationships come from structural satisfaction, so a struct may get `is_implementation` to an interface it was never meant to implement, and a placement rule would then claim it (**unverified**: the fixture had one candidate interface).

What is not portable is the generic argument. A `Relationship` holds a bare symbol by design; the argument survives only as text (scip-java's signature, scip-python's hover, rust-analyzer's descriptor, scip_dart's display string), as a separate occurrence on the `implements` line (scip-typescript), or not at all (scip-dotnet, scip-go). Among the tools, only Konsist asserts it natively. Recommendation: leave the generic argument out. Wherever the code uses the implementation as the contract, the type checker already rejects a wrong argument; judgment call: what is left for a gate is placement, not typing.

Two shapes fit easement's model, and the issue should pick at most one:

1. **Placement by relationship**: a type that implements a contract of component X must be defined in component Y (or: may only be defined in the components that list X). It extends `defines`, which today claims by name glob and could not tell `OrderRepository` from `SqlOrderRepository` in section 3.
2. **Selection by relationship**, the Deptrac/Sonargraph pattern (section 2): a component's `in` that also takes "types implementing X". That changes component membership from files to declarations, a much larger change; not recommended.

Index facts needed for (1): a normalized `implements` edge per type definition (from relationships, or from rust-analyzer's `impl#[Type][Trait]` descriptors), whether it is direct or transitive (scip-java, scip-typescript, scip-dotnet and scip_dart are transitive and scip-python direct, as [declaration-relationships.md §4](declaration-relationships.md#4-facts-question-4-ancestry-externals-and-interface-inheritance) observed; a rule has to normalize one way), and a per-language registry flag for "no implementation facts" that becomes an evidence gap.

### `dart-repository-declaration`: not now; a narrow form is a candidate alongside the previous one

Modifiers are not a SCIP fact. `kind` separates an interface or trait from a class in scip-typescript, scip-java, scip-go and rust-analyzer, but every Dart class is `Class`, and scip-dotnet, scip-php, scip-python and scip-ruby set no kind. `abstract`, `final` or `sealed` is in the signature text of scip-java, scip-php and scip_dart (source), and absent from scip-typescript's and scip-dotnet's class hover. A portable rule would parse display text per language, which is what the class-level tools do with a compiler model instead. Judgment call: "a contract must be `abstract interface` rather than `abstract class`" is a coding convention of one language; leave it to a linter or a Dart architecture-test package (heimdall_test and arch_dart have `beInterfaces` / `shouldBeInterface`; saropa_lints' `prefer_interface_class` covers the reverse direction).

The part with architectural weight is "a type claimed as a contract must be an abstraction, not an implementation". If the issue wants it, it fits as one more condition on the `defines` claim ("`*Repository` in `contracts` must be an interface or trait"), read from `kind` where the indexer sets it and from a registry hook elsewhere, with an evidence gap for the languages that give neither. It shares its facts with the placement rule above and should be decided with it, not before.

### `dart-one-widget-per-file` and `dart-no-widget-helpers`: a linter's job

Both are shipped Dart lint rules: `prefer-single-widget-per-file` and `avoid-returning-widgets` in DCM (commercial, marked "free+"), and under MIT in dart_code_linter, many_lints and saropa_lints, plus `avoid_returning_widgets` in solid_lints. Neither names a component or judges where a reference lands; both judge the arrangement of code inside one file. A cross-language "one declaration kind per file" exists only as a test predicate or metric (Konsist, ArchUnitTS, heimdall_test), in no policy-file tool. In easement it would also need "is a widget", a transitive subclass relation that only scip_dart emits transitively. Recommendation: do not add either rule or a general one-declaration-per-file rule; point Flutter users at those linters in the README's Dart section if PR #71 lands.

### For the issue

- Two of seven are config today; the issue can close them with the examples above once PR #71 lands and they are run on a Dart index.
- Two are lint rules with existing open-source implementations.
- Three reduce to one open question: should easement read `SymbolInformation` relationships (and kind) at all? If yes, the first rule is placement by implementation, with the evidence gaps listed; modifiers and generic arguments stay out. Signature positions stay a spike until an indexer fills `Signature.occurrences` or a cross-language extraction is agreed.
- Whatever is built, the Dart-only extraction companion should not come back: every fact recommended above is in scip_dart's index by its source (relationships, and the modifiers as signature text), so Dart needs nothing beyond PR #71's indexer. That is **unverified** until a Dart index is dumped.
