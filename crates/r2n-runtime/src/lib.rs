//! R2N zero-JS runtime.
//!
//! Modules:
//! * `value`  — the ABI value set (null/bool/number/utf16 string/array/map/
//!   component/builtin/setter)
//! * `hooks`  — `useState`/`useEffect` via the frame protocol (ADR-002/003)
//! * `eval`   — JS IR evaluator (the zero-JS interpreter)
//! * `patch`  — the `Patch` stream (the ABI boundary to renderers)
//! * `engine` — render + keyed reconciliation (ADR-010) producing `Patch[]`
//! * `render` — the `Renderer` trait all backends implement

pub mod engine;
pub mod eval;
pub mod hooks;
pub mod patch;
pub mod render;
pub mod scheduler;
pub mod value;

pub use engine::{RenderedNode, Runtime};
pub use eval::{Env, Host};
pub use hooks::{EffectBody, HookFrame, Setter};
pub use patch::{NodeId, Patch};
pub use render::Renderer;
pub use scheduler::Scheduler;
pub use value::{RuntimeError, Value};
