//! JS IR — a small but real functional expression language embedded in R2N.
//!
//! This is the "JS IR" layer. R2N source expressions lower into `JsExpr`.
//! Notably, JS IR has no `let` binding at the node level; instead we perform a
//! minimal ANF-ish transformation: each `let` in a component body becomes a
//! captured constant in the component closure, and expressions are kept
//! tree-structured (the runtime evaluates them directly). This is sufficient
//! for the supported subset and keeps evaluation deterministic.

use crate::value::Literal;
use serde::{Deserialize, Serialize};

/// A JS IR expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JsExpr {
    /// Compile-time literal.
    Lit(Literal),
    /// Reference to a captured name (component param or `let`/`const` binding).
    Var(String),
    /// Member access on a value: `base.prop`. For host values this reads a
    /// prop/field; for arrays/maps it indexes by key.
    Get { base: Box<JsExpr>, prop: String },
    /// Index into an array/string/map by an integer/string key.
    Index { base: Box<JsExpr>, key: Box<JsExpr> },
    /// Binary operation.
    Bin {
        op: JsBinOp,
        left: Box<JsExpr>,
        right: Box<JsExpr>,
    },
    /// Unary operation.
    Un { op: JsUnOp, expr: Box<JsExpr> },
    /// Call a function value with arguments.
    Call {
        callee: Box<JsExpr>,
        args: Vec<JsExpr>,
    },
    /// A closure over `captures` with `params` and `body`. Used for arrow fns
    /// and for component render functions.
    Closure {
        params: Vec<String>,
        captures: Vec<String>,
        body: Box<JsExpr>,
    },
    /// An array literal.
    Array(Vec<JsExpr>),
    /// A block of expressions evaluated in order; the block's value is its
    /// last expression (or null when empty). From block-bodied arrows.
    Block(Vec<JsExpr>),
    /// `cond ? then : else`.
    If {
        cond: Box<JsExpr>,
        then: Box<JsExpr>,
        else_: Box<JsExpr>,
    },
    /// Reference to a builtin by name (e.g. `"useState"`, `"items.map"` is a
    /// member call, not a builtin). Builtins are resolved by the runtime.
    Builtin(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JsBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JsUnOp {
    Neg,
    Not,
}

impl JsBinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            JsBinOp::Add => "+",
            JsBinOp::Sub => "-",
            JsBinOp::Mul => "*",
            JsBinOp::Div => "/",
            JsBinOp::Mod => "%",
            JsBinOp::Eq => "==",
            JsBinOp::Neq => "!=",
            JsBinOp::Lt => "<",
            JsBinOp::Gt => ">",
            JsBinOp::Le => "<=",
            JsBinOp::Ge => ">=",
            JsBinOp::And => "&&",
            JsBinOp::Or => "||",
        }
    }
}
