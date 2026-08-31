//! In-memory renderer for the R2N runtime.
//!
//! Implements `r2n_runtime::Renderer` by maintaining an in-memory node tree and
//! applying the `Patch` stream to it. This is the reference backend used by the
//! test suite and the CLI `--render` mode. It consumes the *same* `Patch[]`
//! every other backend would, proving the ABI boundary is real and sufficient.

use r2n_runtime::patch::{NodeId, Patch};
use r2n_runtime::value::Value;
use r2n_runtime::Renderer;
use std::collections::BTreeMap;

/// A node in the in-memory tree. Children are tracked separately in
/// `children_of` (the single source of truth) so create/move/remove only touch
/// that map.
#[derive(Debug, Clone, PartialEq)]
pub enum MemNode {
    Element {
        tag: String,
        props: BTreeMap<String, Value>,
    },
    Text {
        text: String,
    },
}

/// The in-memory renderer.
pub struct MemoryRenderer {
    /// All live nodes, indexed by id.
    nodes: BTreeMap<NodeId, MemNode>,
    /// Children of each parent (None parent = root-level nodes).
    children_of: BTreeMap<Option<NodeId>, Vec<NodeId>>,
}

impl Default for MemoryRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryRenderer {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            children_of: BTreeMap::new(),
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&MemNode> {
        self.nodes.get(&id)
    }

    /// All live nodes, indexed by id.
    pub fn nodes(&self) -> &BTreeMap<NodeId, MemNode> {
        &self.nodes
    }

    /// Child lists keyed by parent (`None` = root level). This is the single
    /// source of truth for tree structure.
    pub fn children_of(&self) -> &BTreeMap<Option<NodeId>, Vec<NodeId>> {
        &self.children_of
    }

    pub fn root_nodes(&self) -> &[NodeId] {
        self.children_of
            .get(&None)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Render a single node to a readable XML-like string. Fragments are
    /// transparent (their children render inline).
    fn render_node(&self, id: NodeId) -> String {
        match self.nodes.get(&id) {
            Some(MemNode::Text { text }) => text.clone(),
            Some(MemNode::Element { tag, props }) => {
                // Event-handler props (`onClick` etc.) are runtime-internal:
                // they never render. `key` is reconciliation metadata and is
                // also hidden.
                let attr = props
                    .iter()
                    .filter(|(k, _)| !k.is_empty() && !k.starts_with("on") && k.as_str() != "key")
                    .map(|(k, v)| format!(" {k}={}", self.fmt_value(v)))
                    .collect::<String>();
                let kids = self.children_of.get(&Some(id)).cloned().unwrap_or_default();
                if kids.is_empty() {
                    format!("<{tag}{attr}/>")
                } else {
                    let inner = kids
                        .iter()
                        .map(|c| self.render_node(*c))
                        .collect::<Vec<_>>()
                        .join("");
                    format!("<{tag}{attr}>{inner}</{tag}>")
                }
            }
            None => String::new(),
        }
    }

    fn fmt_value(&self, v: &Value) -> String {
        match v {
            Value::Null => "null".to_string(),
            Value::Str(u) => format!("\"{}\"", String::from_utf16_lossy(u)),
            other => other.display(),
        }
    }
}

impl Renderer for MemoryRenderer {
    fn apply(&mut self, patches: &[Patch]) {
        for patch in patches {
            match patch {
                Patch::Create {
                    id,
                    parent,
                    index,
                    tag,
                } => {
                    self.nodes.insert(
                        *id,
                        MemNode::Element {
                            tag: tag.clone(),
                            props: BTreeMap::new(),
                        },
                    );
                    let kids = self.children_of.entry(*parent).or_default();
                    // Sparse indices (portal children target an external
                    // parent whose child count we don't know) append.
                    kids.insert((*index).min(kids.len()), *id);
                }
                Patch::CreateText {
                    id,
                    parent,
                    index,
                    text,
                } => {
                    self.nodes.insert(*id, MemNode::Text { text: text.clone() });
                    let kids = self.children_of.entry(*parent).or_default();
                    kids.insert((*index).min(kids.len()), *id);
                }
                Patch::SetProp { id, name, value } => {
                    if let Some(MemNode::Element { props, .. }) = self.nodes.get_mut(id) {
                        props.insert(name.clone(), value.clone());
                    }
                }
                Patch::SetText { id, text } => {
                    if let Some(n @ MemNode::Text { .. }) = self.nodes.get_mut(id) {
                        *n = MemNode::Text { text: text.clone() };
                    }
                }
                Patch::Remove { id } => {
                    // Detach from parent's child list and drop the subtree.
                    self.detach(*id);
                    self.drop_subtree(*id);
                }
                Patch::Move { id, parent, index } => {
                    self.detach(*id);
                    let kids = self.children_of.entry(*parent).or_default();
                    kids.insert((*index).min(kids.len()), *id);
                }
            }
        }
    }

    fn render_string(&self) -> String {
        self.root_nodes()
            .iter()
            .map(|id| self.render_node(*id))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl MemoryRenderer {
    fn detach(&mut self, id: NodeId) {
        for vec in self.children_of.values_mut() {
            vec.retain(|c| *c != id);
        }
    }

    fn drop_subtree(&mut self, id: NodeId) {
        // Collect descendant ids first (don't mutate while iterating).
        let mut stack = vec![id];
        let mut to_remove = Vec::new();
        while let Some(cur) = stack.pop() {
            to_remove.push(cur);
            if let Some(children) = self.children_of.get(&Some(cur)) {
                stack.extend(children.iter().copied());
            }
        }
        for rid in to_remove {
            self.nodes.remove(&rid);
        }
        self.children_of.remove(&Some(id));
    }
}
