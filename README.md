# nix-expr-lang

An embeddable Rust API for evaluating the real Nix expression language, backed by the upstream Nix evaluator (libexpr).

This project does not reimplement the Nix grammar, runtime, lazy evaluation, closures, builtins, coercion rules, string contexts, or store semantics. It uses the existing Nix C API through the Rust nix-bindings wrapper. Nix parses and evaluates expressions; this crate provides an embedding-oriented API and a place for applications to register their own native operations.

The old experimental Rust evaluator has been removed on this branch. It was a partial implementation and could not provide complete Nix semantics.

## What is evaluated

The evaluation engine is the installed Nix version's own libexpr. That includes the language constructs, lazy evaluation, builtins and evaluator behavior implemented by that Nix release. This is not a Nickel interpreter and does not translate Nix expressions into another language.

This is also not a replacement for the Nix CLI or all Nix services. Features such as derivation realization, store operations, network fetching, flakes, and import behavior depend on the linked Nix libraries, configured store, permissions and optional features. The Nix expression evaluator itself is upstream code, not a compatibility subset maintained here.

## Requirements

- Nix with its development libraries and C API headers installed.
- Rust 1.95 or later.
- pkg-config, used by nix-bindings build scripts to locate the installed Nix libraries.

The Rust binding is pinned to a specific upstream revision so the wrapper API does not silently drift. The pinned wrapper targets Nix 2.35.6. Use a matching Nix installation and consult the binding's build documentation for platform-specific requirements.

## Add as a dependency

During development, use this branch:

~~~toml
[dependencies]
nix-expr-lang = {
  git = "https://github.com/ep0ll/nix-expr-lang",
  branch = "feat/libexpr-runtime-backend"
}
~~~

## Evaluate Nix expressions

~~~rust
use nix_expr_lang::{EvaluatorBuilder, NixValueOps, Result};

fn main() -> Result<()> {
    let evaluator = EvaluatorBuilder::new()?.build()?;

    let value = evaluator.eval(r#"
        let
          add = x: y: x + y;
          unused = throw "this binding is never forced";
        in
          add 19 23
    "#)?;

    assert_eq!(value.as_int()?, 42);
    Ok(())
}
~~~

Evaluator::eval returns the upstream nix_bindings::Value. It is a handle into Nix's GC-managed runtime and borrows the evaluation session. Keep the Evaluator alive while using values returned by it.

For a source containing relative paths or imports, pass the base directory explicitly:

~~~rust
let value = evaluator.eval_at(
    "import ./config.nix",
    "/work/project",
)?;
~~~

Or evaluate a file directly:

~~~rust
let value = evaluator.eval_file("/work/project/default.nix")?;
~~~

## Bind application logic as native Nix primops

Nix's C API supports registering custom primitive operations. The wrapper exposes these as Rust callbacks. This is the intended integration point for application-specific capabilities such as artifact resolution, internal registries, custom source providers, or organization-specific fetchers.

~~~rust
use nix_expr_lang::{
    EvaluatorBuilder,
    NixValueOps,
    Result,
    primop::PrimOpRet,
};

fn main() -> Result<()> {
    let mut builder = EvaluatorBuilder::new()?;

    builder.register_primop(
        "host_double",
        1,
        Some("Double an integer using the embedding application"),
        |args, result: &mut PrimOpRet<'_>| {
            let number = args[0].as_int()?;
            result.set_int(number * 2)
        },
    )?;

    let evaluator = builder.build()?;
    let value = evaluator.eval("builtins.host_double 21")?;

    assert_eq!(value.as_int()?, 42);
    Ok(())
}
~~~

Primops are registered in Nix's process-wide registry. Register them before creating the evaluator state that should see them. The callback's argument and return wrappers are valid only for the duration of the callback; do not store them or move them to another thread.

A custom fetcher can call the application's HTTP client, verify hashes, add content to the Nix store using the store API, and return a Nix path. The policy and network implementation stay in the embedding application; Nix remains responsible for evaluating the expression that invokes the function.

## Configure the evaluator

~~~rust
use nix_expr_lang::{EvaluatorBuilder, Result};

fn main() -> Result<()> {
    let mut builder = EvaluatorBuilder::with_store_uri("local")?;
    builder
        .lookup_path([
            "nixpkgs=/nix/var/nix/profiles/per-user/root/channels/nixpkgs",
        ])
        .no_load_config();

    let evaluator = builder.build()?;
    let value = evaluator.eval("<nixpkgs>")?;
    println!("{}", value.value_type());
    Ok(())
}
~~~

no_load_config() skips Nix configuration and environment-derived evaluator settings. It does not imply pure evaluation, filesystem isolation, or a security sandbox.

## API surface

The crate re-exports useful upstream types: Context, Store, EvalState, EvalStateBuilder, Value, ValueType, NixValueOps, Error, and Result. The underlying EvalState can be accessed through Evaluator::state().

Use the upstream Value methods for:
- Forcing values to weak-head normal form or deep evaluation.
- Reading scalar values and Nix string contexts.
- Reading attributes and list items lazily.
- Calling Nix functions.
- Creating values, paths, lists and attribute sets.
- Working with store paths and derivations.

See the nix-bindings documentation for the complete low-level and high-level APIs.

## Architecture

~~~text
Embedding application
    |
    | EvaluatorBuilder / custom primops / Nix store APIs
    v
nix-expr-lang (this crate)
    |
    | safe Rust wrappers
    v
nix-bindings
    |
    | generated C bindings
    v
Nix C API (libexpr, libstore, libutil)
    |
    v
Nix parser + evaluator + lazy runtime + builtin implementations
~~~

The boundary is intentionally small: this crate adapts the upstream evaluator instead of cloning it. See docs/architecture.md for the design details and upstream responsibilities.

## Scope and caveats

- The expression parser and evaluator are provided by the installed Nix version. This crate does not claim to extend or out-version the Nix language.
- The Nix C API is experimental and may change between Nix versions. The nix-bindings project tracks these interfaces and contains the unsafe FFI layer.
- The nix-bindings project also has optional C++ shims. They are deliberately disabled by default here. The cpp-shim feature is opt-in for cases that require a capability not exposed by the C API.
- Flake operations are available behind the flakes feature when supported by the linked Nix version.
- The evaluator is not automatically sandboxed. Do not evaluate untrusted expressions with unrestricted imports, store access, or powerful custom primops.

## Development status

This branch changes the implementation strategy from a homegrown subset to the upstream evaluator. Compilation and integration tests must be run on a machine with a compatible Nix development installation. A successful GitHub commit alone is not evidence of a successful native build.
