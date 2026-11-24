use std::{error::Error as StdError, fmt};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Parse,
    Unsupported,
    UnboundVariable,
    Type,
    InfiniteRecursion,
    Assertion,
    MissingAttribute,
    Arithmetic,
    Host,
    InvalidArgument,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub source_id: Option<String>,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), source_id: None }
    }

    pub fn at_source(mut self, source_id: Option<&str>) -> Self {
        self.source_id = source_id.map(str::to_owned);
        self
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    pub(crate) fn type_error(expected: &str, found: &str) -> Self {
        Self::new(ErrorKind::Type, format!("expected {expected}, found {found}"))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source_id {
            write!(f, "{source}: ")?;
        }
        write!(f, "{}", self.message)
    }
}

impl StdError for Error {}
