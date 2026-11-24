use std::fmt;

use crate::NixPath;

#[derive(Clone, Debug)]
pub struct SourceUnit {
    pub source_id: String,
    pub source: String,
}

impl SourceUnit {
    pub fn new(source_id: impl Into<String>, source: impl Into<String>) -> Self {
        Self { source_id: source_id.into(), source: source.into() }
    }
}

#[derive(Clone, Debug)]
pub struct HostError {
    pub message: String,
}

impl HostError {
    pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}

impl std::error::Error for HostError {}

/// Host boundary for imports and source acquisition. The core evaluator never performs
/// implicit filesystem or network access.
pub trait Host {
    fn load_source(
        &mut self,
        path: &NixPath,
        importing_source: Option<&str>,
    ) -> std::result::Result<SourceUnit, HostError>;
}

pub type SourceLoader =
    Box<dyn FnMut(&NixPath, Option<&str>) -> std::result::Result<SourceUnit, HostError>>;

pub struct LoaderHost {
    loader: SourceLoader,
}

impl LoaderHost {
    pub fn new(loader: SourceLoader) -> Self { Self { loader } }
}

impl fmt::Debug for LoaderHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("LoaderHost(..)") }
}

impl Host for LoaderHost {
    fn load_source(
        &mut self,
        path: &NixPath,
        importing_source: Option<&str>,
    ) -> std::result::Result<SourceUnit, HostError> {
        (self.loader)(path, importing_source)
    }
}
