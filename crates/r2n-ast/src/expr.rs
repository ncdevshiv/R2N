//! Expressions in the R2N source language.

use crate::lit::Literal;
use crate::op::{BinOp, UnOp};
use std::fmt;

/// A source-level expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal value.
    Literal(Literal),
    /// A reference to a name (variable, prop, or component). `is_component` is
    /// set when the identifier begins with an uppercase letter, per JSX/React
    /// convention — this is how we distinguish `<Counter/>` from `<div/>`.
    Ident { name: String, is_component: bool },
    /// Member access: `obj.prop`.
    Member { base: Box<Expr>, prop: String },
    /// Binary operation.
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary operation.
    Unary { op: UnOp, expr: Box<Expr> },
    /// Function call: `callee(args...)`. Callee may be a component identifier.
    Call { callee: Box<Expr>, args: Vec<Expr> },
    /// `new Callee(args...)` — allocates an instance (ES classes).
    New { callee: Box<Expr>, args: Vec<Expr> },
    /// Assignment: `target = value` (target is an identifier or a member
    /// access). Right-associative; used by `useRef`'s `.current` writes.
    Assign { target: Box<Expr>, value: Box<Expr> },
    /// JSX element expression: `<Tag props>children</Tag>`.
    Element(Element),
    /// JSX/JS conditional expression: `cond ? then : else`.
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    /// Array literal: `[a, b, c]`.
    Array(Vec<Expr>),
    /// Arrow function: `params => body`. The body is a single expression or a
    /// block of expression statements (`() => { a(); b(); }`). `async` marks
    /// an async arrow (`async () => { await p; ... }`, M2-T07).
    Arrow {
        params: Vec<String>,
        body: Box<Expr>,
        #[allow(dead_code)]
        async_: bool,
    },
    /// `await expr` — suspends an async function at a segment boundary until
    /// expr's promise settles (M2-T07). Only valid as the statement value of
    /// an async body (`let x = await p;` / `x = await p;` / `await p;` /
    /// `return await p;`); the lowerer rejects other positions. `from_return`
    /// marks the `return await p` form (the resolved value COMPLETES the
    /// async fn, vs plain `await p;` which only suspends).
    Await { value: Box<Expr>, from_return: bool },
    /// A block of expression statements, evaluated in order; the block's value
    /// is its last expression. Produced for block-bodied arrows.
    Block(Vec<Expr>),
    /// `throw value` — raises `value` to the nearest enclosing `try`.
    Throw(Box<Expr>),
    /// `try { block } catch (param) { catch } finally { finally }` — at least
    /// one of catch/finally is present (ECMA grammar). Block bodies hold
    /// statement-level expressions (lets arrive pre-lowered as Assigns).
    Try {
        block: Vec<Expr>,
        catch_param: Option<String>,
        catch: Option<Vec<Expr>>,
        finally: Option<Vec<Expr>>,
    },
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Literal(l) => write!(f, "{l}"),
            Expr::Ident { name, .. } => write!(f, "{name}"),
            Expr::Member { base, prop } => write!(f, "{base}.{prop}"),
            Expr::Binary { op, left, right } => write!(f, "({left} {} {right})", op.as_str()),
            Expr::Unary { op, expr } => write!(f, "({}{expr})", op.as_str()),
            Expr::New { callee, args } => {
                write!(f, "new {callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ")")
            }
            Expr::Call { callee, args } => {
                write!(f, "{callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ")")
            }
            Expr::Element(e) => write!(f, "<{}/{:?}>", e.tag, e.props.len()),
            Expr::Ternary { cond, then, else_ } => write!(f, "({cond} ? {then} : {else_})"),
            Expr::Assign { target, value } => write!(f, "({target} = {value})"),
            Expr::Array(items) => {
                write!(f, "[")?;
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{it}")?;
                }
                write!(f, "]")
            }
            Expr::Arrow {
                params,
                body,
                async_,
            } => {
                if *async_ {
                    write!(f, "async ({}) => {body}", params.join(", "))
                } else {
                    write!(f, "({}) => {body}", params.join(", "))
                }
            }
            Expr::Await { value, .. } => write!(f, "await {value}"),
            Expr::Block(stmts) => {
                write!(f, "{{")?;
                for s in stmts {
                    write!(f, "{s}; ")?;
                }
                write!(f, "}}")
            }
            Expr::Throw(v) => write!(f, "throw {v}"),
            Expr::Try {
                block,
                catch_param,
                catch,
                finally,
            } => {
                write!(f, "try {{ {block:?} }} ")?;
                if let Some(p) = catch_param {
                    write!(f, "catch ({p}) {{ {catch:?} }} ")?;
                } else if let Some(c) = catch {
                    write!(f, "catch {{ {c:?} }} ")?;
                }
                if let Some(fl) = finally {
                    write!(f, "finally {{ {fl:?} }}")?;
                }
                Ok(())
            }
        }
    }
}

/// A JSX element: a tag, attributes, and child expressions.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// Tag name. For host elements this is a lowercase tag (e.g. `"div"`);
    /// for components it is the component identifier (e.g. `"Counter"`).
    pub tag: String,
    /// Whether this element's tag is a component (uppercase) vs a host element.
    pub is_component: bool,
    /// JSX attributes, e.g. `count={n}` or `className="x"`.
    pub props: Vec<Prop>,
    /// Child expressions (may include elements, text, and conditionals).
    pub children: Vec<Expr>,
}

/// A JSX attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub name: String,
    /// `Some(expr)` for `name={expr}`; `None` for the shorthand boolean
    /// `name` (treated as `name={true}`).
    pub value: Option<Expr>,
}
