//! Top-level program declarations.

use crate::expr::Expr;

/// A top-level declaration in an R2N source file.
#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    /// `component Name(props) { return <.../>; }`
    Component(Component),
    /// `class Name extends Component { state = ...; render() {...} }`
    Class(ClassComponent),
    /// `import { X, Y } from "module";` — limited to importing other R2N
    /// components from local files (path is resolved relative to the source).
    Import(Import),
    /// `export default Name;` — marks the root component of the app.
    ExportDefault(String),
    /// `export { a, b as c };` — named exports of module-level declarations
    /// (components, classes, generator fns). Each pair is `(local, exported)`:
    /// the alias form `b as c` exports local binding `b` under the name `c`
    /// (M2-T09).
    ExportNamed(ExportNamed),
    /// `function* name(params) { ... }` — a generator function (M2-T08).
    /// The body is a statement list; `yield` splits it into segments.
    GeneratorFn(GeneratorFn),
    /// `function name(params) { ... }` — a plain (non-generator) function
    /// declaration. Evaluates to a first-class `Value::Function` bound under
    /// `name` in the enclosing scope.
    FuncDecl(FuncDecl),
    /// `export function name(params) { ... }` / `export const name = ...;` —
    /// an inline-exported declaration: declares the binding AND registers it
    /// as a named export.
    ExportDecl(ExportDecl),
    /// Top-level `let`/`const` (module-scope bindings, T09b): evaluated once
    /// in source order when the module initializes; visible to every
    /// component and function in the module via the global env.
    TopLevel {
        kind: DeclKind,
        pattern: Pattern,
        value: Expr,
    },
}

/// A plain function declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FuncDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
}

/// One function parameter: a binding pattern with an optional default.
/// `(x)`, `(x = 1)`, `({a, b})`, `([x, y] = pair)`, `(...rest)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub pattern: Pattern,
    pub default: Option<Expr>,
    pub rest: bool,
}

/// An inline-exported declaration: `export function f() {}` or
/// `export const x = expr;`.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportDecl {
    Function(FuncDecl),
    Const { name: String, value: Expr },
}

/// A top-level generator function declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorFn {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
}

/// A named-export declaration: `export { a, b as c };` (M2-T09).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportNamed {
    /// `(local, exported)` pairs: the local binding name and the name it is
    /// exported under (equal unless the `b as c` alias form was used).
    pub names: Vec<(String, String)>,
}

/// A component definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: String,
    /// Parameter names (the component receives a single `props` object in
    /// React, but R2N components take explicit named params for clarity and
    /// to avoid `props.x` plumbing in the supported subset).
    pub params: Vec<Param>,
    /// The render body: a list of statements, the last being `return <.../>`.
    pub body: Vec<Stmt>,
}

/// A class component: `class Name extends Component`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassComponent {
    pub name: String,
    /// The base class (`extends X`), if any. `Component` is the React
    /// special base (M1-T12); other bases are ordinary ES classes (M2-T04).
    pub extends: Option<String>,
    /// The `state = expr;` initializer (React component form).
    pub state: Option<Expr>,
    /// Methods by name — `constructor` is the ES constructor; `render` is
    /// the React lifecycle; componentDidMount / componentDidUpdate /
    /// componentWillUnmount are React lifecycle.
    pub methods: Vec<Method>,
}

/// A class-component method: `name(params) { body }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    pub name: String,
    pub params: Vec<Param>,
    /// Body statements; `render`'s body ends with `return <.../>`.
    pub body: Vec<Stmt>,
}

/// `import ... from "path";` — importing declarations from another R2N
/// module (the specifier resolves relative to the importing source, M2-T09).
///
/// Supports the static ES module forms:
///   - `import { a, b as c } from "path"` — named bindings, with optional
///     local aliasing (`(imported, local)` pairs)
///   - `import Def from "path"` — default binding
///   - `import * as ns from "path"` — namespace binding
///   - `import "path"` — side-effect only (all binding fields empty)
///
/// and the combinations `import Def, { a } from "path"` /
/// `import Def, * as ns from "path"`.
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// Default binding, if any: `import Def from ...`.
    pub default_: Option<String>,
    /// Named bindings as `(imported, local)` pairs: `import { a, b as c }`.
    pub named: Vec<(String, String)>,
    /// Namespace binding: `import * as ns from ...`.
    pub namespace: Option<String>,
    /// The module specifier (a relative path to another R2N source file).
    pub path: String,
}

impl Import {
    /// True when the import binds nothing (side-effect-only `import "path"`).
    pub fn is_side_effect(&self) -> bool {
        self.default_.is_none() && self.namespace.is_none() && self.named.is_empty()
    }
}

/// A statement inside a component body.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `let name = expr;` — variable binding.
    Let { name: String, value: Expr },
    /// `const name = expr;` — read-only binding (treated same as `let` at IR
    /// level; immutability is not enforced beyond this declaration).
    Const { name: String, value: Expr },
    /// `let {a, b: c} = expr;` / `const [x, ...rest] = expr;` — destructuring
    /// binding. Each `(pattern, default)` pair binds one name; `default` is
    /// used when the destructured value is `undefined`.
    Destructure {
        kind: DeclKind,
        pattern: Pattern,
        value: Expr,
    },
    /// `return expr;` — must produce an `Expr::Element` (or a conditional that
    /// resolves to one) in a component body.
    Return(Expr),
    /// A bare expression evaluated for its side effects (e.g. `useEffect(...)`),
    /// with an optional trailing `;`.
    Expr(Expr),
    /// `if (cond) { ... } else { ... }` — statement form. The `else` branch is
    /// optional; a lone `if` without `else` evaluates to null when false.
    If {
        cond: Expr,
        then: Vec<Stmt>,
        else_: Option<Vec<Stmt>>,
    },
    /// `while (cond) { ... }` — loop until `cond` is falsy.
    While { cond: Expr, body: Vec<Stmt> },
    /// `for (init; cond; update) { ... }` — C-style loop. `init` is an optional
    /// `let`/`const` declaration or expression; `cond` defaults to `true` when
    /// absent; `update` is an optional expression.
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Stmt>,
    },
    /// `switch (disc) { case a: ...; default: ... }` — first matching case runs,
    /// then FALL THROUGH continues into subsequent cases until `break`.
    Switch {
        disc: Expr,
        cases: Vec<(Option<Expr>, Vec<Stmt>)>,
    },
    /// `break;` — exit the innermost loop or switch.
    Break,
    /// `continue;` — skip to the next loop iteration.
    Continue,
}

/// `let` vs `const` for destructuring bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Let,
    Const,
}

/// A binding pattern for destructuring declarations and parameters:
/// `x`, `{a, b: c = d, ...rest}`, `[x, , y = d, ...rest]`.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// A single name, with an optional default (`x = 3`).
    Name { name: String, default: Option<Expr> },
    /// `{a, b: c = d, ...rest}` — property bindings plus optional rest name.
    Object {
        props: Vec<ObjectProp>,
        rest: Option<String>,
    },
    /// `[a, , b = d, ...rest]` — positional bindings (holes skipped) plus
    /// optional rest name.
    Array {
        items: Vec<Option<Pattern>>,
        rest: Option<String>,
    },
}

/// One property binding inside an object pattern: `key`, `key: pat`, or
/// `key = default` (shorthand with default). `alias` is `None` for shorthand.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectProp {
    pub key: String,
    pub alias: Option<Pattern>,
}

impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pattern::Name { name, default } => {
                write!(f, "{name}")?;
                if let Some(d) = default {
                    write!(f, " = {d}")?;
                }
                Ok(())
            }
            Pattern::Object { props, rest } => {
                write!(f, "{{")?;
                for (i, p) in props.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p.key)?;
                    if let Some(a) = &p.alias {
                        write!(f, ": {a}")?;
                    }
                }
                if let Some(r) = rest {
                    if !props.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "...{r}")?;
                }
                write!(f, "}}")
            }
            Pattern::Array { items, rest } => {
                write!(f, "[")?;
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    if let Some(p) = it {
                        write!(f, "{p}")?;
                    }
                }
                if let Some(r) = rest {
                    if !items.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "...{r}")?;
                }
                write!(f, "]")
            }
        }
    }
}

/// A complete R2N program: a set of declarations plus the root component name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub decls: Vec<Decl>,
    pub root: Option<String>,
}

impl Program {
    pub fn new() -> Self {
        Self::default()
    }
}
