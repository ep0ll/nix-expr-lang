# nix-expr-lang

An embeddable Rust interpreter for the Nix expression language.

This project is a library first. It is intended to let applications evaluate Nix expressions and bind their own native functions, import resolvers, filesystem policies, fetchers, and other host services without embedding the Nix package manager or CLI.

## Design goals

- Nix syntax, not Nickel syntax.
- Call-by-need evaluation with memoized thunks and recursion detection.
- Lexical closures, recursive bindings, lazy lists, and lazy attribute values.
- A narrow host boundary for imports and custom source loading.
- Explicit registration of native functions; no implicit NixOS-specific builtins.
- Parser and evaluator are separate, with rnix providing the Nix grammar and Rowan syntax tree.

## Quick start

```toml
[dependencies]
nix-expr-lang = { git = "https://github.com/ep0ll/nix-expr-lang" }
```

```rust
use nix_expr_lang::{EvalContext, Result, Value};

fn main() -> Result<()> {
    let mut eval = EvalContext::new();

    eval.register_native("fetch_text", 1, |context, args| {
        let url = context.force(args[0])?;
        let Value::String(url) = url else {
            return Err(nix_expr_lang::Error::new(
                nix_expr_lang::ErrorKind::Type,
                "fetch_text expects a string URL",
            ));
        };

        // Call your own HTTP client, cache, credential provider, or test double here.
        Ok(Value::String(format!("fetched by host: {url}")))
    });

    let value = eval.eval(r#"
        let source = fetch_text "https://example.invalid/source";
        in { source = source; enabled = true; }
    "#)?;

    println!("{value:?}");
    Ok(())
}
```

Native callbacks receive lazy `Thunk` handles. A callback chooses when and whether to force each argument, so host functions can implement their own strictness and avoid evaluating unused arguments.

For import support, implement `Host::load_source`, construct the context with that host, and call `install_import()`. The host owns path resolution, source identity, permissions, network policy, and caching.

## Current status

This is an early foundation, not a complete or drop-in-compatible Nix evaluator. The current implementation covers a useful core of expressions and demonstrates the runtime/host separation. It still has important semantic gaps; see [DESIGN.md](DESIGN.md). In particular, do not use it to evaluate untrusted expressions or production Nixpkgs until the conformance and security work is complete.
