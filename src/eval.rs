use std::{collections::{BTreeMap, HashSet}, cmp::Ordering, mem, rc::Rc};

use rnix::ast::{
    self, AstNode, Attr, BinOpKind, Entry, Expr, HasEntry, Inherit,
    InterpolPart, LiteralKind, Param, UnaryOpKind,
};

use crate::{
    parser::parse,
    value::{Function, FunctionInner, NativeCallback, Thunk, Value},
    Error, ErrorKind, Host, NixPath, NixPathKind, Result,
};

type EnvId = usize;

#[derive(Clone, Debug)]
struct Environment {
    bindings: BTreeMap<String, Thunk>,
    parent: Option<EnvId>,
    with_scope: Option<Thunk>,
}

type DeferredThunk = Box<dyn FnOnce(&mut EvalContext) -> Result<Value>>;

enum ThunkState {
    Suspended { expression: Expr, environment: EnvId, source_id: Option<String> },
    Deferred(DeferredThunk),
    Evaluating,
    Evaluated(Result<Value>),
}

struct ThunkCell {
    state: ThunkState,
}

enum AttrTree {
    Leaf(Thunk),
    Branch(BTreeMap<String, AttrTree>),
}

/// One isolated evaluation session. Thunks and lexical environments live in arenas and
/// are connected through typed handles, avoiding reference-counted cycles in recursive sets.
pub struct EvalContext {
    environments: Vec<Environment>,
    thunks: Vec<ThunkCell>,
    globals: EnvId,
    host: Option<Box<dyn Host>>,
}

impl Default for EvalContext {
    fn default() -> Self { Self::new() }
}

impl EvalContext {
    pub fn new() -> Self {
        Self {
            environments: vec![Environment {
                bindings: BTreeMap::new(),
                parent: None,
                with_scope: None,
            }],
            thunks: Vec::new(),
            globals: 0,
            host: None,
        }
    }

    pub fn with_host(host: impl Host + 'static) -> Self {
        let mut context = Self::new();
        context.host = Some(Box::new(host));
        context
    }

    pub fn set_host(&mut self, host: impl Host + 'static) { self.host = Some(Box::new(host)); }
    pub fn clear_host(&mut self) { self.host = None; }

    pub fn eval(&mut self, source: &str) -> Result<Value> { self.eval_named(source, None) }

    pub fn eval_named(&mut self, source: &str, source_id: Option<&str>) -> Result<Value> {
        let parsed = parse(source).map_err(|error| error.at_source(source_id))?;
        let expression = parsed.expression().ok_or_else(|| {
            Error::new(ErrorKind::Parse, "source has no expression").at_source(source_id)
        })?;
        self.eval_expression(expression, self.globals, source_id.map(str::to_owned))
    }

    /// Install the import binding using the configured host source resolver.
    /// Import is opt-in so embedders can control or entirely prohibit source loading.
    pub fn install_import(&mut self) {
        self.register_native("import", 1, |context, arguments| {
            let value = context.force(arguments[0])?;
            let path = match value {
                Value::Path(path) => path,
                Value::String(raw) => NixPath::new(raw, NixPathKind::String),
                other => return Err(Error::type_error("a path or string", other.type_name())),
            };
            context.import_path(&path)
        });
    }

    pub fn import_path(&mut self, path: &NixPath) -> Result<Value> {
        let importing_source = path.source_id.as_deref();
        let unit = self.host.as_mut()
            .ok_or_else(|| Error::new(ErrorKind::Host, "import requested but no Host is installed"))?
            .load_source(path, importing_source)
            .map_err(|error| Error::new(ErrorKind::Host, error.to_string()))?;
        self.eval_named(&unit.source, Some(&unit.source_id))
    }

    /// Register an already evaluated value in the global environment.
    pub fn register_global(&mut self, name: impl Into<String>, value: Value) {
        let thunk = self.ready(value);
        self.environments[self.globals].bindings.insert(name.into(), thunk);
    }

    /// Create a context-owned thunk from a host-created value. The returned handle must
    /// only be forced by this context.
    pub fn value_thunk(&mut self, value: Value) -> Thunk { self.ready(value) }

    /// Construct a lazy list whose elements are host-created values.
    pub fn make_list<I>(&mut self, values: I) -> Value
    where
        I: IntoIterator<Item = Value>,
    {
        Value::List(values.into_iter().map(|value| self.ready(value)).collect())
    }

    /// Construct an attribute set from host-created values.
    pub fn make_attrset<I, K>(&mut self, entries: I) -> Result<Value>
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        let mut attributes = BTreeMap::new();
        for (name, value) in entries {
            let name = name.into();
            if attributes.contains_key(&name) {
                return Err(Error::new(ErrorKind::InvalidArgument, format!("duplicate attribute '{name}'")));
            }
            let thunk = self.ready(value);
            attributes.insert(name, thunk);
        }
        Ok(Value::AttrSet(attributes))
    }

    /// Force a named field of an attribute set returned by the evaluator or a host callback.
    pub fn force_attr(&mut self, value: &Value, name: &str) -> Result<Option<Value>> {
        match value {
            Value::AttrSet(attributes) => attributes.get(name).copied().map(|thunk| self.force(thunk)).transpose(),
            other => Err(Error::type_error("an attribute set", other.type_name())),
        }
    }

    /// Force an item of a list returned by the evaluator or a host callback.
    pub fn force_list_item(&mut self, value: &Value, index: usize) -> Result<Option<Value>> {
        match value {
            Value::List(items) => items.get(index).copied().map(|thunk| self.force(thunk)).transpose(),
            other => Err(Error::type_error("a list", other.type_name())),
        }
    }

    /// Call a function value from Rust with an already evaluated argument.
    pub fn call(&mut self, function: Value, argument: Value) -> Result<Value> {
        let argument = self.ready(argument);
        self.apply(function, argument)
    }

    /// Register a curried native function. Arguments arrive as lazy thunks and callbacks
    /// explicitly choose which arguments to force.
    pub fn register_native<F>(&mut self, name: impl Into<String>, arity: usize, callback: F)
    where
        F: Fn(&mut EvalContext, &[Thunk]) -> Result<Value> + 'static,
    {
        assert!(arity > 0, "native functions must have at least one argument");
        let name = name.into();
        let function = Value::Function(Function(Rc::new(FunctionInner::Native {
            name: name.clone(),
            arity,
            supplied: Vec::new(),
            callback: Rc::new(callback) as NativeCallback,
        })));
        self.register_global(name, function);
    }

    /// Force to weak-head normal form. Both values and evaluation errors are memoized.
    pub fn force(&mut self, thunk: Thunk) -> Result<Value> {
        let index = thunk.index();
        if index >= self.thunks.len() {
            return Err(Error::new(ErrorKind::InvalidArgument, "thunk belongs to another EvalContext"));
        }

        let state = mem::replace(&mut self.thunks[index].state, ThunkState::Evaluating);
        match state {
            ThunkState::Evaluated(result) => {
                self.thunks[index].state = ThunkState::Evaluated(result.clone());
                result
            }
            ThunkState::Evaluating => {
                self.thunks[index].state = ThunkState::Evaluating;
                Err(Error::new(ErrorKind::InfiniteRecursion, "infinite recursion encountered"))
            }
            ThunkState::Suspended { expression, environment, source_id } => {
                let result = self.eval_expression(expression, environment, source_id);
                self.thunks[index].state = ThunkState::Evaluated(result.clone());
                result
            }
            ThunkState::Deferred(computation) => {
                let result = computation(self);
                self.thunks[index].state = ThunkState::Evaluated(result.clone());
                result
            }
        }
    }

    /// Apply a callable value to a delayed argument.
    pub fn apply(&mut self, function: Value, argument: Thunk) -> Result<Value> {
        match function {
            Value::Function(function) => match function.0.as_ref() {
                FunctionInner::Lambda { parameter, body, environment, source_id } => {
                    let child = self.new_environment(Some(*environment), None);
                    self.bind_parameter(parameter, argument, child, source_id.as_deref())?;
                    self.eval_expression(body.clone(), child, source_id.clone())
                }
                FunctionInner::Native { name, arity, supplied, callback } => {
                    let mut arguments = supplied.clone();
                    arguments.push(argument);
                    if arguments.len() < *arity {
                        return Ok(Value::Function(Function(Rc::new(FunctionInner::Native {
                            name: name.clone(),
                            arity: *arity,
                            supplied: arguments,
                            callback: callback.clone(),
                        }))));
                    }
                    callback(self, &arguments[..*arity])
                }
            },
            Value::AttrSet(attributes) => {
                let functor = attributes.get("__functor").copied().ok_or_else(|| {
                    Error::type_error("a function", "an attribute set without callable __functor")
                })?;
                let set = self.ready(Value::AttrSet(attributes));
                let functor = self.force(functor)?;
                let partially_applied = self.apply(functor, set)?;
                self.apply(partially_applied, argument)
            }
            other => Err(Error::type_error("a function", other.type_name())),
        }
    }

    fn new_environment(&mut self, parent: Option<EnvId>, with_scope: Option<Thunk>) -> EnvId {
        let id = self.environments.len();
        self.environments.push(Environment { bindings: BTreeMap::new(), parent, with_scope });
        id
    }

    fn ready(&mut self, value: Value) -> Thunk {
        let id = self.thunks.len();
        self.thunks.push(ThunkCell { state: ThunkState::Evaluated(Ok(value)) });
        Thunk(id)
    }

    fn suspend(&mut self, expression: Expr, environment: EnvId, source_id: Option<String>) -> Thunk {
        let id = self.thunks.len();
        self.thunks.push(ThunkCell {
            state: ThunkState::Suspended { expression, environment, source_id },
        });
        Thunk(id)
    }

    fn defer<F>(&mut self, computation: F) -> Thunk
    where
        F: FnOnce(&mut EvalContext) -> Result<Value> + 'static,
    {
        let id = self.thunks.len();
        self.thunks.push(ThunkCell {
            state: ThunkState::Deferred(Box::new(computation)),
        });
        Thunk(id)
    }

    fn lookup(&mut self, environment: EnvId, name: &str) -> Result<Thunk> {
        // Lexical variables win over all with-scopes, even outer lexical frames.
        let mut cursor = Some(environment);
        while let Some(id) = cursor {
            let frame = &self.environments[id];
            if let Some(value) = frame.bindings.get(name) { return Ok(*value); }
            cursor = frame.parent;
        }

        // Only after lexical lookup fails are with namespaces forced and inspected.
        cursor = Some(environment);
        while let Some(id) = cursor {
            let (scope, parent) = {
                let frame = &self.environments[id];
                (frame.with_scope, frame.parent)
            };
            if let Some(scope) = scope {
                match self.force(scope)? {
                    Value::AttrSet(attributes) => {
                        if let Some(value) = attributes.get(name) { return Ok(*value); }
                    }
                    other => return Err(Error::type_error("an attribute set in with", other.type_name())),
                }
            }
            cursor = parent;
        }
        Err(Error::new(ErrorKind::UnboundVariable, format!("undefined variable '{name}'")))
    }

    fn eval_expression(&mut self, expression: Expr, environment: EnvId, source_id: Option<String>) -> Result<Value> {
        use Expr::*;

        match expression {
            Ident(ident) => {
                let name = ident.syntax().text().to_string();
                match name.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" => Ok(Value::Null),
                    _ => {
                        let thunk = self.lookup(environment, &name)
                            .map_err(|error| error.at_source(source_id.as_deref()))?;
                        self.force(thunk).map_err(|error| error.at_source(source_id.as_deref()))
                    }
                }
            }
            Literal(literal) => self.eval_literal(literal.kind()),
            Str(string) => self.eval_string(&string, environment, source_id.as_deref()),
            PathAbs(path) => Ok(Value::Path(NixPath::new(path.syntax().text().to_string(), NixPathKind::Absolute).from_source(source_id.as_deref()))),
            PathRel(path) => Ok(Value::Path(NixPath::new(path.syntax().text().to_string(), NixPathKind::Relative).from_source(source_id.as_deref()))),
            PathHome(path) => Ok(Value::Path(NixPath::new(path.syntax().text().to_string(), NixPathKind::Home).from_source(source_id.as_deref()))),
            PathSearch(path) => Ok(Value::Path(NixPath::new(path.syntax().text().to_string(), NixPathKind::Search).from_source(source_id.as_deref()))),
            Apply(node) => {
                let function_expr = node.lambda().ok_or_else(|| Error::unsupported("application has no function expression"))?;
                let argument_expr = node.argument().ok_or_else(|| Error::unsupported("application has no argument expression"))?;
                let function = self.eval_expression(function_expr, environment, source_id.clone())?;
                let argument = self.suspend(argument_expr, environment, source_id);
                self.apply(function, argument)
            }
            Lambda(node) => {
                let parameter = node.param().ok_or_else(|| Error::unsupported("lambda has no parameter"))?;
                let body = node.body().ok_or_else(|| Error::unsupported("lambda has no body"))?;
                Ok(Value::Function(Function(Rc::new(FunctionInner::Lambda {
                    parameter, body, environment, source_id,
                }))))
            }
            AttrSet(set) => self.eval_attr_set(&set, environment, source_id),
            List(list) => {
                let values = list.syntax().children().filter_map(Expr::cast)
                    .map(|item| self.suspend(item, environment, source_id.clone()))
                    .collect();
                Ok(Value::List(values))
            }
            IfElse(node) => {
                let condition = node.condition().ok_or_else(|| Error::unsupported("if has no condition"))?;
                match self.eval_expression(condition, environment, source_id.clone())? {
                    Value::Bool(true) => self.eval_expression(node.body().ok_or_else(|| Error::unsupported("if has no then branch"))?, environment, source_id),
                    Value::Bool(false) => self.eval_expression(node.else_body().ok_or_else(|| Error::unsupported("if has no else branch"))?, environment, source_id),
                    other => Err(Error::type_error("a boolean condition", other.type_name())),
                }
            }
            Assert(node) => {
                let condition = node.condition().ok_or_else(|| Error::unsupported("assert has no condition"))?;
                match self.eval_expression(condition, environment, source_id.clone())? {
                    Value::Bool(true) => self.eval_expression(node.body().ok_or_else(|| Error::unsupported("assert has no body"))?, environment, source_id),
                    Value::Bool(false) => Err(Error::new(ErrorKind::Assertion, "assertion failed").at_source(source_id.as_deref())),
                    other => Err(Error::type_error("a boolean assertion condition", other.type_name())),
                }
            }
            BinOp(node) => self.eval_binary(&node, environment, source_id),
            UnaryOp(node) => {
                let operand = node.expr().ok_or_else(|| Error::unsupported("unary expression has no operand"))?;
                let value = self.eval_expression(operand, environment, source_id.clone())?;
                match (node.operator(), value) {
                    (Some(UnaryOpKind::Invert), Value::Bool(value)) => Ok(Value::Bool(!value)),
                    (Some(UnaryOpKind::Negate), Value::Integer(value)) => value.checked_neg().map(Value::Integer)
                        .ok_or_else(|| Error::new(ErrorKind::Arithmetic, "integer negation overflow")),
                    (Some(UnaryOpKind::Negate), Value::Float(value)) => Ok(Value::Float(-value)),
                    (Some(UnaryOpKind::Invert), other) => Err(Error::type_error("a boolean", other.type_name())),
                    (Some(UnaryOpKind::Negate), other) => Err(Error::type_error("a number", other.type_name())),
                    (None, _) => Err(Error::unsupported("unknown unary operator")),
                }
            }
            Select(node) => self.eval_select(&node, environment, source_id),
            HasAttr(node) => self.eval_has_attr(&node, environment, source_id),
            With(node) => {
                let namespace = node.namespace().ok_or_else(|| Error::unsupported("with has no namespace"))?;
                let body = node.body().ok_or_else(|| Error::unsupported("with has no body"))?;
                let scope = self.suspend(namespace, environment, source_id.clone());
                let child = self.new_environment(Some(environment), Some(scope));
                self.eval_expression(body, child, source_id)
            }
            LetIn(node) => self.eval_let_in(&node, environment, source_id),
            Paren(node) => {
                let inner = node.syntax().children().find_map(Expr::cast)
                    .ok_or_else(|| Error::unsupported("empty parenthesized expression"))?;
                self.eval_expression(inner, environment, source_id)
            }
            Root(node) => {
                let inner = node.expr().ok_or_else(|| Error::unsupported("empty root expression"))?;
                self.eval_expression(inner, environment, source_id)
            }
            CurPos(_) => Err(Error::unsupported("__curPos is parsed but not implemented")),
            LegacyLet(_) => Err(Error::unsupported("legacy let syntax is parsed but not implemented")),
            Error(_) => Err(Error::new(ErrorKind::Parse, "cannot evaluate an error node")),
        }
    }

    fn eval_literal(&self, kind: LiteralKind) -> Result<Value> {
        match kind {
            LiteralKind::Integer(node) => {
                let raw = node.syntax().text().to_string().replace('_', "");
                let value = raw.parse::<i64>().map_err(|_| Error::new(
                    ErrorKind::Arithmetic,
                    format!("integer literal is outside the supported i64 range: {raw}"),
                ))?;
                Ok(Value::Integer(value))
            }
            LiteralKind::Float(node) => {
                let raw = node.syntax().text().to_string().replace('_', "");
                let value = raw.parse::<f64>().map_err(|_| Error::new(ErrorKind::Parse, format!("invalid float literal: {raw}")))?;
                Ok(Value::Float(value))
            }
            LiteralKind::Uri(node) => Ok(Value::String(node.syntax().text().to_string())),
        }
    }

    fn eval_string(&mut self, string: &ast::Str, environment: EnvId, source_id: Option<&str>) -> Result<Value> {
        let mut output = String::new();
        for part in string.normalized_parts() {
            match part {
                InterpolPart::Literal(text) => output.push_str(&text),
                InterpolPart::Interpolation(interpolation) => {
                    let expression = interpolation.expr().ok_or_else(|| Error::unsupported("empty string interpolation"))?;
                    let value = self.eval_expression(expression, environment, source_id.map(str::to_owned))?;
                    output.push_str(&value.primitive_string()?);
                }
            }
        }
        Ok(Value::String(output))
    }

    fn eval_attr_name(&mut self, attr: Attr, environment: EnvId, source_id: Option<&str>) -> Result<Option<String>> {
        match attr {
            Attr::Ident(ident) => Ok(Some(ident.syntax().text().to_string())),
            Attr::Str(string) => match self.eval_string(&string, environment, source_id)? {
                Value::String(value) => Ok(Some(value)),
                _ => unreachable!(),
            },
            Attr::Dynamic(dynamic) => {
                let expression = dynamic.expr().ok_or_else(|| Error::unsupported("empty dynamic attribute name"))?;
                match self.eval_expression(expression, environment, source_id.map(str::to_owned))? {
                    Value::Null => Ok(None),
                    Value::String(value) => Ok(Some(value)),
                    other => Err(Error::type_error("a string attribute name", other.type_name())),
                }
            }
        }
    }

    fn eval_attr_path(&mut self, path: &ast::Attrpath, environment: EnvId, source_id: Option<&str>) -> Result<Option<Vec<String>>> {
        let mut names = Vec::new();
        for attr in path.attrs() {
            match self.eval_attr_name(attr, environment, source_id)? {
                Some(name) => names.push(name),
                None => return Ok(None),
            }
        }
        Ok(Some(names))
    }

    fn insert_attr_tree(tree: &mut BTreeMap<String, AttrTree>, path: &[String], value: Thunk) -> Result<()> {
        if path.is_empty() { return Err(Error::unsupported("empty attribute path")); }
        if path.len() == 1 {
            if tree.contains_key(&path[0]) {
                return Err(Error::new(ErrorKind::InvalidArgument, format!("attribute '{}' is defined more than once", path[0])));
            }
            tree.insert(path[0].clone(), AttrTree::Leaf(value));
            return Ok(());
        }
        let key = path[0].clone();
        let item = tree.entry(key.clone()).or_insert_with(|| AttrTree::Branch(BTreeMap::new()));
        match item {
            AttrTree::Branch(children) => Self::insert_attr_tree(children, &path[1..], value),
            AttrTree::Leaf(_) => Err(Error::new(ErrorKind::InvalidArgument, format!("attribute path conflicts at '{key}'"))),
        }
    }

    fn materialize_tree(&mut self, tree: BTreeMap<String, AttrTree>) -> BTreeMap<String, Thunk> {
        tree.into_iter().map(|(name, item)| {
            let thunk = match item {
                AttrTree::Leaf(value) => value,
                AttrTree::Branch(children) => {
                    let attrs = self.materialize_tree(children);
                    self.ready(Value::AttrSet(attrs))
                }
            };
            (name, thunk)
        }).collect()
    }

    fn add_inherit(
        &mut self,
        inherit: &Inherit,
        environment: EnvId,
        source_id: Option<&str>,
        target: &mut BTreeMap<String, AttrTree>,
    ) -> Result<Vec<(String, Thunk)>> {
        let from_expression = if let Some(from) = inherit.from() {
            Some(from.expr().ok_or_else(|| Error::unsupported("inherit-from has no expression"))?)
        } else {
            None
        };

        let mut created = Vec::new();
        for attr in inherit.attrs() {
            let Some(name) = self.eval_attr_name(attr, environment, source_id)? else { continue };
            let value = if let Some(expression) = &from_expression {
                // inherit (expr) field is equivalent to field = expr.field. Keep the
                // source expression lazy until this inherited field is actually forced.
                let expression = expression.clone();
                let field = name.clone();
                let deferred_source = source_id.map(str::to_owned);
                self.defer(move |context| {
                    let source = context.eval_expression(expression, environment, deferred_source)?;
                    let attributes = match source {
                        Value::AttrSet(attributes) => attributes,
                        other => return Err(Error::type_error("an attribute set after inherit-from", other.type_name())),
                    };
                    let field_thunk = attributes.get(&field).copied().ok_or_else(|| Error::new(
                        ErrorKind::MissingAttribute,
                        format!("inherit source has no attribute '{field}'"),
                    ))?;
                    context.force(field_thunk)
                })
            } else {
                self.lookup(environment, &name)?
            };

            Self::insert_attr_tree(target, &[name.clone()], value)?;
            created.push((name, value));
        }
        Ok(created)
    }

    fn eval_attr_set(&mut self, set: &ast::AttrSet, parent: EnvId, source_id: Option<String>) -> Result<Value> {
        let recursive = set.rec_token().is_some();
        let environment = if recursive { self.new_environment(Some(parent), None) } else { parent };
        let mut tree = BTreeMap::new();

        for entry in set.entries() {
            match entry {
                Entry::AttrpathValue(binding) => {
                    let path = binding.attrpath().ok_or_else(|| Error::unsupported("attribute binding has no path"))?;
                    let Some(names) = self.eval_attr_path(&path, environment, source_id.as_deref())? else { continue };
                    let expression = binding.value().ok_or_else(|| Error::unsupported("attribute binding has no value"))?;
                    let thunk = self.suspend(expression, environment, source_id.clone());
                    Self::insert_attr_tree(&mut tree, &names, thunk)?;
                }
                Entry::Inherit(inherit) => {
                    self.add_inherit(&inherit, environment, source_id.as_deref(), &mut tree)?;
                }
            }
        }

        let attributes = self.materialize_tree(tree);
        if recursive {
            // Install the set's top-level fields into the shared lexical frame after all
            // thunks are allocated. Each field closure already points at this frame.
            self.environments[environment].bindings.extend(attributes.clone());
        }
        Ok(Value::AttrSet(attributes))
    }

    fn eval_let_in(&mut self, node: &ast::LetIn, parent: EnvId, source_id: Option<String>) -> Result<Value> {
        let environment = self.new_environment(Some(parent), None);
        let mut bindings = BTreeMap::new();
        let mut inherited = Vec::new();

        // First install ordinary let bindings. Let environments are recursive, so every
        // name must exist before inherited expressions (which may refer to sibling names)
        // can be evaluated.
        for entry in node.entries() {
            match entry {
                Entry::AttrpathValue(binding) => {
                    let path = binding.attrpath().ok_or_else(|| Error::unsupported("let binding has no name"))?;
                    let Some(names) = self.eval_attr_path(&path, environment, source_id.as_deref())? else { continue };
                    if names.len() != 1 { return Err(Error::unsupported("nested let binding paths are not supported")); }
                    let name = names[0].clone();
                    let expression = binding.value().ok_or_else(|| Error::unsupported("let binding has no value"))?;
                    let thunk = self.suspend(expression, environment, source_id.clone());
                    Self::insert_attr_tree(&mut bindings, &names, thunk)?;
                    self.environments[environment].bindings.insert(name, thunk);
                }
                Entry::Inherit(inherit) => inherited.push(inherit),
            }
        }

        // Inherit bindings are also installed in the recursive frame. Their source-side
        // lookup remains deferred until the inherited value is demanded.
        for inherit in inherited {
            let created = self.add_inherit(&inherit, environment, source_id.as_deref(), &mut bindings)?;
            for (name, thunk) in created {
                self.environments[environment].bindings.insert(name, thunk);
            }
        }

        let body = node.body().ok_or_else(|| Error::unsupported("let expression has no body"))?;
        self.eval_expression(body, environment, source_id)
    }

    fn bind_parameter(&mut self, parameter: &Param, argument: Thunk, environment: EnvId, source_id: Option<&str>) -> Result<()> {
        match parameter {
            Param::IdentParam(param) => {
                let ident = param.ident().ok_or_else(|| Error::unsupported("lambda parameter has no identifier"))?;
                self.environments[environment].bindings.insert(ident.syntax().text().to_string(), argument);
                Ok(())
            }
            Param::Pattern(pattern) => {
                let attributes = match self.force(argument)? {
                    Value::AttrSet(attributes) => attributes,
                    other => return Err(Error::type_error("an attribute set argument", other.type_name())),
                };

                if let Some(bind) = pattern.pat_bind() {
                    let ident = bind.ident().ok_or_else(|| Error::unsupported("pattern alias has no identifier"))?;
                    self.environments[environment].bindings.insert(ident.syntax().text().to_string(), argument);
                }

                let mut consumed = BTreeMap::new();
                for entry in pattern.pat_entries() {
                    let ident = entry.ident().ok_or_else(|| Error::unsupported("pattern entry has no identifier"))?;
                    let name = ident.syntax().text().to_string();
                    if let Some(value) = attributes.get(&name) {
                        self.environments[environment].bindings.insert(name.clone(), *value);
                        consumed.insert(name, ());
                    } else if let Some(default) = entry.default() {
                        let thunk = self.suspend(default, environment, source_id.map(str::to_owned));
                        self.environments[environment].bindings.insert(name, thunk);
                    } else {
                        return Err(Error::new(ErrorKind::MissingAttribute, format!("missing required function argument attribute '{name}'")));
                    }
                }

                if pattern.ellipsis_token().is_none() {
                    if let Some(unexpected) = attributes.keys().find(|name| !consumed.contains_key(*name)) {
                        return Err(Error::new(ErrorKind::InvalidArgument, format!("unexpected function argument attribute '{unexpected}'")));
                    }
                }
                Ok(())
            }
        }
    }

    fn eval_select(&mut self, node: &ast::Select, environment: EnvId, source_id: Option<String>) -> Result<Value> {
        let target = node.expr().ok_or_else(|| Error::unsupported("selection has no base expression"))?;
        let mut value = self.eval_expression(target, environment, source_id.clone())?;
        let path = node.attrpath().ok_or_else(|| Error::unsupported("selection has no attribute path"))?;
        let names = self.eval_attr_path(&path, environment, source_id.as_deref())?.unwrap_or_default();

        for name in names {
            let attributes = match value {
                Value::AttrSet(attributes) => attributes,
                other => return Err(Error::type_error("an attribute set during selection", other.type_name())),
            };
            let Some(thunk) = attributes.get(&name).copied() else {
                if let Some(default) = node.default_expr() {
                    return self.eval_expression(default, environment, source_id);
                }
                return Err(Error::new(ErrorKind::MissingAttribute, format!("attribute '{name}' is missing")));
            };
            value = self.force(thunk)?;
        }
        Ok(value)
    }

    fn eval_has_attr(&mut self, node: &ast::HasAttr, environment: EnvId, source_id: Option<String>) -> Result<Value> {
        let target = node.expr().ok_or_else(|| Error::unsupported("has-attribute expression has no base"))?;
        let value = self.eval_expression(target, environment, source_id.clone())?;
        let mut attributes = match value {
            Value::AttrSet(attributes) => attributes,
            other => return Err(Error::type_error("an attribute set for the ? operator", other.type_name())),
        };
        let path = node.attrpath().ok_or_else(|| Error::unsupported("has-attribute expression has no path"))?;
        let Some(names) = self.eval_attr_path(&path, environment, source_id.as_deref())? else {
            return Ok(Value::Bool(false));
        };

        for (index, name) in names.iter().enumerate() {
            let Some(thunk) = attributes.get(name).copied() else { return Ok(Value::Bool(false)); };
            if index + 1 == names.len() { return Ok(Value::Bool(true)); }
            match self.force(thunk)? {
                Value::AttrSet(nested) => attributes = nested,
                _ => return Ok(Value::Bool(false)),
            }
        }
        Ok(Value::Bool(true))
    }

    fn eval_binary(&mut self, node: &ast::BinOp, environment: EnvId, source_id: Option<String>) -> Result<Value> {
        let lhs_expression = node.lhs().ok_or_else(|| Error::unsupported("binary expression has no left operand"))?;
        let rhs_expression = node.rhs().ok_or_else(|| Error::unsupported("binary expression has no right operand"))?;
        let operator = node.operator().ok_or_else(|| Error::unsupported("unknown binary operator"))?;
        let lhs_thunk = self.suspend(lhs_expression, environment, source_id.clone());

        if matches!(operator, BinOpKind::And | BinOpKind::Or | BinOpKind::Implication) {
            let lhs = match self.force(lhs_thunk)? {
                Value::Bool(value) => value,
                other => return Err(Error::type_error("a boolean left operand", other.type_name())),
            };
            let evaluate_rhs = match operator {
                BinOpKind::And | BinOpKind::Implication => lhs,
                BinOpKind::Or => !lhs,
                _ => unreachable!(),
            };
            if !evaluate_rhs {
                return Ok(Value::Bool(matches!(operator, BinOpKind::Or | BinOpKind::Implication)));
            }
            return match self.eval_expression(rhs_expression, environment, source_id)? {
                Value::Bool(value) => Ok(Value::Bool(value)),
                other => Err(Error::type_error("a boolean right operand", other.type_name())),
            };
        }

        let rhs_thunk = self.suspend(rhs_expression, environment, source_id);
        if matches!(operator, BinOpKind::PipeRight | BinOpKind::PipeLeft) {
            if operator == BinOpKind::PipeRight {
                let function = self.force(rhs_thunk)?;
                return self.apply(function, lhs_thunk);
            }
            let function = self.force(lhs_thunk)?;
            return self.apply(function, rhs_thunk);
        }

        let lhs = self.force(lhs_thunk)?;
        let rhs = self.force(rhs_thunk)?;
        match operator {
            BinOpKind::Update => match (lhs, rhs) {
                (Value::AttrSet(mut left), Value::AttrSet(right)) => { left.extend(right); Ok(Value::AttrSet(left)) }
                (left, _) => Err(Error::type_error("an attribute set on the left of //", left.type_name())),
            },
            BinOpKind::Concat => match (lhs, rhs) {
                (Value::List(mut left), Value::List(right)) => { left.extend(right); Ok(Value::List(left)) }
                (left, _) => Err(Error::type_error("a list on the left of ++", left.type_name())),
            },
            BinOpKind::Add => self.add(lhs, rhs),
            BinOpKind::Sub | BinOpKind::Mul | BinOpKind::Div => self.numeric(lhs, rhs, operator),
            BinOpKind::Equal => Ok(Value::Bool(self.primitive_equal(&lhs, &rhs)?)),
            BinOpKind::NotEqual => Ok(Value::Bool(!self.primitive_equal(&lhs, &rhs)?)),
            BinOpKind::Less | BinOpKind::LessOrEq | BinOpKind::More | BinOpKind::MoreOrEq => self.compare(lhs, rhs, operator),
            BinOpKind::And | BinOpKind::Or | BinOpKind::Implication | BinOpKind::PipeRight | BinOpKind::PipeLeft => unreachable!(),
        }
    }

    fn add(&self, lhs: Value, rhs: Value) -> Result<Value> {
        match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => a.checked_add(b).map(Value::Integer)
                .ok_or_else(|| Error::new(ErrorKind::Arithmetic, "integer addition overflow")),
            (Value::String(mut a), Value::String(b)) => { a.push_str(&b); Ok(Value::String(a)) }
            (Value::Path(mut a), Value::String(b)) => { a.raw.push_str(&b); Ok(Value::Path(a)) }
            (Value::Path(mut a), Value::Path(b)) => { a.raw.push_str(&b.raw); Ok(Value::Path(a)) }
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a + b as f64)),
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (a, _) => Err(Error::type_error("compatible values for +", a.type_name())),
        }
    }

    fn numeric(&self, lhs: Value, rhs: Value, operator: BinOpKind) -> Result<Value> {
        match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => match operator {
                BinOpKind::Sub => a.checked_sub(b).map(Value::Integer)
                    .ok_or_else(|| Error::new(ErrorKind::Arithmetic, "integer subtraction overflow")),
                BinOpKind::Mul => a.checked_mul(b).map(Value::Integer)
                    .ok_or_else(|| Error::new(ErrorKind::Arithmetic, "integer multiplication overflow")),
                BinOpKind::Div if b == 0 => Err(Error::new(ErrorKind::Arithmetic, "division by zero")),
                BinOpKind::Div => a.checked_div(b).map(Value::Integer)
                    .ok_or_else(|| Error::new(ErrorKind::Arithmetic, "integer division overflow")),
                _ => unreachable!(),
            },
            (a, b) if a.as_float().is_some() && b.as_float().is_some() => {
                let a = a.as_float().unwrap();
                let b = b.as_float().unwrap();
                match operator {
                    BinOpKind::Sub => Ok(Value::Float(a - b)),
                    BinOpKind::Mul => Ok(Value::Float(a * b)),
                    BinOpKind::Div if b == 0.0 => Err(Error::new(ErrorKind::Arithmetic, "division by zero")),
                    BinOpKind::Div => Ok(Value::Float(a / b)),
                    _ => unreachable!(),
                }
            }
            (a, _) => Err(Error::type_error("numeric operands", a.type_name())),
        }
    }

    fn primitive_equal(&mut self, lhs: &Value, rhs: &Value) -> Result<bool> {
        self.values_equal(lhs, rhs, &mut HashSet::new())
    }

    fn values_equal(
        &mut self,
        lhs: &Value,
        rhs: &Value,
        visited: &mut HashSet<(usize, usize)>,
    ) -> Result<bool> {
        match (lhs, rhs) {
            (Value::Null, Value::Null) => Ok(true),
            (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
            (Value::Integer(a), Value::Integer(b)) => Ok(a == b),
            (Value::Float(a), Value::Float(b)) => Ok(a == b),
            (Value::Integer(a), Value::Float(b)) => Ok(*a as f64 == *b),
            (Value::Float(a), Value::Integer(b)) => Ok(*a == *b as f64),
            (Value::String(a), Value::String(b)) => Ok(a == b),
            (Value::Path(a), Value::Path(b)) => Ok(a == b),
            (Value::List(a), Value::List(b)) => {
                if a.len() != b.len() { return Ok(false); }
                for (left, right) in a.iter().zip(b) {
                    if !self.thunks_equal(*left, *right, visited)? { return Ok(false); }
                }
                Ok(true)
            }
            (Value::AttrSet(a), Value::AttrSet(b)) => {
                if a.len() != b.len() { return Ok(false); }
                for ((left_name, left_value), (right_name, right_value)) in a.iter().zip(b) {
                    if left_name != right_name
                        || !self.thunks_equal(*left_value, *right_value, visited)?
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Function(a), Value::Function(b)) => Ok(Rc::ptr_eq(&a.0, &b.0)),
            _ => Ok(false),
        }
    }

    fn thunks_equal(
        &mut self,
        left: Thunk,
        right: Thunk,
        visited: &mut HashSet<(usize, usize)>,
    ) -> Result<bool> {
        if left == right { return Ok(true); }

        let pair = if left.index() <= right.index() {
            (left.index(), right.index())
        } else {
            (right.index(), left.index())
        };
        if !visited.insert(pair) {
            return Err(Error::new(
                ErrorKind::InfiniteRecursion,
                "cyclic values encountered while comparing distinct thunks",
            ));
        }

        let left_value = match self.force(left) {
            Ok(value) => value,
            Err(error) => {
                visited.remove(&pair);
                return Err(error);
            }
        };
        let right_value = match self.force(right) {
            Ok(value) => value,
            Err(error) => {
                visited.remove(&pair);
                return Err(error);
            }
        };
        let result = self.values_equal(&left_value, &right_value, visited);
        // Track the active recursion path, not all previously compared pairs: the same
        // acyclic shared pair may occur in more than one field.
        visited.remove(&pair);
        result
    }

    fn compare(&mut self, lhs: Value, rhs: Value, operator: BinOpKind) -> Result<Value> {
        let ordering = self.values_ordering(&lhs, &rhs)?;
        let result = match operator {
            BinOpKind::Less => ordering.is_lt(),
            BinOpKind::LessOrEq => !ordering.is_gt(),
            BinOpKind::More => ordering.is_gt(),
            BinOpKind::MoreOrEq => !ordering.is_lt(),
            _ => unreachable!(),
        };
        Ok(Value::Bool(result))
    }

    fn values_ordering(&mut self, lhs: &Value, rhs: &Value) -> Result<Ordering> {
        match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => Ok(a.cmp(b)),
            (a, b) if a.as_float().is_some() && b.as_float().is_some() => {
                a.as_float().unwrap().partial_cmp(&b.as_float().unwrap())
                    .ok_or_else(|| Error::new(ErrorKind::Arithmetic, "cannot order NaN"))
            }
            (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
            (Value::Path(a), Value::Path(b)) => Ok(a.raw.cmp(&b.raw)),
            (Value::List(a), Value::List(b)) => {
                for (left, right) in a.iter().zip(b) {
                    if self.thunks_equal(*left, *right, &mut HashSet::new())? { continue; }
                    let left_value = self.force(*left)?;
                    let right_value = self.force(*right)?;
                    return self.values_ordering(&left_value, &right_value);
                }
                Ok(a.len().cmp(&b.len()))
            }
            (Value::Integer(_) | Value::Float(_), Value::Integer(_) | Value::Float(_)) => {
                unreachable!("all numeric pairs are covered by the mixed numeric arm")
            }
            (a, b) if a.type_name() == b.type_name() => {
                Err(Error::type_error("values ordered as numbers, strings, paths, or lists", a.type_name()))
            }
            (a, _) => Err(Error::type_error("values of compatible comparable types", a.type_name())),
        }
    }
}

/// Convenience owner for applications that prefer an evaluator object over EvalContext.
pub struct Evaluator {
    context: EvalContext,
}

impl Default for Evaluator {
    fn default() -> Self { Self::new() }
}

impl Evaluator {
    pub fn new() -> Self { Self { context: EvalContext::new() } }
    pub fn with_host(host: impl Host + 'static) -> Self { Self { context: EvalContext::with_host(host) } }
    pub fn context(&self) -> &EvalContext { &self.context }
    pub fn context_mut(&mut self) -> &mut EvalContext { &mut self.context }
    pub fn eval(&mut self, source: &str) -> Result<Value> { self.context.eval(source) }
}
