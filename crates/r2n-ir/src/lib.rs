//! R2N interlinked IR.
//!
//! Modules:
//! * `value`  — compile-time literal type
//! * `js`      — JS IR (the embedded functional expression language)
//! * `react`   — React IR (the node tree components render)
//! * `runtime` — Runtime IR (the language-neutral artifact: templates)
//! * `lower`   — AST -> RuntimeTemplate lowering (the build pipeline)
//! * `ser`     — serialization of the artifact (the language-neutral output)

pub mod js;
pub mod lower;
pub mod react;
pub mod runtime;
pub mod ser;
pub mod value;

pub use js::JsExpr;
pub use lower::{lower, LowerError};
pub use react::{ComponentRef, ReactNode};
pub use runtime::{RuntimeComponent, RuntimeTemplate, TemplateNodeId};
pub use value::Literal;
