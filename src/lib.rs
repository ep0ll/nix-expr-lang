//! An embeddable interpreter for the Nix expression language.
//!
//! The crate separates language evaluation from Nix's package manager, store, fetchers,
//! filesystem policy, and command-line interface. Applications install native functions and
//! a Host implementation for the effects they want to expose.
//!
//! This is an early interpreter milestone, not yet a drop-in replacement for Nix's
//! production evaluator. See DESIGN.md for the implemented surface and remaining work.

mod eval;
mod error;
mod host;
mod parser;
mod value;

pub use eval::{EvalContext, Evaluator};
pub use error::{Error, ErrorKind, Result};
pub use host::{Host, HostError, LoaderHost, SourceLoader, SourceUnit};
pub use parser::{ParsedExpression, parse};
pub use value::{Function, NativeCallback, NixPath, NixPathKind, Thunk, Value};
