//! End-to-end tests for the FIFO scheduler (M0.2-T04): batching, dedup, and
//! deterministic order — the reactive loop's re-render discipline.

use r2n_compiler::compile_source;
use r2n_renderer_memory::{MemNode, MemoryRenderer};
use r2n_runtime::patch::Patch;
use r2n_runtime::{NodeId, Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let t = compile_source(src).expect("compile");
    let mut rt = Runtime::new(t);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

fn find_button(r: &MemoryRenderer, text: &str) -> Option<NodeId> {
    r.nodes().iter().find_map(|(id, n)| match n {
        MemNode::Element { tag, .. } if tag == "button" => {
            let kids = r.children_of().get(&Some(*id)).cloned().unwrap_or_default();
            if kids
                .iter()
                .any(|k| matches!(r.nodes().get(k), Some(MemNode::Text { text: t }) if t == text))
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
fn multiple_setters_in_one_handler_batch_to_one_render() {
    // One click handler sets the SAME state slot three times (as three setter
    // calls). Batched scheduling means ONE re-render pass — the final value
    // wins, and the patch stream shows a single text update to "3".
    let src = r#"
        component App() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            return (
                <div>
                    <h1>{n}</h1>
                    <button onClick={() => { setN(1); setN(2); setN(3); }}>batch</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = find_button(&r, "batch").expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    assert_eq!(
        r.render_string(),
        "<div><h1>3</h1><button>batch</button></div>"
    );
    // Exactly one SetText for the h1: the intermediate 1 and 2 never rendered.
    let set_texts = patches
        .iter()
        .filter(|p| matches!(p, Patch::SetText { text, .. } if text == "3"))
        .count();
    assert_eq!(set_texts, 1, "one batched render: {patches:?}");
    assert!(
        !patches
            .iter()
            .any(|p| matches!(p, Patch::SetText { text, .. } if text == "1" || text == "2")),
        "intermediate states must not render: {patches:?}"
    );
}

#[test]
fn independent_instances_dirty_independently() {
    // Two counters; clicking each is a separate dispatch cycle. State updates
    // in one never leak into the other (scheduler keyed by instance path).
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            return <button onClick={() => setN(n + 1)}>{n}</button>;
        }
        component App() {
            return <div><Counter/><Counter/><Counter/></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert_eq!(
        r.render_string(),
        "<div><button>0</button><button>0</button><button>0</button></div>"
    );
    let buttons: Vec<NodeId> = r
        .nodes()
        .iter()
        .filter(|(_, n)| matches!(n, MemNode::Element { tag, .. } if tag == "button"))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(buttons.len(), 3);
    // Click second counter twice.
    for _ in 0..2 {
        let ps = rt.dispatch(buttons[1], "onClick").expect("dispatch");
        r.apply(&ps);
    }
    assert_eq!(
        r.render_string(),
        "<div><button>0</button><button>2</button><button>0</button></div>",
        "only the clicked instance's state advanced"
    );
}

#[test]
fn flush_is_idempotent_when_clean() {
    // A flush with no dirty frames and empty scheduler emits zero patches.
    let src = r#"
        component App() {
            let n = 5;
            return <p>{n}</p>;
        }
        export default App;
    "#;
    let (mut rt, _) = setup(src);
    let patches = rt.flush().expect("flush");
    assert!(
        patches.is_empty(),
        "clean flush must emit nothing: {patches:?}"
    );
}
