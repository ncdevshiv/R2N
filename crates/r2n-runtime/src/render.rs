//! The `Renderer` trait — the contract every backend implements.
//!
//! A renderer consumes the runtime's `Patch` stream (the ABI boundary) and
//! applies it to a concrete output. This is the single seam the architecture
//! defines: a memory renderer (this crate), a native renderer, a WASM/browser
//! renderer, and a terminal renderer all implement `Renderer` and receive the
//! *same* `Patch[]`. The runtime never knows which backend is attached.

use crate::patch::Patch;

/// A renderer applies a batch of patches (the output of one `Runtime::flush`)
/// to its backing tree. Implementations must be deterministic and idempotent
/// given the same ordered patch sequence from the same starting state.
pub trait Renderer {
    /// Apply a list of patches in order.
    fn apply(&mut self, patches: &[Patch]);

    /// Return a stable textual representation of the current tree (for tests,
    /// debugging, and the `--render` CLI output). Not part of the ABI.
    fn render_string(&self) -> String;
}
