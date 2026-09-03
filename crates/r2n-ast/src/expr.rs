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
    Ident {
        name: String,
        is_component: bool,
    },
    /// Member access: `obj.prop`.
    Member {
        base: Box<Expr>,
        prop: String,
    },
    /// Binary operation.
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary operation.
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    /// Function call: `callee(args...)`. Callee may be a component identifier.
    /// Args may be `...spread`.
    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
    },
    /// `new Callee(args...)` — allocates an instance (ES classes).
    New {
        callee: Box<Expr>,
        args: Vec<CallArg>,
    },
    /// Assignment: `target = value` (target is an identifier or a member
    /// access). Right-associative; used by `useRef`'s `.current` writes.
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// JSX element expression: `<Tag props>children</Tag>`.
    Element(Element),
    /// JSX/JS conditional expression: `cond ? then : else`.
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    /// Array literal: `[a, b, c]` — items may be `...spread`.
    Array(Vec<ArrayItem>),
    /// Object literal: `{a, b: expr, ...spread}`.
    Object(Vec<ObjectItem>),
    /// Template literal: `` `a${x}b` `` — cooked string parts interleaved with
    /// interpolated expressions (`parts.len() == exprs.len() + 1`).
    Template {
        parts: Vec<String>,
        exprs: Vec<Expr>,
    },
    /// `x++`, `x--`, `++x`, `--x`.
    Update {
        op: UpdateOp,
        target: Box<Expr>,
        prefix: bool,
    },
    /// Compound assignment: `x += v`, `x -= v`, `x *= v`, `x /= v`, `x %= v`.
    CompoundAssign {
        op: BinOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// Arrow function: `params => body`. The body is a single expression or a
    /// block of expression statements (`() => { a(); b(); }`). `async` marks
    /// an async arrow (`async () => { await p; ... }`, M2-T07). Params are
    /// full patterns (`({a, b})`, `(x = 1)`, `(...rest)`).
    Arrow {
        params: Vec<crate::program::Param>,
        body: Box<Expr>,
        #[allow(dead_code)]
        async_: bool,
    },
    /// `function Name?(params) { stmts }` in expression position — a
    /// first-class function value with a full statement body (early
    /// `return`, loops, `switch` all lower to the runtime's control-flow
    /// channel, caught at the call boundary). `name` is documentation
    /// (named function expressions bind their name only inside themselves
    /// in full ES; ours treats it as anonymous — the outer binding, e.g.
    /// `const Item = memo(function Item() {...})`, supplies the name).
    Function {
        name: Option<String>,
        params: Vec<crate::program::Param>,
        body: Vec<crate::program::Stmt>,
    },
    /// `return expr?;` in block-expression position (try/catch/finally
    /// bodies): raises function-return control flow carrying the value
    /// (`None` = bare `return;` → undefined). The runtime catches it at the
    /// function-call boundary (and async/generator step boundaries, where
    /// it completes the unit). Without this, a `return` inside `try` would
    /// silently fall through to the code after the `try`.
    Return(Option<Box<Expr>>),
    /// `yield expr` — suspends a generator, producing expr (or undefined) as
    /// the `{value, done: false}` result (M2-T08). Statement-position only
    /// (`yield v;` / `let x = yield v;` / `x = yield v;` / `return yield v;`);
    /// the lowerer rejects other positions. `from_return` marks the
    /// `return yield` form (the next `next(arg)` COMPLETES the generator
    /// with arg instead of resuming).
    Yield {
        value: Option<Box<Expr>>,
        from_return: bool,
    },
    /// `await expr` — suspends an async function at a segment boundary until
    /// expr's promise settles (M2-T07). Only valid as the statement value of
    /// an async body (`let x = await p;` / `x = await p;` / `await p;` /
    /// `return await p;`); the lowerer rejects other positions. `from_return`
    /// marks the `return await p` form (the resolved value COMPLETES the
    /// async fn, vs plain `await p;` which only suspends).
    Await {
        value: Box<Expr>,
        from_return: bool,
    },
    /// `import("path")` — dynamic import (M2-T09). The specifier must be a
    /// string literal: the linker resolves it to a module at compile time and
    /// lowering rewrites the expression to the reserved `@module:N` variable,
    /// which the runtime evaluates to the module's namespace record.
    DynImport {
        specifier: String,
    },
    /// A block of expression statements, evaluated in order; the block's value
    /// is its last expression. Produced for block-bodied arrows.
    Block(Vec<Expr>),
    /// `throw value` — raises `value` to the nearest enclosing `try`.
    Throw(Box<Expr>),
    /// `while (cond) body` — loop expression form (from statement lowering
    /// inside block bodies). Value is null; `break`/`continue` travel the
    /// runtime control-flow channel.
    While {
        cond: Box<Expr>,
        body: Box<Expr>,
    },
    /// `break` / `continue` in expression position (from statement lowering
    /// inside block bodies). The runtime raises control flow; a stray use
    /// outside a loop/switch is a runtime error.
    Break,
    Continue,
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
                    match a {
                        CallArg::Expr(e) => write!(f, "{e}")?,
                        CallArg::Spread(e) => write!(f, "...{e}")?,
                    }
                }
                write!(f, ")")
            }
            Expr::Call { callee, args } => {
                write!(f, "{callee}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match a {
                        CallArg::Expr(e) => write!(f, "{e}")?,
                        CallArg::Spread(e) => write!(f, "...{e}")?,
                    }
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
                    match it {
                        ArrayItem::Expr(e) => write!(f, "{e}")?,
                        ArrayItem::Spread(e) => write!(f, "...{e}")?,
                    }
                }
                write!(f, "]")
            }
            Expr::Object(items) => {
                write!(f, "{{")?;
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match it {
                        ObjectItem::Shorthand(n) => write!(f, "{n}")?,
                        ObjectItem::Prop(k, v) => write!(f, "{k}: {v}")?,
                        ObjectItem::Spread(e) => write!(f, "...{e}")?,
                    }
                }
                write!(f, "}}")
            }
            Expr::Template { parts, exprs } => {
                write!(f, "`")?;
                for (i, p) in parts.iter().enumerate() {
                    write!(f, "{p}")?;
                    if i < exprs.len() {
                        write!(f, "${{{}}}", exprs[i])?;
                    }
                }
                write!(f, "`")
            }
            Expr::Update { op, target, prefix } => {
                let s = match op {
                    UpdateOp::Inc => "++",
                    UpdateOp::Dec => "--",
                };
                if *prefix {
                    write!(f, "({s}{target})")
                } else {
                    write!(f, "({target}{s})")
                }
            }
            Expr::CompoundAssign { op, target, value } => {
                write!(f, "({target} {} {value})", op.as_str())
            }
            Expr::Arrow {
                params,
                body,
                async_,
            } => {
                let ps = param_list_string(params);
                if *async_ {
                    write!(f, "async ({}) => {body}", ps)
                } else {
                    write!(f, "({}) => {body}", ps)
                }
            }
            Expr::Function { name, params, body } => {
                write!(
                    f,
                    "function {}({}) {{ {} stmts }}",
                    name.as_deref().unwrap_or(""),
                    param_list_string(params),
                    body.len()
                )
            }
            Expr::Await { value, .. } => write!(f, "await {value}"),
            Expr::Yield { value, .. } => match value {
                Some(v) => write!(f, "yield {v}"),
                None => write!(f, "yield"),
            },
            Expr::Block(stmts) => {
                write!(f, "{{")?;
                for s in stmts {
                    write!(f, "{s}; ")?;
                }
                write!(f, "}}")
            }
            Expr::Throw(v) => write!(f, "throw {v}"),
            Expr::Return(v) => match v {
                Some(e) => write!(f, "return {e}"),
                None => write!(f, "return"),
            },
            Expr::While { cond, body } => write!(f, "(while {cond} {body})"),
            Expr::Break => write!(f, "break"),
            Expr::Continue => write!(f, "continue"),
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
            Expr::DynImport { specifier } => write!(f, "import({specifier:?})"),
        }
    }
}

/// One argument in a call: a plain expression or `...spread`.
#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    Expr(Expr),
    Spread(Expr),
}

/// One item inside an array literal: a plain element or `...spread`.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayItem {
    Expr(Expr),
    Spread(Expr),
}

/// One item inside an object literal.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectItem {
    /// `{name}` — shorthand (value is the in-scope binding).
    Shorthand(String),
    /// `{key: value}`.
    Prop(String, Expr),
    /// `{...expr}` — spread own enumerable props.
    Spread(Expr),
}

/// `++` / `--`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOp {
    Inc,
    Dec,
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

/// Render a parameter list (`x`, `x = dflt`, `{a, b}`, `...rest`).
pub fn param_list_string(params: &[crate::program::Param]) -> String {
    params
        .iter()
        .map(|p| {
            let mut s = String::new();
            if p.rest {
                s.push_str("...");
            }
            s.push_str(&p.pattern.to_string());
            if let Some(d) = &p.default {
                s.push_str(&format!(" = {d}"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}
