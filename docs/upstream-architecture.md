# Upstream evaluator architecture notes

This document records the implementation-level distinctions used when designing this crate. It is not a claim of Nix compatibility. The linked source files are the primary references; the expression language manual is the semantic reference.

## Nix implementation: layers and ownership

Nix keeps several layers separate, even though the production evaluator integrates them through a large evaluation state:

| Concern | Nix implementation | Responsibility |
| --- | --- | --- |
| Grammar | `src/libexpr/parser/parser.y` and the lexer/scanner implementation | Turns Nix source into parser AST nodes. |
| Expression representation | `src/libexpr/include/nix/expr/nixexpr.hh` | Defines expression nodes such as lambdas, attrsets, let, with, lists and operators. |
| Runtime values | `src/libexpr/include/nix/expr/value.hh` | Tagged runtime values, including lists, attrsets, functions, paths, strings and thunks. |
| Evaluation state | `src/libexpr/include/nix/expr/eval.hh` | Owns evaluation-wide services, environments, parsing/evaluation caches, symbol tables, source and store integrations. |
| Forcing | `src/libexpr/include/nix/expr/eval-inline.hh` | Forces delayed values and checks recursive forcing. |
| Evaluation implementation | `src/libexpr/eval.cc` | Implements expression evaluation, lexical lookup, let/attrset construction, comparison and file evaluation. |
| Primitive operations | `src/libexpr/primops.cc` plus store/fetcher libraries | Exposes operational capabilities such as imports, path/string operations and store-aware functions. |

The C++ `#include` lines in these files are build-time dependencies between Nix implementation headers. They are not directives in the Nix expression language and should not become parser features in this crate. Nix source has its own grammar and comments. The expression-level `import` operation is an evaluator primitive; it is not a C/C++ preprocessor include.

### Thunks and weak-head normal form

Nix's runtime `Value` has an explicit thunk representation. A thunk retains an expression and its lexical `Env`. `EvalState::forceValue` changes the delayed value into a blackhole before evaluating the expression into that same value slot. A recursive force observes the blackhole and raises an infinite-recursion error. The language manual describes the observable call-by-need behavior: expressions are evaluated only when needed, and their results are reused.

This crate uses the same essential state machine, expressed with Rust ownership:

`Suspended(expression, environment) -> Evaluating -> Evaluated(result)`

A second `Deferred` state supports host-side delayed computations and lazy `inherit (expr) name` projections. The evaluator stores thunks in a session-owned arena and passes typed `Thunk` handles through values. That avoids `Rc<RefCell<...>>` cycles between mutually recursive environments and closures. It is a different memory representation from Nix's GC-managed in-place `Value`, not a translation of its C++ implementation.

### Environments and variable resolution

Nix uses a static environment description for lexical bindings. An `ExprVar` carries a lexical level/displacement and a `fromWith` marker. `lookupVar` walks the environment chain to the precomputed frame; ordinary variables load a slot, while variables resolved through a `with` scope cause that scope to be forced and searched.

The current Rust implementation keeps environment maps and resolves names dynamically. It preserves the key lookup rule—lexical bindings across the chain are checked before dynamic `with` scopes—but does not yet have Nix's static name-resolution pass. A future resolver should compile the rnix AST into a compact internal IR with resolved local slots, retaining a dynamic-with fallback for names that depend on runtime attribute sets.

### Recursive sets, lets, lists and inherit

Nix's `ExprAttrs::eval` creates a recursive environment frame before installing recursive fields. It allocates lazy values for fields, then exposes those values in the set. For recursive sets, normal fields are evaluated in the recursive environment; inherited fields and `inherit (expr)` have distinct environment rules. Dynamic attributes are evaluated after the static/recursive bindings have been prepared.

`ExprLet::eval` likewise creates a new environment, installs each binding as a delayed value, and evaluates the body in that frame. `ExprList::eval` stores each list element as a delayed value rather than recursively evaluating every element immediately.

The Rust implementation follows the same broad knot-tying approach for recursive let bindings, recursive attrsets and list members. `inherit (expr) name` is represented as a deferred projection, so an unused inherited member need not force the source expression. Exact inherited-binding corner cases, duplicate handling, recursive dynamic attribute names, `__overrides`, and detailed error positions still need differential tests against Nix.

### Import is not parsing

Nix's file evaluation path resolves a source path, checks its import-resolution and file-evaluation caches, parses the file, and evaluates the resulting expression. It is called by the `import` primitive. This means import is not part of the lexer grammar; it combines language-level binding with source resolution, file identity, caching, and host policy.

The library keeps this operation behind `Host::load_source`. An embedding application can implement a filesystem, virtual filesystem, remote source registry, immutable source tree, or restricted resolver. The current adapter does not yet implement Nix's path canonicalization, search path, cache identity, cycle diagnostics, or evaluation cache.

### Where NixOS-specific behavior belongs

Nix's production evaluator is connected to the Nix store, derivation model, fetcher subsystem, source accessors, URI policy, string contexts and import-from-derivation machinery. This is visible in the evaluation interfaces and in primitive-operation implementations that call those services.

For an embeddable expression interpreter, those capabilities must be explicit host interfaces or separately layered packages:

- Import/source loading: source identity, relative resolution, search paths, access control, caching.
- Path operations: path normalization, filesystem abstraction, source accessor identity and path realization.
- String contexts: propagation of store references and realization when a string is consumed by store-aware operations.
- Fetchers: HTTP/Git/archive policy, hashes, cache, credentials and store insertion.
- Derivations/store: derivation construction, output paths, realization and import-from-derivation.

The current `Host` trait only provides source loading. Native functions are registered separately with `register_native`, and their arguments are lazy `Thunk` handles. This is sufficient to demonstrate the extension point, but it is not yet the complete host capability API needed for Nixpkgs evaluation.

## Nickel: what is reusable and what is not

Nickel is useful as a Rust implementation study, not as the language front-end for this crate. Nickel's `core/src/eval/cache/lazy.rs` uses a thunk state with suspended, blackholed and evaluated states, closely matching the call-by-need / blackhole idea. Its comments also explain revertible thunks for recursive records and overriding: a recursive record is modeled as a fixpoint, and selected thunks can be restored when recomputing that fixpoint after a merge.

That revertible-recursive-record mechanism is specific to Nickel's record merge and contract model. It should not be copied wholesale into Nix. Nix recursive attrsets have their own environment/attribute-binding semantics. Likewise, Nickel's typed term/value and contract machinery is not part of Nix syntax and is intentionally absent here.

The reusable lessons are narrower: separate syntax from evaluated values, isolate lazy cache state, make blackhole detection explicit, keep recursive environment ownership deliberate, and make evaluation errors contextual. The Nix grammar and Nix semantics remain authoritative.

## Rust design decisions

The guiding principle is to use Rust's type system and ownership model rather than reproducing class-oriented patterns mechanically.

- **Closed AST dispatch:** rnix's `Expr` is a closed enum. An exhaustive `match` acts as a compact visitor and makes new AST variants compiler-visible. A runtime plugin registry for grammar nodes would make core language semantics less predictable; host extensibility belongs at effects and native-function bindings.
- **Newtype handle:** `Thunk` wraps an arena index so a thunk cannot be confused with an arbitrary integer. The handle is session-local.
- **Arena ownership:** environments and thunks are owned by `EvalContext`; values retain handles instead of owning recursive environment graphs. This is preferable to accidental reference-counted cycles for recursive values.
- **Strategy boundary:** `Host` is a trait for interchangeable source-resolution strategies. It can be implemented by a real filesystem, virtual source store, remote registry or test fixture.
- **Adapter:** `LoaderHost` wraps a closure-based loader for embedding applications that do not need a named host type.
- **Explicit capabilities:** native functions are registered by the embedding application. The expression evaluator itself does not silently acquire network, filesystem, process or store capabilities.
- **Single mutable session:** the initial `EvalContext` uses `&mut self` and `Rc`; it is intentionally single-threaded. Concurrency should be designed with a clear evaluation-session boundary rather than adding locks to each thunk prematurely.

Traditional Visitor, Strategy and Adapter descriptions are useful vocabulary, but the correct implementation depends on Rust's enum dispatch, traits, generics, borrowing and ownership. The Rust Patterns catalogue also emphasizes idioms and anti-patterns, not only named GoF patterns.

## References

### Nix
- [Nix parser grammar](https://github.com/NixOS/nix/blob/master/src/libexpr/parser/parser.y)
- [Nix expression AST](https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/nixexpr.hh)
- [Nix runtime values](https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/value.hh)
- [Nix evaluator interface](https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/eval.hh)
- [Nix thunk forcing](https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/eval-inline.hh)
- [Nix evaluator implementation](https://github.com/NixOS/nix/blob/master/src/libexpr/eval.cc)
- [Nix primitive operations](https://github.com/NixOS/nix/blob/master/src/libexpr/primops.cc)
- [Nix language reference](https://nix.dev/manual/nix/2.34/language)

### Nickel
- [Nickel lazy evaluation cache](https://github.com/nickel-lang/nickel/blob/master/core/src/eval/cache/lazy.rs)
- [Nickel term representation](https://github.com/nickel-lang/nickel/blob/master/core/src/term/mod.rs)
- [Nickel runtime values](https://github.com/nickel-lang/nickel/blob/master/core/src/eval/value/mod.rs)

### Rust patterns
- [Rust Design Patterns catalogue](https://rust-unofficial.github.io/patterns/)
- [Refactoring Guru: Rust design patterns](https://refactoring.guru/design-patterns/rust)
- [Refactoring Guru Rust examples](https://github.com/RefactoringGuru/design-patterns-rust)
