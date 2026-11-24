# Architecture and implementation boundary

## Purpose

nix-expr-lang is a library for embedding the Nix expression language in another Rust application. It is not a Nix command-line frontend, package manager, evaluator daemon, Nix store client, or derivation builder. Those systems can use the crate and install their own services without making those services part of the expression language implementation.

The design separates four concerns:

1. Syntax parsing and source representation.
2. Nix language evaluation and lazy runtime values.
3. Host integration for source loading and native functions.
4. Application policy, which remains outside the interpreter.

## Why rnix

The lexer and parser are delegated to rnix. Its Rowan syntax tree preserves source text and syntax structure, and it exposes a typed AST for Nix constructs. Maintaining a second grammar would create a permanent compatibility burden: Nix syntax includes indented strings, interpolation, path forms, dynamic attribute names, set-pattern lambdas, recursive attrsets, legacy let syntax, and precedence-sensitive operators.

The interpreter consumes the AST and owns evaluation semantics. Delegating parsing does not imply every AST construct is already evaluated by this crate.

## Evaluation model

The runtime is call-by-need:

- A suspended thunk contains an expression, its lexical environment handle, and source identity.
- Forcing transitions a thunk from suspended to evaluating to evaluated.
- Re-entering an evaluating thunk reports infinite recursion (a black-hole check).
- Successful results and errors are memoized.
- Function arguments, list members, and attribute values are represented by thunk handles.
- let bindings share one recursive lexical frame.
- recursive attrsets expose their top-level fields through the environment captured by each field thunk.
- evaluation returns weak-head normal form. Constructing a list or attrset does not force its elements/fields.

The current implementation uses session-local arenas with integer handles. This avoids ownership cycles between recursive environments and delayed expressions, and makes an evaluation session a natural ownership boundary. The current context is single-threaded and is not Send/Sync.

## Lexical lookup and with

Identifier lookup uses two passes. First, it checks the entire lexical environment chain. Only if no lexical binding exists does it force and inspect with-scopes from inner to outer. This prevents an inner with namespace from shadowing a let or function parameter from an outer lexical frame.

## Host integration

### Native functions

No standard library is installed implicitly. Applications register globals and native functions explicitly.

A native callback receives lazy Thunk arguments and a mutable EvalContext. It can force an argument using context.force(handle), leave an argument unevaluated, or return a host-created Value. This lets a fetcher, filesystem helper, secret provider, or project-specific operation live entirely in the embedding application.

For example, an application can register fetchurl, fetchGit, readFile, or a company-specific artifact resolver. Those functions can use reqwest, a Nix store binding, a cache, credentials, or a mock service. None of those dependencies belong in the parser or expression evaluator.

### Imports

The optional import binding is installed by calling install_import(). It delegates to the Host trait. Host implementations decide how relative paths are resolved, how search paths work, what locations may be read, how source identity is canonicalized, how remote imports are allowed, and how imported source is cached.

The current Host interface is intentionally small. It is a seam for later policies; the current implementation does not yet provide complete Nix import semantics, cycle tracking, or a built-in filesystem resolver.

## Rust design choices

- Newtype handles prevent accidental mixing of arena indices with application integers.
- Exhaustive Expr matching makes newly added parser AST variants visible to the evaluator at compile time.
- Host is a Strategy boundary: local files, in-memory modules, remote registries, virtual filesystems, and test fixtures can provide different source-loading behavior.
- LoaderHost is an Adapter for closure-based loaders.
- EvalContext owns mutable runtime state; no process-global mutable singleton is needed.
- Thunks are memoized computations, while native callback registration is a controlled extension point.
- The runtime Value is distinct from the rnix syntax AST, avoiding accidental conflation of syntax and evaluated data.

## Implemented language surface in this milestone

The current evaluator includes integer and float literals, null/booleans, basic strings and interpolation, opaque path literals, identifiers, unary operations, arithmetic, comparisons for primitive values, boolean short-circuiting, lambdas, function application, simple and attribute-set pattern parameters, lists, non-recursive and recursive attribute sets, static/dynamic attribute names, nested attrpaths in attrsets, inherit bindings, let-in, with, if, assert, attribute selection with defaults, has-attribute, list concatenation, attrset update, and function-like attrsets via __functor.

This list describes the current code, not full conformance to Nix semantics.

## Known gaps and non-goals

A production-compatible Nix evaluator still requires substantial work:

- Full semantics for legacy let syntax, all syntax corner cases, and improved diagnostics/spans.
- Path interpolation, path normalization, search path resolution, URI parsing, and correct path-vs-string behavior.
- Nix-compatible string coercion and string context, including __toString, toString, store paths, and context-preserving concatenation.
- Exact Nix numeric representation and overflow/division behavior.
- Full equality, ordering, list and attrset operations, and subtle operator strictness.
- Correct function/set-pattern defaults, ellipsis semantics, duplicate handling, and argument error behavior.
- __curPos, throw/tryEval and the language-level error model.
- Stack-safe evaluation, recursion/call-depth limits, interruption/cancellation, and resource accounting.
- Import cycle detection, source caching, source maps, path policy, and full Nix search path behavior.
- Extensive differential tests against the reference Nix evaluator, including Nixpkgs expressions.

Nix builtins, primops, store operations, derivation realization, package fetchers, NixOS module evaluation, and CLI behavior are deliberately not bundled into this initial library. They are either host-provided integrations or separate layers. The goal is to make those layers pluggable, not to silently ship partial substitutes.

## Safety

An expression evaluator that can invoke host callbacks or import source is a capability boundary. Embedding applications must define an allowlist of native functions, constrain source loading, add evaluation limits, and decide whether expressions are trusted. The current milestone does not provide hardened resource limits or a sandbox.
