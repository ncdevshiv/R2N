//! Serialization of the compiled artifact — the language-neutral output.
//!
//! The runtime template is `serde`-serializable to JSON (and could be CBOR/
//! postcard). This is the concrete form of the "language-independent artifact"
//! in the architecture: a Rust runtime and a (future) Go runtime both consume
//! the same JSON. No closures or source are embedded — only ABI primitives.

use crate::runtime::RuntimeTemplate;

/// Serialize a runtime template to pretty JSON.
pub fn to_json(t: &RuntimeTemplate) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(t)
}

/// Serialize a runtime template to compact JSON bytes.
pub fn to_json_bytes(t: &RuntimeTemplate) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(t)
}

/// Deserialize a runtime template from JSON bytes.
pub fn from_json_bytes(bytes: &[u8]) -> Result<RuntimeTemplate, serde_json::Error> {
    serde_json::from_slice(bytes)
}
