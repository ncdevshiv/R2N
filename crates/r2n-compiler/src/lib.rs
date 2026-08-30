//! R2N compiler orchestration.
//!
//! The build pipeline (ADR pipeline `parser -> ast -> js-ir -> react-ir ->
//! runtime-ir`): parse source into an AST, then lower it into a language-neutral
//! runtime artifact. The output is *pure data* (`RuntimeTemplate`), so it can be
//! serialized (e.g. JSON) and handed to any conformant runtime — the literal
//! "language-independent artifact" the architecture promises.

use r2n_ir::runtime::RuntimeTemplate;
use r2n_ir::{lower, LowerError};
use r2n_parser::ParseError;

/// A compile error, carrying the stage it failed at.
#[derive(Debug, Clone)]
pub enum CompileError {
    Parse(String),
    Lower(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Parse(m) => write!(f, "parse error: {m}"),
            CompileError::Lower(m) => write!(f, "lower error: {m}"),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<ParseError> for CompileError {
    fn from(e: ParseError) -> Self {
        CompileError::Parse(e.to_string())
    }
}

impl From<LowerError> for CompileError {
    fn from(e: LowerError) -> Self {
        CompileError::Lower(e.to_string())
    }
}

/// Compile R2N source into a runtime template (the artifact).
pub fn compile_source(src: &str) -> Result<RuntimeTemplate, CompileError> {
    let program = r2n_parser::parse(src)?;
    let template = lower(&program)?;
    Ok(template)
}
