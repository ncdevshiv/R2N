//! The patch stream — the single ABI boundary between runtime and renderer.
//!
//! The runtime never hands the renderer a live node tree; instead it emits a
//! minimal, ordered sequence of `Patch` operations describing how the rendered
//! tree changed. All renderers (memory, native, WASM, browser, terminal) consume
//! this same stream. This is the ADR-010 "core loop" output:
//! `write state -> schedule dirty -> flush -> reconcile -> minimal Patch[]`.

use crate::value::Value;
use std::fmt;

/// A node identity in the rendered tree. The runtime assigns these; the renderer
/// tracks them. Opaque to the renderer beyond equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// A single mutation to apply to the rendered tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Patch {
    /// Create a host element under `parent` at `index` (child position).
    Create {
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
        tag: String,
    },
    /// Create a text node under `parent` at `index`.
    CreateText {
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
        text: String,
    },
    /// Set/update a property on an existing node.
    SetProp {
        id: NodeId,
        name: String,
        value: Value,
    },
    /// Update the text content of a text node.
    SetText { id: NodeId, text: String },
    /// Remove a node (and its subtree) from the tree.
    Remove { id: NodeId },
    /// Move an existing node to a new parent/position (reconciliation reorder).
    Move {
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
}

impl fmt::Display for Patch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Patch::Create {
                id,
                tag,
                parent,
                index,
            } => {
                write!(f, "Create {id} <{tag}> parent={parent:?} @ {index}")
            }
            Patch::CreateText {
                id,
                parent,
                index,
                text,
            } => {
                write!(f, "CreateText {id} '{}' parent={parent:?} @ {index}", text)
            }
            Patch::SetProp { id, name, value } => {
                write!(f, "SetProp {id}.{name} = {value}")
            }
            Patch::SetText { id, text } => write!(f, "SetText {id} = '{text}'"),
            Patch::Remove { id } => write!(f, "Remove {id}"),
            Patch::Move { id, parent, index } => {
                write!(f, "Move {id} parent={parent:?} @ {index}")
            }
        }
    }
}
