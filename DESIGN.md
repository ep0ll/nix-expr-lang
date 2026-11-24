# Design: delegate language semantics to Nix libexpr

## Decision

The project uses the upstream Nix expression evaluator through the public Nix C API and a maintained Rust binding. It no longer owns a second parser, AST evaluator, thunk cache, closure implementation, or builtin table.

This is the credible way for an embedding library to offer complete evaluation of the Nix expression language without maintaining a second implementation of Nix semantics.

The language implementation is versioned by the installed Nix libraries. This crate's job is to present an ergonomic and stable embedding boundary, not to outpace the upstream interpreter.

## Components

~~~text
Application
  |-- evaluator configuration
  |-- Nix store / filesystem policy
  |-- custom primops and application services
  v
nix-expr-lang
  |-- EvaluatorBuilder
  |-- Evaluator
  |-- primop convenience API
  v
nix-bindings
  |-- safe Rust wrappers
  |-- Value / EvalState / Store / Context
  |-- PrimOp / external value adapters
  v
Nix C API
  |-- libexpr
  |-- libstore
  |-- libutil
  v
Nix internals
  |-- lexer and parser
  |-- expression AST
  |-- lazy evaluation and thunk forcing
  |-- closures and recursive bindings
  |-- builtin and primop implementations
  |-- path and string-context semantics
~~~

### Why not own an independent Rust evaluator?

A partial evaluator is deceptively easy to mistake for a Nix interpreter. Full compatibility requires far more than parsing common constructs. Semantics include lazy evaluation and blackholing, recursive scope resolution, function-pattern behavior, attribute set merge/update behavior, string contexts, paths, import resolution, error traces, numeric edge cases and the versioned builtin surface.

A homegrown implementation would also have to track changes to Nix. Even a comprehensive differential test suite cannot make a separately maintained evaluator free of semantic drift. The upstream evaluator already exists, is used by Nix itself, and is the definition of behavior for a matching Nix release.

## Native evaluation path

Evaluator::eval and Evaluator::eval_at call EvalState::eval_from_string. The Rust binding calls nix_expr_eval_from_string from nix_api_expr.h. Nix then:
1. Parses the source through its own parser.
2. Constructs its own expression representation.
3. Evaluates through EvalState.
4. Forces the top-level result to weak-head normal form.
5. Returns a GC-managed nix_value wrapper to Rust.

Evaluator::eval_file uses the Rust binding's file evaluator. Nix import expressions remain native Nix expressions and are handled by the linked evaluator, not by a Rust callback in this crate.

The result Value preserves Nix's lazy runtime. Attribute members and list items remain lazy until the upstream value API or an expression forces them. The binding provides both lazy and forcing operations.

## Native integrations

### Store

EvaluatorBuilder creates or accepts a nix_bindings::Store. Applications can reuse a store they already own via EvaluatorBuilder::from_parts(context, store). Store realization, path validation, copying, and derivation operations should use the upstream store API rather than recreating Nix store formats.

### Primops

EvaluatorBuilder::register_primop adapts the safe nix_bindings::primop::PrimOp interface.

Primops are the extension point for application operations that should appear inside Nix expressions. Examples:
- An organization-specific artifact fetcher.
- A package source resolver backed by an internal service.
- An immutable configuration registry.
- A secret or credential lookup that follows application-specific access policy.
- A custom data source or content-addressed cache.

The callback receives Nix values in a callback-scoped lifetime. Nix may pass lazy thunks; the callback should force only what it needs. A callback must not retain its borrowed argument/return wrappers past the call.

Nix primop registration is process-global in the C API. A primop must be registered before constructing evaluator states that should contain it. This has consequences for tests and multi-tenant embedding: different per-session primop registries cannot be assumed. Use one process-wide registry or expose a namespaced dispatch primop that consults an application-owned registry.

### External values

The upstream wrapper's external-value API allows Rust values to be represented as Nix external values with type-safe downcasting. Use this when a value is opaque to Nix expressions but should cross the language boundary. It is not a substitute for a serializable Nix value and should be used deliberately.

### Flakes

Flake APIs are optional and are exposed behind the crate's flakes feature. Flake evaluation and lockfile behavior are outside the core expression language API but can be added by applications that need them.

## C API and C++ shims

The primary backend uses the public Nix C API. It does not include or call Nix private C++ headers from this crate. Nix's C API is marked experimental upstream, so API and ABI compatibility across Nix releases must be treated as a dependency-management concern.

The Rust binding has optional C++ shims for operations not exposed by the C API. These are excluded from the default build. Enable cpp-shim only when a concrete integration requires it, and isolate such use behind a feature-gated module.

C++ #include lines are build-time implementation dependencies. They are unrelated to the Nix expression grammar. The language-level import keyword is evaluated by libexpr and may consult filesystem, search-path, or store facilities.

## Versioning and compatibility

The dependency is pinned to a specific Git revision rather than a floating branch. The pinned nix-bindings revision tracks Nix 2.35.6 and requires Rust 1.95.0. The linked Nix development libraries must be compatible with the binding's expected headers and C API.

The effective language semantics are therefore:

nix-expr-lang API version + nix-bindings revision + linked Nix libexpr version

A release process should:
1. Update the pinned binding revision intentionally.
2. Build against the targeted Nix development package.
3. Run the integration suite against that exact Nix runtime.
4. Review the Nix C API changes and binding release notes.
5. Record the supported Nix version range.

## Why the C++ bindings are not the default

Nix's C++ evaluator types and implementation headers are not a stable public ABI. Binding directly to EvalState, Value, Expr, parser internals or private headers would couple this crate to Nix implementation details and often to exact compiler/build flags.

The public C API already exposes evaluation from a string, function application, forcing, value access, value construction, primop registration, and store functionality. Use that seam first. The experimental C++ shim remains an explicitly opt-in escape hatch.

## Threading and capability model

Nix evaluator state is mutable and backed by Nix's garbage collector. Follow the upstream wrapper's thread-safety rules: construct, use and drop Nix contexts/evaluator wrappers on a compatible thread; do not assume a mutex turns them into freely shareable concurrent objects. Primop callback handles are frame-scoped.

The embedding application controls powerful effects:
- Store selection and permissions.
- Lookup paths and import roots.
- Whether ambient Nix configuration is loaded.
- Whether pure evaluation or other evaluator restrictions are enabled.
- Which custom primops are registered and what those callbacks can access.

This crate does not claim to sandbox untrusted Nix code. Use Nix's restriction settings and OS-level isolation where a security boundary is required.

## Future work

The next work should improve the embedding surface, not reimplement evaluator internals:
- Add a capability-oriented host registry for configuring primops and services.
- Wrap source access / path policy and Nix settings where the public C API permits.
- Add optional flake and store convenience adapters.
- Add version/build diagnostics and native integration tests.
- Document Nix library installation for Linux/macOS and supported platforms.
