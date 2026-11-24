//! A reusable Rust façade over the upstream Nix expression evaluator.
//!
//! This crate intentionally does not implement Nix syntax or evaluation semantics itself.
//! Parsing, lazy evaluation, closures, builtins, coercion, string contexts, paths, and
//! recursive attribute sets are provided by Nix's own `libexpr` through its C API.
//!
//! The embedding application owns its integration policy: store selection, lookup paths,
//! custom primitive operations, filesystem permissions, and any additional capabilities.
//! See the repository README and `docs/architecture.md` for the boundary.

mod evaluator;
pub mod primop;

pub use evaluator::{Evaluator, EvaluatorBuilder};
pub use nix_bindings::{
    Context, Derivation, Error, EvalState, EvalStateBuilder, NixValueOps, Result, Store,
    StorePath, Value, ValueType, is_pure_eval, nix_version,
};

#[cfg(feature = "external")]
pub use nix_bindings::external;

#[cfg(feature = "flakes")]
pub use nix_bindings::flake;
