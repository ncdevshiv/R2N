//! Compile-time IR value type (only literals; runtime values are separate).
//!
//! Keeping runtime values out of the IR is deliberate: the IR is pure,
//! serializable data (a language-neutral artifact), while runtime values may
//! carry host closures that cannot be serialized.

pub use r2n_ast::lit::Literal;
