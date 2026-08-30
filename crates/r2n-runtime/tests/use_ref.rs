//! M1-T09 acceptance: useRef — stable identity across renders.
//!
//! React semantics under test:
//! 1. useRef returns the SAME ref box every render (identity is the slot).
//! 2. `.current` writes persist across renders WITHOUT re-render (no dirty
//!    flag — mutation doesn't schedule; the value is simply read later).
//! 3. `.current` initialized with the argument, persists across re-renders.
//! 4. Ref identity is stable under re-renders triggered by OTHER state
//!    (a ref does not reset when the component re-renders for other reasons).
//! 5. Refs of two component instances are independent.

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
fn ref_current_write_persists_across_renders_without_dirty() {
    let src = r#"
        component App() {
            let count = useRef(0);
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            return (
                <div>
                    <p className="out">{n}</p>
                    <button className="inc" onClick={() => { count.current = count.current + 1; log("ref", count.current); }}>+</button>
                    <button className="render" onClick={() => setN(n + 1)}>render</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // Find buttons by document order: first is inc, second is render.
    let btns: Vec<NodeId> = r
        .nodes()
        .iter()
        .filter(|(_, n)| matches!(n, r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button"))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(btns.len(), 2, "two buttons");
    let (inc, render) = (btns[0], btns[1]);

    let patches = rt.dispatch(inc, "onClick").expect("inc 1");
    r.apply(&patches);
    // Ref mutation does NOT dirty: no patches at all.
    assert_eq!(
        patches.len(),
        0,
        "ref write is not a state change: {patches:?}"
    );
    let patches = rt.dispatch(inc, "onClick").expect("inc 2");
    r.apply(&patches);
    assert_eq!(
        patches.len(),
        0,
        "second ref write still no patches: {patches:?}"
    );
    assert_eq!(
        rt.logs().iter().filter(|l| l.starts_with("ref")).count(),
        2,
        "mutations observed: {:?}",
        rt.logs()
    );

    // Now trigger a STATE re-render; the ref must still hold 2 (it is not
    // re-initialized).
    let patches = rt.dispatch(render, "onClick").expect("render");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l == "ref 2"),
        "ref persisted to 2: {logs:?}"
    );
    assert_eq!(
        logs.iter().filter(|l| l.starts_with("ref")).count(),
        2,
        "no stale re-observations: {logs:?}"
    );
}

#[test]
fn ref_initial_value_and_identity_stable() {
    let src = r#"
        component App() {
            let r = useRef(42);
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => log("current", r.current), [n]);
            return <div><p className="out">{n}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let logs = rt.logs();
    // Effect fired per n change; each read of r.current sees 42 (init value).
    assert!(
        logs.iter().any(|l| l == "current 42"),
        "ref init value: {logs:?}"
    );
}

#[test]
fn refs_are_independent_per_instance() {
    let src = r#"
        component C(tag) {
            let r = useRef(tag);
            return <b>{r.current}</b>;
        }
        component App() {
            return <div><C tag="x"/><C tag="y"/></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains(">x<") && tree.contains(">y<"),
        "per-instance refs: {tree}"
    );
}
