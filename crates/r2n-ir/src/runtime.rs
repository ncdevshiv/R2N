//! Runtime IR — the language-neutral artifact the runtime executes.
//!
//! This models ADR-010's "template vs instance" split. A `RuntimeTemplate`
//! (compile-time) describes the structure of the UI and the closures to run.
//! At runtime, `TemplateNodeId`s are instantiated into `NodeHandle`s carrying
//! per-instance state (hooks). The artifact is `serde`-serializable so it is a
//! genuine, language-neutral compile output — a Rust runtime and a (future) Go
//! runtime can both consume it because it references only the ABI primitives.

use crate::js::JsExpr;
use crate::react::ReactNode;
use serde::{Deserialize, Serialize};

/// Compile-time identity of a node within a template. Stable across renders;
/// used as the reconciliation key within a component instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TemplateNodeId(pub u32);

/// A component in the runtime IR: its name, captured (free) variable names,
/// parameter names, and the body expression to evaluate on render. The body
/// lowers to either a `ReactNode` (the VNode) or a `JsExpr` for non-node
/// returns (not exercised in the supported subset, but kept for completeness).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeComponent {
    pub name: String,
    pub params: Vec<String>,
    /// Names the component closes over (from `let`/`const` in its scope, or
    /// imported components). The runtime supplies these via the frame protocol.
    pub captures: Vec<String>,
    /// Top-level bindings established in the component body, evaluated in order
    /// at render time and available to the return expression. Each is a `let`.
    pub bindings: Vec<(String, JsExpr)>,
    /// The render body: a React node tree (possibly with conditionals/lists).
    pub body: ReactNode,
}

/// The whole compiled program: a table of components plus the root index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RuntimeTemplate {
    pub components: Vec<RuntimeComponent>,
    pub root: usize,
}

impl RuntimeTemplate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn root_component(&self) -> &RuntimeComponent {
        &self.components[self.root]
    }
}
