use std::{collections::BTreeMap, fmt, rc::Rc};

use rnix::ast::{Expr, Param};

use crate::{EvalContext, Error, Result};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Thunk(pub(crate) usize);

impl Thunk {
    pub(crate) fn index(self) -> usize { self.0 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NixPathKind { Absolute, Relative, Home, Search, String }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NixPath {
    pub raw: String,
    pub kind: NixPathKind,
    pub source_id: Option<String>,
}

impl NixPath {
    pub fn new(raw: impl Into<String>, kind: NixPathKind) -> Self {
        Self { raw: raw.into(), kind, source_id: None }
    }

    pub fn from_source(mut self, source_id: Option<&str>) -> Self {
        self.source_id = source_id.map(str::to_owned);
        self
    }
}

pub type NativeCallback = Rc<dyn Fn(&mut EvalContext, &[Thunk]) -> Result<Value>>;

#[derive(Clone)]
pub struct Function(pub(crate) Rc<FunctionInner>);

pub(crate) enum FunctionInner {
    Lambda {
        parameter: Param,
        body: Expr,
        environment: usize,
        source_id: Option<String>,
    },
    Native {
        name: String,
        arity: usize,
        supplied: Vec<Thunk>,
        callback: NativeCallback,
    },
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_ref() {
            FunctionInner::Lambda { .. } => f.write_str("Function(<lambda>)"),
            FunctionInner::Native { name, arity, supplied, .. } => f
                .debug_struct("NativeFunction")
                .field("name", name)
                .field("arity", arity)
                .field("supplied", &supplied.len())
                .finish(),
        }
    }
}

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Path(NixPath),
    List(Vec<Thunk>),
    AttrSet(BTreeMap<String, Thunk>),
    Function(Function),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "a boolean",
            Self::Integer(_) => "an integer",
            Self::Float(_) => "a float",
            Self::String(_) => "a string",
            Self::Path(_) => "a path",
            Self::List(_) => "a list",
            Self::AttrSet(_) => "an attribute set",
            Self::Function(_) => "a function",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self { Self::Bool(value) => Some(*value), _ => None }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self { Self::Integer(value) => Some(*value), _ => None }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Integer(value) => Some(*value as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self { Self::String(value) => Some(value), _ => None }
    }

    pub fn attr(&self, name: &str) -> Option<Thunk> {
        match self { Self::AttrSet(attrs) => attrs.get(name).copied(), _ => None }
    }

    pub(crate) fn primitive_string(&self) -> Result<String> {
        match self {
            Self::Null => Ok(String::new()),
            Self::Bool(value) => Ok(value.to_string()),
            Self::Integer(value) => Ok(value.to_string()),
            Self::Float(value) => Ok(value.to_string()),
            Self::String(value) => Ok(value.clone()),
            Self::Path(path) => Ok(path.raw.clone()),
            other => Err(Error::type_error("a string-coercible primitive", other.type_name())),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("Null"),
            Self::Bool(value) => f.debug_tuple("Bool").field(value).finish(),
            Self::Integer(value) => f.debug_tuple("Integer").field(value).finish(),
            Self::Float(value) => f.debug_tuple("Float").field(value).finish(),
            Self::String(value) => f.debug_tuple("String").field(value).finish(),
            Self::Path(value) => f.debug_tuple("Path").field(value).finish(),
            Self::List(values) => f.debug_tuple("List").field(values).finish(),
            Self::AttrSet(values) => f.debug_tuple("AttrSet").field(values).finish(),
            Self::Function(value) => f.debug_tuple("Function").field(value).finish(),
        }
    }
}
