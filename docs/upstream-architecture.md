# Nix and Nickel implementation study

This is a source-oriented comparison of the Nix expression evaluator and Nickel's Rust evaluator. It explains why the current implementation delegates Nix expression semantics to Nix's own libexpr rather than translating either evaluator into another partial interpreter.

## Nix is the semantic authority

The Nix source tree separates parser grammar, expression nodes, runtime values, evaluation state and primitive operations:

| Layer | Upstream source | Role |
| --- | --- | --- |
| Grammar | src/libexpr/parser/parser.y and lexer sources | Tokenizes and parses Nix expressions. |
| Expression nodes | src/libexpr/include/nix/expr/nixexpr.hh | Represents parsed language constructs such as variables, lambdas, let, attrsets, lists and operators. |
| Runtime values | src/libexpr/include/nix/expr/value.hh | Represents scalars, strings, paths, lists, attrsets, functions, thunks and external values. |
| Evaluation state | src/libexpr/include/nix/expr/eval.hh | Owns evaluation memory, environments, symbol tables, settings, source/cache state and store integration. |
| Forcing | src/libexpr/include/nix/expr/eval-inline.hh | Forces delayed values and handles recursive blackholes. |
| Evaluation implementation | src/libexpr/eval.cc | Evaluates AST nodes, resolves variables, creates closures, implements recursive bindings, file evaluation and equality. |
| Primitive operations | src/libexpr/primops.cc and related modules | Implements builtins, imports and operations connected to store/fetchers. |
| Public embedding boundary | src/libexpr-c/nix_api_expr.h, nix_api_value.h and related headers | Exposes expression evaluation, values, function application, primops and store operations to embedders. |

The language manual defines observable semantics; implementation source explains how Nix realizes them. The code should not assume every internal detail is an API contract.

### C API evaluation call chain

The public C API entry point nix_expr_eval_from_string is the crucial integration seam. Its implementation creates a source path, asks EvalState to parse the supplied Nix source, evaluates the parsed expression into a Nix Value, and forces that outer value. The Rust nix-bindings wrapper exposes this through EvalState::eval_from_string.

Consequences for this repository:

1. The real Nix lexer and parser are used. There is no parallel Rust grammar.
2. Nix's own expression AST and evaluator dispatch are used. There is no Rust AST visitor that must track every language feature.
3. Nix owns evaluation strictness, thunk caching, recursive environments, function calls and errors.
4. The returned Rust Value is a handle to a Nix runtime value. It is not an eager conversion into Rust enums or JSON.
5. Calls that need store, fetcher or filesystem behavior continue through the linked Nix runtime and its configured capabilities.

A language interpreter can be complete relative to a particular Nix release even though the C API is experimental and does not expose every CLI feature or every internal implementation hook. Those are separate claims. This project delegates expression evaluation to libexpr while documenting the C API boundary and version requirement.

## Nix lazy evaluation

### Thunks and blackholing

A Nix thunk stores an expression and the environment in which that expression was created. It is not just a memoized Rust closure: it participates in Nix's native Value representation and evaluator memory model.

The relevant forcing path is EvalState::forceValue in eval-inline.hh. When a value is a thunk, Nix:
- Reads the thunk's expression and environment.
- Changes the value slot into a blackhole before evaluating it.
- Evaluates the expression into the value slot.
- Reuses the resulting value on subsequent forces.
- Detects a recursive attempt to force the same blackholed slot and reports infinite recursion.
- Preserves evaluation failure details for diagnostic handling.

This is call-by-need rather than call-by-name: an expression is computed at most once per thunk and the result is shared. Evaluating a list or attrset normally creates the outer container and delayed members, not a deep eager tree. Nix's explicit deep-force operations are different operations and can diverge on infinite or recursive values.

The current crate keeps these semantics in Nix. Its public surface returns the upstream Value and exposes force, force_deep, get_attr_lazy, list_get_lazy, and ordinary forcing accessors through nix-bindings.

### Lexical environments and with

Nix has both statically resolved lexical bindings and dynamically searched with scopes. In eval.hh an Env points to an outer Env and contains runtime slots. ExprVar carries a lexical level/displacement and a marker for variables resolved through with. lookupVar walks to the selected frame; ordinary variables read their slot, whereas a with-resolved variable searches the runtime attribute set.

This distinction matters: an evaluator that simply walks a chain of HashMaps and checks a with map at each frame can get shadowing order wrong. The production Nix evaluator already implements the actual rules; this repository does not introduce its own name resolver.

### Recursive attrsets and let

ExprAttrs::eval builds a recursive environment frame before exposing recursive fields. Attribute expressions become delayed runtime values tied to the correct environment. Inherited fields and inherit-from expressions have distinct environment rules; dynamic attributes are evaluated in a later phase. Nix also supports override-specific behavior and extensive duplicate/error position handling.

ExprLet::eval similarly allocates an environment for the bindings and evaluates the body in that environment. The bindings are delayed and can refer to one another. ExprList::eval stores delayed element values.

These details explain why the first experimental homegrown evaluator was not suitable as a complete interpreter. Matching the surface syntax is not enough; the evaluation environment and strictness rules are subtle, and changes in Nix's internal implementation would otherwise have to be reimplemented continuously.

### Equality and ordering

Nix equality forces the compared outer values and then compares supported types recursively. Lists and attribute sets require recursive element/field comparisons; functions are not comparable in the ordinary equality sense. Mixed integer/float comparison is special-cased. Derivation-like attribute sets have additional semantics involving outPath.

Ordering is a separate operator family with Nix-specific type restrictions. A Rust implementation that uses ordinary derived PartialEq on a recursive value tree would not reproduce those rules, and it could accidentally force fields or recurse forever in the wrong places.

### Builtins and fetchers

Nix's builtins are not a separate language grammar. They are runtime operations installed into the evaluator. The native evaluator contains operations such as fetchurl, fetchTarball, fetchGit and fetchClosure. These interact with the store, fetcher settings, URI policy, caches and purity/restricted-evaluation rules.

The Nixpkgs library also supplies many higher-level fetcher helpers as Nix expressions. Those are evaluated by the same interpreter after importing Nixpkgs. They are not parser features.

This leads to two supported integration modes:
- Use the builtin or Nixpkgs function when it already implements the desired operation.
- Register a native primop when the embedding application needs a custom source provider, credentials policy, cache, registry, or external library.

Custom primops execute inside the real Nix evaluator. Their arguments may be lazy Nix values, and the callback chooses which values to force. The Rust wrapper requires the callback to be Send + Sync + 'static because registration is process-wide and the callback is retained by Nix. Callback argument/result wrapper values have a callback-scoped lifetime and must not escape.

## Import, source files and C++ includes

The Nix expression keyword/builtin import is a language-level evaluator operation. It combines source/path resolution, parsing, evaluation and the Nix runtime's source/file caching. It is not a lexer directive.

The C++ #include directives in Nix's implementation are ordinary build-time dependencies between C++ headers. They do not belong in Nix source parsing and they should not become a language feature in this crate.

The wrapper's eval_from_file and eval_from_string APIs pass source into Nix's native evaluator. For application-owned source resolution, use the APIs and policy hooks exposed by Nix where possible. Do not build a second import system unless the application explicitly needs a virtual source abstraction that Nix cannot provide through its public interfaces.

## Nickel: useful implementation study, different language

Nickel's evaluator is implemented in Rust and is a useful source of design ideas. Its runtime has a distinct term representation, runtime value representation, evaluator cache and transformation pipeline. The following files are particularly relevant:

- core/src/eval/cache/lazy.rs
- core/src/term/mod.rs
- core/src/eval/value/mod.rs
- core/src/eval/value/lens.rs
- core/src/term/record.rs

### Nickel thunk states

Nickel models thunk state as Blackholed, Suspended or Evaluated. Its lazy cache documentation explains the same fundamental invariant as Nix: mark a suspended computation as blackholed before evaluating it; a recursive force then identifies a blackhole instead of looping without bound; after successful evaluation the thunk contains its computed value.

Nickel also stores evaluation data in explicit runtime terms and uses value lenses to mutate parts of shared runtime values. This reflects Nickel's own memory representation and transformation pipeline.

### Revertible thunks and recursive records

One particularly interesting Nickel mechanism is the revertible thunk. Nickel's comments explain recursive records as a fixpoint, conceptually similar to a function that receives the record itself. Revertible thunks memoize computations derived from this fixpoint but can restore their original expression when a recursive merge or override requires recomputation.

That mechanism is specific to Nickel's merge/contract and recursive-record semantics. Nix recursive attribute sets have a different evaluator implementation, environment setup and override model. Copying Nickel's revertible-thunk design into a Nix interpreter would not produce Nix semantics.

### What is reusable

The reusable lessons are architectural:
- Keep parsed syntax separate from runtime values.
- Isolate lazy evaluation state.
- Detect blackholes before recursively evaluating a thunk.
- Keep ownership of recursive environments explicit.
- Make forcing and deep forcing distinct operations.
- Treat diagnostics, source positions and error traces as evaluator responsibilities.
- Use Rust ownership, borrowing and typed handles to prevent runtime values escaping the state that owns them.

The current branch deliberately uses the Nix implementation for these semantic responsibilities. Nickel informs the design review, but is not a dependency or evaluator backend.

## Rust patterns used by this crate

The wrapper uses a small number of patterns naturally suited to Rust:

- **Adapter:** EvaluatorBuilder and Evaluator adapt the native C API into an embedding-friendly facade. The underlying EvalState and Value APIs remain available.
- **RAII:** nix-bindings value/store/state wrappers release C and GC handles through Drop.
- **Lifetimes as ownership contracts:** Value<'a> is tied to its EvalState; a host cannot safely retain a value handle after the evaluation state is dropped.
- **Dependency injection:** EvaluatorBuilder::from_parts accepts an application-owned Context and Store.
- **Strategy / callback extension:** custom primops inject behavior without changing the parser or evaluator implementation.
- **Feature-gated integrations:** experimental C++ shims and flake APIs are optional build features rather than implicit dependencies.

A Visitor implementation for Nix AST nodes is unnecessary here because Nix itself owns the AST and dispatch. Recreating those nodes in Rust would duplicate upstream semantics. The extension seam is host effects and primops, not plugin-defined syntax.

## References

### Nix
- Nix parser grammar: https://github.com/NixOS/nix/blob/master/src/libexpr/parser/parser.y
- Nix AST: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/nixexpr.hh
- Nix runtime values: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/value.hh
- Nix evaluator interface: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/eval.hh
- Nix thunk forcing: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/eval-inline.hh
- Nix evaluator implementation: https://github.com/NixOS/nix/blob/master/src/libexpr/eval.cc
- Nix public C API: https://github.com/NixOS/nix/blob/master/src/libexpr-c/nix_api_expr.h
- Nix C API evaluator implementation: https://github.com/NixOS/nix/blob/master/src/libexpr-c/nix_api_expr.cc
- Nix builtins manual: https://nix.dev/manual/nix/2.35/language/builtins.html

### Nickel
- Nickel lazy cache: https://github.com/nickel-lang/nickel/blob/master/core/src/eval/cache/lazy.rs
- Nickel term representation: https://github.com/nickel-lang/nickel/blob/master/core/src/term/mod.rs
- Nickel runtime values: https://github.com/nickel-lang/nickel/blob/master/core/src/eval/value/mod.rs

### Rust
- Rust Design Patterns catalogue: https://rust-unofficial.github.io/patterns/
- Refactoring Guru: Rust design patterns: https://refactoring.guru/design-patterns/rust
- Refactoring Guru Rust examples: https://github.com/RefactoringGuru/design-patterns-rust

### Binding
- nix-bindings source and build requirements: https://github.com/NotAShelf/nix-bindings
