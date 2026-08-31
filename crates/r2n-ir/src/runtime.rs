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
    /// Class-component info (`class X extends Component { ... }`), `None`
    /// for function components.
    #[serde(default)]
    pub class: Option<ClassInfo>,
}

/// Class-component lowering: the `state` initializer and the methods
/// (`render` body is already the component `body` above).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassInfo {
    pub state: Option<JsExpr>,
    /// Methods by name: params + body (a Block of statements).
    pub methods: Vec<(String, ClassMethod)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassMethod {
    pub params: Vec<String>,
    pub body: JsExpr,
}

/// Artifact format version. Bumped on any breaking change to the serialized
/// shape of `RuntimeTemplate` — a runtime that receives an artifact with an
/// unknown major version must reject it (ABI rule, RUNTIME_ABI spec).
pub const ARTIFACT_FORMAT_VERSION: u32 = 1;

/// The whole compiled program: a table of components plus the root index.
/// Carries an artifact manifest: format version + generator version, so any
/// consumer can verify compatibility before executing (stamped by the
/// compiler; round-trips through JSON with the artifact).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RuntimeTemplate {
    /// Dev build flag: in dev, effects run the React StrictMode double
    /// invoke (mount → cleanup → mount). Production artifacts NEVER carry
    /// it (absent = false; StrictMode nodes are stripped at lowering).
    #[serde(default)]
    pub strict_mode: bool,

    pub components: Vec<RuntimeComponent>,
    pub root: usize,
    /// Artifact manifest (M0.3-T09): format + generator stamps.
    #[serde(default)]
    pub manifest: ArtifactManifest,
}

/// Version stamps every compiled artifact carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArtifactManifest {
    /// Serialization format major version (see ARTIFACT_FORMAT_VERSION).
    pub format_version: u32,
    /// Version of the compiler that produced this artifact.
    #[serde(default)]
    pub compiler_version: (u32, u32, u32),
}

impl RuntimeTemplate {
    pub fn new() -> Self {
        let mut parts: Vec<u32> = env!("CARGO_PKG_VERSION")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        parts.resize(3, 0);
        Self {
            manifest: ArtifactManifest {
                format_version: ARTIFACT_FORMAT_VERSION,
                compiler_version: (parts[0], parts[1], parts[2]),
            },
            ..Self::default()
        }
    }

    pub fn root_component(&self) -> &RuntimeComponent {
        &self.components[self.root]
    }
}
