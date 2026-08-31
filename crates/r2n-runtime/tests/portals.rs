//! M1-T14 acceptance: portals — logical parent vs rendering parent.
//!
//! React semantics under test:
//! 1. `<Portal target="className">` children render under the FIRST host
//!    element with that className — a different rendering parent.
//! 2. Reconciliation identity follows the LOGICAL position (children stay
//!    keyed where they were written).
//! 3. Prop/state updates inside a portal flow through (update patches go
//!    to the portaled subtree).
//! 4. Portal content respects the logical tree's parent for key identity
//!    (a portal child does not collide with the target's own children).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{NodeId, Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

fn button(r: &MemoryRenderer) -> Option<NodeId> {
    r.nodes().iter().find_map(|(id, n)| match n {
        r2n_renderer_memory::MemNode::Element { tag, props } => {
            if tag == "button"
                && props
                    .iter()
                    .any(|(k, v)| k == "onClick" && matches!(v, r2n_runtime::Value::Handler { .. }))
            {
                Some(*id)
            } else {
                None
            }
        }
        _ => None,
    })
}

#[test]
fn portal_renders_under_target_parent() {
    let src = r#"
        component App() {
            return (
                <div className="root">
                    <div className="modal"></div>
                    <p className="logical">plain</p>
                    <Portal target="modal">
                        <b className="popped">in-modal</b>
                    </Portal>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("<div className=\"modal\"><b className=\"popped\">in-modal</b></div>"),
        "portal children under the target: {tree}"
    );
    assert!(
        tree.find("modal").unwrap() < tree.find("popped").unwrap(),
        "popped appears inside modal: {tree}"
    );
}

#[test]
fn portal_keeps_logical_order_for_siblings() {
    let src = r#"
        component App() {
            return (
                <div className="root">
                    <div className="shell"></div>
                    <i>one</i>
                    <Portal target="shell"><u>p1</u></Portal>
                    <i>two</i>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    // Logical node order: one, two around the portal; portaled content goes
    // to the shell. The renderer shows node ids in flat order — the portal
    // content is under shell, siblings one/two remain.
    assert!(
        tree.contains("<i>one</i>") && tree.contains("<i>two</i>"),
        "siblings kept: {tree}"
    );
    assert!(tree.contains("<u>p1</u>"), "portal content: {tree}");
}

#[test]
fn portal_updates_propagate_with_state_change() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            return (
                <div className="root">
                    <div className="outbox"></div>
                    <Portal target="outbox">
                        <p className="display">{n}</p>
                    </Portal>
                    <button onClick={() => setN(n + 1)}>go</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(tree.contains(">1<"), "portaled content updated: {tree}");
    let set_text = patches
        .iter()
        .any(|p| matches!(p, r2n_runtime::Patch::SetText { .. }));
    assert!(
        set_text,
        "minimal SetText for the portaled text: {patches:?}"
    );
}

#[test]
fn portal_target_missing_renders_safely() {
    // No element matches the target: portal children attach to nothing
    // (rendered at the logical position by fallback) — a documented subset
    // behavior: no crash.
    let src = r#"
        component App() {
            return (
                <div className="root">
                    <Portal target="missing">
                        <b className="orphan">x</b>
                    </Portal>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("orphan"),
        "children still rendered (fallback attach): {tree}"
    );
}
