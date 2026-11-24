# Native libexpr architecture

The implementation delegates expression semantics to Nix. This file records the boundary and explains why that is preferable to implementing a second evaluator.

## Relevant upstream Nix components

| Nix component | Upstream location | Responsibility |
| --- | --- | --- |
| Parser grammar | src/libexpr/parser/parser.y and lexer sources | Nix language syntax and parsing. |
| Expression nodes | src/libexpr/include/nix/expr/nixexpr.hh | Internal representation of Nix expression forms. |
| Runtime values | src/libexpr/include/nix/expr/value.hh | Scalars, paths, lists, attribute sets, functions, thunks. |
| Evaluator | src/libexpr/include/nix/expr/eval.hh, src/libexpr/eval.cc | Environments, evaluation, application, coercion, errors. |
| Lazy forcing | src/libexpr/include/nix/expr/eval-inline.hh | Forces thunks and detects blackholes. |
| Builtins and primops | src/libexpr/primops.cc and other primop files | Nix builtins; many are connected to store/fetcher libraries. |
| Public C interface | src/libexpr-c/nix_api_expr.h, nix_api_value.h | Supported embedding operations. |

The public C API's nix_expr_eval_from_string function invokes Nix's native parser and evaluator. Its implementation calls parseExprFromString, eval, and forceValue on the Nix EvalState. The Rust façade does not reimplement these operations.

### Nix lazy values

Nix's Value representation includes thunks. EvalState::forceValue turns a thunk into a blackhole before evaluating the expression into the value slot. Re-entering the same value while it is blackholed is treated as infinite recursion. Evaluating only the outer constructor is Nix's normal weak-head-normal-form behavior: a list or attribute set can be returned while its members are still delayed.

The upstream Rust wrapper exposes Value, lazy value inspection, forcing and function application without converting every value into eager Rust data. Retain and use those wrappers when lazy semantics matter. Calling force_deep is an explicit operation and can diverge or overflow on recursive structures.

### Import and file resolution

The expression-level import operation is not a C/C++ preprocessor include. It is a Nix expression/builtin operation that causes Nix to resolve and evaluate another source file. File evaluation, relative paths, search paths, source accessors and import caching are handled by libexpr and the wider Nix runtime.

C++ #include lines seen in Nix's source tree are ordinary build dependencies among Nix implementation headers. They do not belong in the language lexer or parser.

## Why Nickel was studied, but not adopted as the evaluator

Nickel is valuable for Rust implementation ideas. Its lazy cache has the suspended/blackholed/evaluated thunk model, and its comments explain recursive records and revertible thunks. However, Nickel is a different language with a different type system, syntax, merge model and runtime contracts. Its evaluator cannot stand in for Nix.

The first experimental branch of this repository copied a small subset of the Nix language into a custom Rust evaluator. That work was removed from this backend branch because a subset implementation cannot honestly be called complete Nix evaluation.

## Existing Rust binding

The project uses NotAShelf/nix-bindings, a safe Rust API layered over generated bindings for Nix's public C API. Its responsibilities include:
- Managing Nix contexts, stores, evaluator states and value references.
- Calling the native string/file evaluator.
- Reading and constructing values.
- Calling Nix functions and forcing lazy values.
- Inspecting attributes and list elements.
- Registering Rust-backed primops.
- Representing external Rust values.

This repository adds a smaller domain API around those capabilities and intentionally keeps the binding visible for advanced users.

## Primop registration

A primop is a Nix primitive function implemented by a host callback. The upstream C API exposes primop allocation and registration. The Rust wrapper provides callback-scoped argument and return types, catches Rust panics before they cross the C ABI, and translates callback errors into Nix errors.

Global registration has process scope and affects evaluator states created after registration. It is not a per-Evaluator registry. For applications requiring isolated tenants, register one namespaced primop and dispatch from it using an application-owned capability registry; do not assume separate Nix evaluator instances have independent global builtin namespaces.

Callbacks may force arguments selectively. A custom fetcher can read a lazy attribute-set argument field-by-field, call application code, put the result into a Nix store, and return an appropriate Nix value. The evaluator itself does not need to know how that fetcher works.

### Nix's existing fetchers

Do not reimplement a fetcher that Nix already provides. The native language includes builtins such as builtins.fetchurl, builtins.fetchTarball, builtins.fetchGit and builtins.fetchClosure. They run inside the upstream evaluator and use Nix's own fetcher/store implementation. Whether a particular fetch operation is available depends on the linked Nix version and evaluator restrictions; for example, fetchTarball and fetchurl are unavailable in restricted evaluation mode.

A host primop is appropriate when an application needs a different source provider, credential model, cache, audit policy, or artifact registry. Otherwise, using the existing Nix builtin preserves upstream behavior and avoids duplicating fetcher semantics.

## C API versus C++ implementation headers

Use the C API by default. The underlying C++ classes are internal implementation details with no general stable ABI guarantee. The binding project offers opt-in C++ shims, but they are experimental and should only be introduced to cover a demonstrated gap in the C API.

The default crate feature set therefore includes expression evaluation, primops and external values but excludes the optional C++ shim. Store and filesystem behavior are still provided by Nix's C API and the Rust wrappers.

## Rust design patterns and ownership

The wrapper follows Rust's ownership model rather than mirroring the C++ class hierarchy.

- **Adapter:** Evaluator and EvaluatorBuilder adapt the native Nix C API to an embedding-oriented Rust interface. They do not shadow or translate Nix runtime values.
- **Dependency injection:** from_parts accepts a Context and Store created by the embedding application. This lets an application reuse its established store connection, logging context and configuration.
- **Strategy / callback boundary:** register_primop accepts a closure, so a host application can inject HTTP clients, credential resolvers, internal artifact services, or mocks. Larger integrations can capture an Arc-backed service trait object.
- **RAII:** nix-bindings wrappers release Nix GC references and C handles through Drop. Value carries a lifetime tied to EvalState so a value handle cannot safely outlive the evaluator that owns its runtime.
- **Feature-gated capabilities:** C++ shims and flakes are opt-in Cargo features. The default build stays on the public C API.
- **No custom Visitor over Nix expressions:** the C++ evaluator already owns the grammar, expression nodes and exhaustive evaluation dispatch. Recreating these in Rust would duplicate the source of truth and invite language drift.

The primop closure is constrained by Send + Sync + 'static because it is retained by Nix's global primop registry. Its argument and result wrappers are callback-frame values and must not escape the call. For per-tenant or request-local services, prefer a process-registered dispatcher primop that consults an application-owned capability registry rather than registering a separate global primop for every tenant.

## References

- Nix 2.35.6 release notes: https://nix.dev/manual/nix/2.35/release-notes/rl-2.35.html
- Nix C API reference: https://nix.dev/manual/nix/2.35/c-api
- Nix expression evaluation manual: https://nix.dev/manual/nix/2.35/language/evaluation.html
- Nix expression AST: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/nixexpr.hh
- Nix runtime values: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/value.hh
- Nix evaluator: https://github.com/NixOS/nix/blob/master/src/libexpr/eval.cc
- Nix forcing logic: https://github.com/NixOS/nix/blob/master/src/libexpr/include/nix/expr/eval-inline.hh
- Nix C API evaluator implementation: https://github.com/NixOS/nix/blob/master/src/libexpr-c/nix_api_expr.cc
- Nickel lazy evaluation cache: https://github.com/nickel-lang/nickel/blob/master/core/src/eval/cache/lazy.rs
- nix-bindings: https://github.com/NotAShelf/nix-bindings
