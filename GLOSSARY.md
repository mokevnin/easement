# easement

An architecture quality gate: a declared policy says which parts of a project may depend on which, and easement judges the project's real dependencies against it, saying what its verdict does not cover.

## Policy

**Policy**:
Everything the project declares about allowed structure: components, rules, layers, surfaces and examples, across every file that contributes to it.
_Avoid_: config, rules (a rule is one part of the policy)

**Policy input**:
A file whose content is part of the policy: the configuration, every configuration it extends, the drawn diagram.
_Avoid_: config files

**Component**:
A named set of files that the policy treats as one owner; a file belongs to at most one component.
_Avoid_: module, package, layer

**Set**:
A named list of components in the policy (`sets`), of their names and globs over them, never another set; a list of components or a rule key names it as `'@drivers'`. Like a glob over component names, it stands for the components it covers, and every rule judges them as if they were written out.
_Avoid_: group, alias (an alias is a name in the code)

**Grant**:
What permits one component to depend on another: an explicit allowance, a common component, a lower layer, being test code, or being the same component.
_Avoid_: allow source, permission

**Prohibition**:
An explicit statement that one component must not depend on or reach another; it overrides every grant.
_Avoid_: deny, ban

**Surface**:
The part of a component that other components may use. The public surface is open to every consumer the policy lets in; a named surface is open only to the consumers it lists.
_Avoid_: interface (reserved for language-level interfaces and contracts), API, entry point

**Constraint**:
A clause of the policy that limits what the code of a component, or of the whole project, may use, must use or must contain, beyond the decision on a pair of components: a name or external module it must not use, a file it must contain, a most it may depend on. A change to the policy loosens a constraint when it permits what was refused or stops requiring what was required, and tightens it otherwise.
_Avoid_: ban (a prohibition is between components), restriction

## Code

**Dependency**:
A name a file writes that is defined in another file or in an external package: a module, or a name at the top level of one, such as a class, a function or a constant. A member reached through a value, such as a method or a field, is not a dependency on its own, in any language.
_Avoid_: import (an import is one way to write a dependency), edge

**Alias**:
A name whose definition the index marks as standing for other names: a type alias, or a constant the index relates to another name. Writing an alias is also a dependency on what it stands for. A variable initialized from another name is not an alias.
_Avoid_: re-export (a re-export points the use at the original itself)

**Re-export**:
A name a module makes available while another module defines it: a Rust `pub use`, a TypeScript `export … from`, a Python `__init__` import. The index points a use of it at the original, so writing it is a dependency on the component that defines it. The lines of the re-exporting component that refer to the original are evidence of that dependency too.
_Avoid_: alias (an alias is its own definition), forward

**Written import**:
An import as the text of a file spells it, whether or not the index recorded it. Each written import of a checked file is either resolved or an evidence gap.
_Avoid_: syntactic import, raw import

**Resolved import**:
A written import that the index takes to a source target: a file of the project other than the importing one, or an external package, on the text the index records the import on (the module, or for Python's `from . import y` the name `y`). A name a file of the project defines resolves it there too, as `VERSION` does `from . import VERSION`. An import that the index ties to nothing, or only to a wildcard declaration such as `"*.css"`, is unresolved.
_Avoid_: indexed import, import with a reference

**Entry file**:
The file by which a name a file writes enters the component that defines it: the module the file names, else the first re-exporting module or alias of that component on the way the name came, else the file that defines the name. Surfaces judge the name there, against the component of the file that writes it.
_Avoid_: entry point (a program's start), target (the file that defines the name)

**Manifest**:
A file that declares the external dependencies of a package, such as `package.json`, `go.mod` or `Cargo.toml`: the `manifests` rule checks the imports of the files under it against what it declares, and `packages` judges each dependency it declares as one of the file. Its kind is an entry of the registry, told by the name of the file.
_Avoid_: lockfile (it pins versions, it declares nothing), project file (a `.csproj` or `pom.xml` is read by an indexer, not here)

**Installed package**:
The package a name is of when the path of its symbol runs into a directory where the language's toolchain installs third-party code, such as `node_modules/<pkg>/`: the package under the innermost one (`hono`, `@scope/pkg`, the one pnpm's store links), whatever package the index credits the name to. Every other name is of the package the index credits it to, which is the project's own when a file of the project defines a name of it. A path an import writes is read the same way: a TypeScript specifier, or the static prefix of an import computed at run time, that is a path into `node_modules/<pkg>/` with something inside the package is a use of that package (`/node_modules/flag-icons/flags/4x3/de.svg?url`, `import.meta.glob("/node_modules/flag-icons/flags/*.svg")`), whether or not it is installed.
_Avoid_: unnamed package (the index may credit such a name to the project or to no named package, but its path names it)

**Framework**:
A dependency of a package that loads files of the package by convention, which no import names, such as SvelteKit's routes or Next.js's pages. It is a row of the table of the kind of manifest that lists it, and holds below that manifest's directory only, not in a package nested in it. `unused` takes the files it loads as entries, unless a config file may move them.
_Avoid_: plugin, convention (a convention is what the framework loads)

**Augmentation**:
A declaration of the project that the compiler merges into an external name, such as a TypeScript `declare module "pkg" { … }`. Writing the external name is a dependency on the package only, while a name that only the augmentation defines is a dependency on its file.
_Avoid_: merged declaration, alias (an alias is a name of its own), re-export (a re-export makes a name of the project available), ambient module (one with no package behind it is the only definition of its name)

**Wildcard declaration**:
A declaration that stands for every module its pattern matches and names none, such as a TypeScript `declare module "*.css"`. It is no module: neither it nor a name in it is a dependency or a symbol use, and an import it catches depends on what the import's text names, or is an unresolved import.
_Avoid_: ambient module (one declared by its whole name, `"virtual:x"`, is a module of its file), shim (the file that holds the declaration), declaration file (a `.d.ts` file)

**Source target**:
Where the index must take an import a file writes for it to count as resolved: a file of the project, checked or excluded, or an external package. A wildcard declaration is none, nor is a module of the project's own package that no indexed file defines, nor a file the walk leaves out (one `.gitignore` hides), which the index may still resolve the import to.
_Avoid_: resolved module (a module reference that leads nowhere resolves nothing)

**Unresolved import**:
An import a file writes that the index takes to no source target and whose text, or static prefix, names none either, or names more than one file it may mean (both `x.py` and `x/__init__.py`): its dependency is not in the graph, so it is an evidence gap, one for each cause. One that a `target_only` or excluded file writes is no gap, as no check judges that file's dependencies, but `unused` takes a file it may reach as unjudged.
_Avoid_: missing import, broken import (the code may build)

**Static prefix**:
The whole path segments of an import computed at run time before the first that is not literal, as its language's entry reads them: in an `import.meta.glob` pattern, the segments without any character of glob syntax (`* ? [ ] { } ( ) !` or `\`); in a template literal, the text of its fragments before the first substitution or escape, cut at its last `/`. A segment cut short is no part of it, so `/node_modules/flag-*/x.svg` has `/node_modules/`. One that is a path into one installed package makes the import a use of that package; any other, a bare one among them (`pkg/*.svg`, which an alias may stand for), leaves the import an unresolved one.
_Avoid_: base, glob base (a glob library's base may cut inside a segment)

**Import call**:
A call that imports the module its literal argument names, such as `require("./x")` or `importlib.import_module("a.b")`, which an index may not record: the compiler records `require()` only where it knows `require`, and scip-python records no such call. One the index does not record is an unresolved import of its own cause. Its argument is read as an import's specifier where the language reads it so (`require`), and never where the call reads it in its own terms (`import_module` against the package it is given).
_Avoid_: require (one language's name for it), dynamic import (its target may be computed, which is another cause)

**Implementation**:
A type's declared relation to a type it implements or extends, directly or through its ancestors. It is no dependency: the relation holds whether or not the code writes the supertype's name.
_Avoid_: inheritance edge, subtype dependency, structural match (a Go type that satisfies an interface declares nothing)

**Symbol use**:
Any reference from a file to a name defined elsewhere, members included, whether the code writes the type or the type is inferred or inherited.
_Avoid_: dependency (a dependency is only what the code writes)

## Decisions

**Dependency decision**:
The verdict on one dependency: allowed by a grant, or rejected by one rule, with the reason the policy gives.
_Avoid_: result, outcome

**Violation**:
A dependency or structure the policy rejects, located in the source or the policy.

**Proposed dependency**:
A dependency judged before the code that would create it exists.
_Avoid_: hypothetical edge, synthetic edge, preflight edge

**Policy example**:
A proposed dependency, or a proposed use of a name, kept in the policy together with the decision it must receive, so that a policy change that flips it fails.
_Avoid_: contract test, policy test, contract

**Policy delta**:
The violations that fail under one of two policies and not under the other, over the same code: of every rule, not only dependency decisions.
_Avoid_: config diff

## Trust in a verdict

**Narrowed verdict**:
A verdict that, on request, judges only part of the project or only part of a change.
_Avoid_: partial verdict

**Evidence gap**:
Code or facts the check could not read or could not judge, so a clean verdict says nothing about them.
_Avoid_: partial, unread reason, missing coverage

**Accepted gap**:
An evidence gap the policy declares deliberate, in an entry of `accepted_gaps` that names its kind and gives a reason, which is required; it is still reported and never counts as read. Only a gap that is a decision about the code can be accepted: source files no indexer covers (never because their indexer cannot run on this platform) and imports whose target is computed at run time; an entry that accepts nothing fails as `unmatched`. It lets a verdict pass, but an answer about the dependency graph still counts it as an evidence gap, since nobody saw its dependencies.
_Avoid_: ignored file, covered file

**Legalized violation**:
A violation recorded in the baseline as accepted debt until it is fixed.
_Avoid_: ignored violation, suppressed violation

**Suppression**:
An exception written at one source line, with its reason, for the rules it names.
_Avoid_: ignore, legalization

**Excused violation**:
A violation an inline suppression takes out of the verdict; it is reported under the suppression, never fails and is not debt in the baseline.
_Avoid_: legalized violation, ignored violation, fixed violation
