//! Native Nix primitive-operation integration.
//!
//! These are wrappers/re-exports of the high-level `nix-bindings` primop API. A callback
//! executes inside Nix's evaluator and can inspect lazy arguments or write its return value.
//! The argument/value wrappers are only valid during the callback.

pub use nix_bindings::primop::{
    ArgAttrs, ArgList, PrimOp, PrimOpArg, PrimOpRet, PrimOpValue,
};
pub use nix_bindings::NixValueOps;

use std::sync::Arc;

use nix_bindings::{Context, Result};

/// Register a global native Nix primitive operation.
///
/// Registration affects evaluator states created after this call. Use
/// `EvaluatorBuilder::register_primop` when constructing a new session.
pub fn register<F>(
    context: &Arc<Context>,
    name: &str,
    arity: u32,
    documentation: Option<&str>,
    callback: F,
) -> Result<()>
where
    F: Fn(&[PrimOpArg<'_>], &mut PrimOpRet<'_>) -> Result<()>
        + Send
        + Sync
        + 'static,
{
    PrimOp::new(context, name, arity, documentation, callback)?.register(context)
}
