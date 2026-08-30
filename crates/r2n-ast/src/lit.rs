//! Literal values that appear in source.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A literal value written directly in source code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    /// Integer literal (arbitrary precision would be overkill; i64 covers the
    /// supported programs; overflow is an intentional, documented limit).
    Int(i64),
    /// Floating point literal.
    Float(f64),
    /// UTF-16-capable string literal (R2N uses UTF-16 strings per ADR-009,
    /// but stores as Rust `String` internally; length is measured in code
    /// units when needed).
    String(String),
    /// Boolean literal.
    Bool(bool),
    /// The `null` literal.
    Null,
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Int(i) => write!(f, "{i}"),
            Literal::Float(x) => write!(f, "{x}"),
            Literal::String(s) => write!(f, "{:?}", s),
            Literal::Bool(b) => write!(f, "{b}"),
            Literal::Null => write!(f, "null"),
        }
    }
}
