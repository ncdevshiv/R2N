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
    /// `new Constructor(args...)` — allocate an ES class instance.
    New {
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
    /// Assignment: `target = value` — target is a variable or a member
    /// access (`ref.current`). Member writes go through the frame (refs);
    /// variable writes update the current env scope.
    Assign {
        target: Box<JsExpr>,
        value: Box<JsExpr>,
    },
    /// An async function's state machine (M2-T07): the body is split at each
    /// `await` into segments. Segment 0 runs synchronously on call; a
    /// terminal await suspends until its promise settles, then the next
    /// segment resumes with the resolved value bound to `await_bind`.
    AsyncFn {
        params: Vec<String>,
        segments: Vec<JsAsyncSegment>,
    },
    /// `throw value` — raises `value` to the nearest enclosing `Try` (M2-T06).
    Throw { value: Box<JsExpr> },
    /// `try { block } catch (param) { catch } finally { finally }`. The catch
    /// scope binds `param` to the thrown value; at least one of catch/finally
    /// is present (parser enforces). Finally runs on every path; an error
    /// raised IN the finally replaces any pending outcome.
    Try {
        block: Vec<JsExpr>,
        catch_param: Option<String>,
        catch: Option<Vec<JsExpr>>,
        finally: Option<Vec<JsExpr>>,
    },
    /// Reference to a builtin by name (e.g. `"useState"`, `"items.map"` is a
    /// member call, not a builtin). Builtins are resolved by the runtime.
    Builtin(String),
    /// Pre-lowered React-IR nodes for a component call's `children` prop.
    /// Produced by lowering JSX children of a component element; evaluates to
    /// a `Value::Children` holding the nodes verbatim (they still reference
    /// the PARENT's scope — the child splices them, it does not own them).
    Children(Vec<crate::react::ReactNode>),
}

/// One await-delimited segment of an async function (M2-T07). `stmts` run in
/// order; the segment's value (or the resolved `await_expr`) drives the
/// continuation. Serde so the artifact stays serializable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsAsyncSegment {
    pub stmts: Vec<JsExpr>,
    /// Terminal await: this segment suspends until this expression's promise
    /// settles (None = the segment completes the function with its value).
    pub await_expr: Option<Box<JsExpr>>,
    /// `let x = await p` / `x = await p` — the resolved value binds to x.
    pub await_bind: Option<String>,
    /// `return await p` — the resolved value completes the async function.
    pub await_completes: bool,
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
    StrictEq,
    StrictNeq,
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
            JsBinOp::StrictEq => "===",
            JsBinOp::StrictNeq => "!==",
            JsBinOp::Lt => "<",
            JsBinOp::Gt => ">",
            JsBinOp::Le => "<=",
            JsBinOp::Ge => ">=",
            JsBinOp::And => "&&",
            JsBinOp::Or => "||",
        }
    }
}
