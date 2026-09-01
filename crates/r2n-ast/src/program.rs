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
}

/// A top-level generator function declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorFn {
    pub name: String,
    pub params: Vec<String>,
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
    pub params: Vec<String>,
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
    pub params: Vec<String>,
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
