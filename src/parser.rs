use rnix::ast::{AstNode, Expr, Root};

use crate::{Error, ErrorKind, Result};

#[derive(Clone, Debug)]
pub struct ParsedExpression {
    root: Root,
    source: String,
}

impl ParsedExpression {
    pub fn expression(&self) -> Option<Expr> { self.root.expr() }
    pub fn source(&self) -> &str { &self.source }
    pub fn root(&self) -> &Root { &self.root }
}

pub fn parse(source: &str) -> Result<ParsedExpression> {
    let parsed = Root::parse(source);
    if !parsed.errors().is_empty() {
        return Err(Error::new(
            ErrorKind::Parse,
            format!("invalid Nix expression: {:?}", parsed.errors()),
        ));
    }
    let root = parsed.tree();
    if root.expr().is_none() {
        return Err(Error::new(ErrorKind::Parse, "source does not contain an expression"));
    }
    Ok(ParsedExpression { root, source: source.to_owned() })
}
