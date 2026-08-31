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
    /// block of expression statements (`() => { a(); b(); }`).
    Arrow {
        params: Vec<String>,
        body: Box<Expr>,
    },
    /// A block of expression statements, evaluated in order; the block's value
    /// is its last expression. Produced for block-bodied arrows.
    Block(Vec<Expr>),
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
            Expr::Arrow { params, body } => {
                write!(f, "({}) => {body}", params.join(", "))
            }
            Expr::Block(stmts) => {
                write!(f, "{{")?;
                for s in stmts {
                    write!(f, "{s}; ")?;
                }
                write!(f, "}}")
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
