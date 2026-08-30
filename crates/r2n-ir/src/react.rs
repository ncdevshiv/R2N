//! React IR — the node tree that components render.
//!
//! A `ReactNode` is produced by lowering a JSX element. Crucially, an element
//! may render a component (`<Counter/>`), in which case the React node carries
//! the *component reference* (`ComponentRef`) and the props expression; the
//! actual subtree is materialized at runtime by calling the component's render
//! closure. This is the ADR-002 "interlink": React IR references JS IR
//! closures rather than flattening them.

use crate::js::JsExpr;
use serde::{Deserialize, Serialize};

/// A node in the rendered tree (the "React IR" of a VNode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReactNode {
    /// A host element such as `<div>`. `tag` is lowercase. Children are
    /// themselves React nodes (or conditionals). Props carrying dynamic values
    /// are stored as expressions evaluated at render time.
    Host {
        tag: String,
        props: Vec<(String, JsExpr)>,
        children: Vec<ReactNode>,
    },
    /// A component instance: a call to a component by id, with props.
    Component {
        /// Index into the program's component table.
        component: ComponentRef,
        props: Vec<(String, JsExpr)>,
    },
    /// Conditional render: `cond ? a : b`. Lowered from `Expr::Ternary` when
    /// both branches are renderable nodes. The runtime chooses one.
    If {
        cond: JsExpr,
        then: Box<ReactNode>,
        else_: Box<ReactNode>,
    },
    /// A list produced by mapping an array through a renderer arrow: the
    /// canonical R2N way to render keyed lists. `items` is the array expr and
    /// `item` is the per-element React node template (with `key_expr` and
    /// `$item` bound to the element). Reconciliation keys on `key_expr`.
    List {
        items: JsExpr,
        key_expr: JsExpr,
        item: Box<ReactNode>,
    },
    /// A context provider: `<Ctx.Provider value={v}>` — `ctx` is the
    /// runtime value of the context handle, `value` is evaluated at render
    /// time and becomes the nearest value for the subtree.
    ContextProvider {
        ctx: JsExpr,
        value: JsExpr,
        children: Vec<ReactNode>,
    },
    /// The splice point for a parent's `children` prop inside a child
    /// component's body (`React children composition`). Lowered from the
    /// `children` identifier wherever it appears in render position. At
    /// render time the engine expands the `Value::Children` nodes carried by
    /// the current component's `children` prop, evaluating each in the
    /// PARENT's saved scope (composition is by reference, not by copy: the
    /// nodes still close over their original component).
    Children,
    /// A fragment (`<>...</>`): a group of children with no host element of
    /// its own. Transparent at reconciliation — its children splice into the
    /// parent's child list, so siblings and keyed identity flow through.
    /// `key` is the only prop React fragments accept (keyed fragments inside
    /// `.map` lists keep item identity across reorders).
    Fragment {
        key: Option<JsExpr>,
        children: Vec<ReactNode>,
    },
    /// A dynamic value rendered as text (e.g. `{count}`).
    Text(JsExpr),
}

/// Reference to a component in the program's component table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComponentRef(pub usize);

impl ComponentRef {
    pub fn index(self) -> usize {
        self.0
    }
}
