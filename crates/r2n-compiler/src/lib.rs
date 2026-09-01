//! R2N compiler orchestration.
//!
//! The build pipeline (ADR pipeline `parser -> ast -> js-ir -> react-ir ->
//! runtime-ir`): parse source into an AST, then lower it into a language-neutral
//! runtime artifact. The output is *pure data* (`RuntimeTemplate`), so it can be
//! serialized (e.g. JSON) and handed to any conformant runtime — the literal
//! "language-independent artifact" the architecture promises.

mod link;

pub use link::{
    link_source, link_source_dev, FsResolver, LinkError, MemResolver, ModuleResolver,
};
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

/// Compile in DEV mode: keeps StrictMode nodes and marks the artifact so
/// the runtime double-invokes effects inside StrictMode subtrees.
pub fn compile_source_dev(src: &str) -> Result<RuntimeTemplate, CompileError> {
    let program = r2n_parser::parse(src)?;
    let template = r2n_ir::lower_dev(&program)?;
    Ok(template)
}

/// Collect every diagnostic in the source in one pass (parse with recovery),
/// then — if it parses — continue into lowering for its diagnostics too.
/// Returns the rendered diagnostics, one String per error, ready to print.
/// An Ok with a non-empty Vec means the source does not compile; the Vec is
/// the complete list of reasons why.
pub fn collect_diagnostics(src: &str) -> Result<Vec<String>, CompileError> {
    let recovered = r2n_parser::parse_with_recovery(src)?;
    let mut rendered = Vec::new();
    for err in &recovered.errors {
        rendered.push(format!("error: {}\n{}", err.message, err.render(src)));
    }
    if rendered.is_empty() {
        // The parse is clean; surface lowering errors too (single error —
        // lowering does not recover; that is M1+ work).
        if let Err(e) = lower(&recovered.program) {
            rendered.push(format!("lower error: {e}"));
        }
    }
    Ok(rendered)
}
