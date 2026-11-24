# nix-expr-lang

A reusable Rust library for parsing and evaluating the Nix expression language inside host applications.

> Early development: this repository is being initialized. The goal is a general-purpose expression interpreter, not a Nix CLI or a Nix store implementation.

The evaluator is designed around call-by-need evaluation, lexical closures, recursive bindings, lazy lists and attribute sets, and explicit host integration points. Parsing is delegated to `rnix` so the project does not maintain a second, partial Nix grammar.

## Scope

This crate focuses on the Nix expression language and the runtime needed to evaluate expressions. Nix store operations, derivation realization, filesystem policy, URI fetching, and package-specific fetchers are host responsibilities. Applications register these as native functions and provide import/path services through host interfaces.

## Status

The implementation is under active development. Do not treat it as a drop-in replacement for the Nix evaluator yet; consult `DESIGN.md` for the supported language surface and deliberate exclusions.
