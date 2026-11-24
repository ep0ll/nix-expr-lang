use std::{
    path::Path,
    sync::Arc,
};

use nix_bindings::{
    Context, Error, EvalState, EvalStateBuilder, Result, Store, Value,
};

use crate::primop::{PrimOp, PrimOpArg, PrimOpRet};

/// Configures one Nix evaluation session.
///
/// A builder owns the Nix C API context and store used to create the evaluator. Register
/// custom primops before calling `build`: Nix's C API registers primops process-wide, and
/// newly-created evaluator states receive the registered primops.
pub struct EvaluatorBuilder {
    context: Arc<Context>,
    store: Arc<Store>,
    lookup_path: Vec<String>,
    load_config: bool,
}

impl EvaluatorBuilder {
    /// Create a builder using Nix's default local store.
    pub fn new() -> Result<Self> {
        Self::with_store_uri(None)
    }

    /// Create a builder using an explicitly selected Nix store URI.
    ///
    /// Examples include `local`, `daemon`, or a configured remote store URI accepted by
    /// the installed Nix version.
    pub fn with_store_uri(uri: impl AsRef<str>) -> Result<Self> {
        Self::with_optional_store_uri(Some(uri.as_ref()))
    }

    /// Create a builder using the default store when `uri` is `None`.
    pub fn with_optional_store_uri(uri: Option<&str>) -> Result<Self> {
        let context = Arc::new(Context::new()?);
        let store = Arc::new(Store::open(&context, uri)?);

        Ok(Self {
            context,
            store,
            lookup_path: Vec::new(),
            load_config: true,
        })
    }

    /// Reuse an application-owned Nix context and store.
    ///
    /// This is useful when the same process already manages Nix store operations through
    /// `nix-bindings`. The supplied store must have been created from the supplied context.
    pub fn from_parts(context: Arc<Context>, store: Arc<Store>) -> Self {
        Self {
            context,
            store,
            lookup_path: Vec::new(),
            load_config: true,
        }
    }

    /// Set the lookup path used by Nix angle-bracket paths such as `<nixpkgs>`.
    ///
    /// Entries use Nix's standard `name=path` or `path` syntax. If no lookup path is
    /// provided, Nix's normal configuration and environment handling apply.
    pub fn lookup_path<I, S>(&mut self, entries: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.lookup_path = entries.into_iter().map(Into::into).collect();
        self
    }

    /// Do not load Nix configuration files or environment-derived evaluator settings.
    ///
    /// This is useful for controlled embedding and reproducible tests. It does not by
    /// itself enable pure evaluation or sandbox host callbacks.
    pub fn no_load_config(&mut self) -> &mut Self {
        self.load_config = false;
        self
    }

    /// Access the Nix context used by this builder.
    pub fn context(&self) -> &Arc<Context> {
        &self.context
    }

    /// Access the Nix store used by this builder.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Register a custom Nix primitive operation.
    ///
    /// This uses Nix's native primop API; the callback is invoked by the real Nix evaluator.
    /// Registration is process-wide and must happen before building the evaluator state that
    /// should see the primop. The callback receives values scoped to the native callback
    /// frame; do not retain them after the callback returns.
    pub fn register_primop<F>(
        &mut self,
        name: &str,
        arity: u32,
        documentation: Option<&str>,
        callback: F,
    ) -> Result<&mut Self>
    where
        F: Fn(&[PrimOpArg<'_>], &mut PrimOpRet<'_>) -> Result<()>
            + Send
            + Sync
            + 'static,
    {
        PrimOp::new(&self.context, name, arity, documentation, callback)?
            .register(&self.context)?;
        Ok(self)
    }

    /// Build a session backed by Nix's `EvalState`.
    pub fn build(self) -> Result<Evaluator> {
        let mut builder = EvalStateBuilder::new(&self.store)?;
        if !self.lookup_path.is_empty() {
            builder = builder.set_lookup_path(&self.lookup_path)?;
        }
        if !self.load_config {
            builder = builder.no_load_config();
        }

        let state = builder.build()?;
        Ok(Evaluator {
            // EvalState itself keeps the context and store alive. Retaining the Arcs here
            // makes ownership and reuse of the embedding resources explicit at this API.
            _context: self.context,
            _store: self.store,
            state,
        })
    }
}

/// An evaluation session backed by Nix's own `libexpr`.
///
/// The returned `Value` handles borrow the evaluator state. Keep the evaluator alive while
/// using any values returned by its methods.
pub struct Evaluator {
    _context: Arc<Context>,
    _store: Arc<Store>,
    state: EvalState,
}

impl Evaluator {
    /// Construct an evaluator with the default Nix store and configuration.
    pub fn new() -> Result<Self> {
        EvaluatorBuilder::new()?.build()
    }

    /// Construct an evaluator using the given store URI.
    pub fn with_store_uri(uri: impl AsRef<str>) -> Result<Self> {
        EvaluatorBuilder::with_store_uri(uri)?.build()
    }

    /// Parse and evaluate a Nix expression.
    ///
    /// The source label `<eval>` is used for diagnostics. Use `eval_at` when relative
    /// paths in the expression need an explicit base directory.
    pub fn eval(&self, source: &str) -> Result<Value<'_>> {
        self.state.eval_from_string(source, "<eval>")
    }

    /// Parse and evaluate a Nix expression with a source/base path.
    ///
    /// The path is passed to Nix's C API and used for diagnostics and relative path handling.
    pub fn eval_at(&self, source: &str, base_path: impl AsRef<Path>) -> Result<Value<'_>> {
        let path = base_path.as_ref().to_string_lossy();
        self.state.eval_from_string(source, &path)
    }

    /// Read and evaluate a Nix expression file using Nix's native file evaluator.
    ///
    /// Relative imports and paths are resolved by Nix according to the file's location and
    /// the configured evaluator environment.
    pub fn eval_file(&self, path: impl AsRef<Path>) -> Result<Value<'_>> {
        self.state.eval_from_file(path)
    }

    /// Access the underlying native Nix evaluation state.
    ///
    /// This exposes the rest of the upstream wrapper API, including value construction,
    /// function application, store-path operations, and lazy attribute/list inspection.
    pub fn state(&self) -> &EvalState {
        &self.state
    }

    /// Access the Nix store associated with this evaluation session.
    pub fn store(&self) -> &Arc<Store> {
        &self._store
    }

    /// Access the Nix C API context associated with this evaluation session.
    pub fn context(&self) -> &Arc<Context> {
        &self._context
    }
}
