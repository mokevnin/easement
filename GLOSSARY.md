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
An import as the text of a file spells it, whether or not the index recorded it. Each written import is either resolved or an evidence gap.
_Avoid_: syntactic import, raw import

**Resolved import**:
A written import that the index takes to a source target: a file of the project or an external package. An import that the index ties to nothing, or only to a wildcard declaration such as `"*.css"`, is unresolved.
_Avoid_: indexed import, import with a reference

**Entry file**:
The file by which a name a file writes enters the component that defines it: the module the file names, else the first re-exporting module or alias of that component on the way the name came, else the file that defines the name. Surfaces judge the name there, against the component of the file that writes it.
_Avoid_: entry point (a program's start), target (the file that defines the name)

**Augmentation**:
A declaration of the project that the compiler merges into an external name, such as a TypeScript `declare module "pkg" { … }`. Writing the external name is a dependency on the package only, while a name that only the augmentation defines is a dependency on its file.
_Avoid_: merged declaration, alias (an alias is a name of its own), re-export (a re-export makes a name of the project available), ambient module (one with no package behind it is the only definition of its name)

**Wildcard declaration**:
A declaration that stands for every module its pattern matches and names none, such as a TypeScript `declare module "*.css"`. It is no module: neither it nor a name in it is a dependency or a symbol use, and an import it catches depends on what the import's text names, or is an unresolved import.
_Avoid_: ambient module (one declared by its whole name, `"virtual:x"`, is a module of its file), shim (the file that holds the declaration), declaration file (a `.d.ts` file)

**Source target**:
Where the index must take an import a file writes for it to count as resolved: a file of the project, checked or excluded, or an external package. A wildcard declaration is none, nor is a module of the project's own package that no indexed file defines.
_Avoid_: resolved module (a module reference that leads nowhere resolves nothing)

**Unresolved import**:
An import a file writes that the index takes to no source target and whose text names none either: its dependency is not in the graph, so it is an evidence gap, one for each cause.
_Avoid_: missing import, broken import (the code may build)

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
A proposed dependency kept in the policy together with the decision it must receive, so that a policy change that flips it fails.
_Avoid_: contract test, policy test, contract

**Policy delta**:
The dependency decisions that differ between two policies over the same code.
_Avoid_: config diff

## Trust in a verdict

**Narrowed verdict**:
A verdict that, on request, judges only part of the project or only part of a change.
_Avoid_: partial verdict

**Evidence gap**:
Code or facts the check could not read or could not judge, so a clean verdict says nothing about them.
_Avoid_: partial, unread reason, missing coverage

**Accepted gap**:
An evidence gap the policy declares deliberate; it is still reported and never counts as read.
_Avoid_: ignored file, covered file

**Legalized violation**:
A violation recorded in the baseline as accepted debt until it is fixed.
_Avoid_: ignored violation, suppressed violation

**Suppression**:
An exception written at one source line, with its reason, for one rule.
_Avoid_: ignore, legalization
