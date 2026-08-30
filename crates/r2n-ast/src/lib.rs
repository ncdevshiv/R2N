//! Source-level AST for the R2N language subset.
//!
//! This is the output of the parser and the input to the IR lowering pass.
//! It deliberately models only the subset R2N actually supports (see the
//! architecture design), so an invalid program is a parse error, not a
//! runtime surprise.

pub mod expr;
pub mod lit;
pub mod op;
pub mod program;

pub use expr::*;
pub use lit::*;
pub use op::*;
pub use program::*;
