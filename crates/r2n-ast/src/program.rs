//! Top-level program declarations.

use crate::expr::Expr;

/// A top-level declaration in an R2N source file.
#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    /// `component Name(props) { return <.../>; }`
    Component(Component),
    /// `import { X, Y } from "module";` — limited to importing other R2N
    /// components from local files (path is resolved relative to the source).
    Import(Import),
    /// `export default Name;` — marks the root component of the app.
    ExportDefault(String),
}

/// A component definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: String,
    /// Parameter names (the component receives a single `props` object in
    /// React, but R2N components take explicit named params for clarity and
    /// to avoid `props.x` plumbing in the supported subset).
    pub params: Vec<String>,
    /// The render body: a list of statements, the last being `return <.../>`.
    pub body: Vec<Stmt>,
}

/// `import { a, b } from "path";`
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    pub names: Vec<String>,
    pub path: String,
}

/// A statement inside a component body.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `let name = expr;` — variable binding.
    Let { name: String, value: Expr },
    /// `const name = expr;` — read-only binding (treated same as `let` at IR
    /// level; immutability is not enforced beyond this declaration).
    Const { name: String, value: Expr },
    /// `return expr;` — must produce an `Expr::Element` (or a conditional that
    /// resolves to one) in a component body.
    Return(Expr),
    /// A bare expression evaluated for its side effects (e.g. `useEffect(...)`),
    /// with an optional trailing `;`.
    Expr(Expr),
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
