//! M1-T11 acceptance: useId — stable unique ids.
//!
//! React semantics under test:
//! 1. useId() returns the SAME id across re-renders of the same instance.
//! 2. Two component instances get DIFFERENT ids.
//! 3. Two useId calls in one component differ (each call site unique).
//! 4. The id is a string.

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

fn all_text(r: &MemoryRenderer) -> Vec<String> {
    r.nodes()
        .values()
        .filter_map(|n| match n {
            r2n_renderer_memory::MemNode::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
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
fn useid_stable_across_renders() {
    let src = r#"
        component App() {
            let id = useId();
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            return <div><p className="id">{id}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let before = all_text(&r);
    let id_before = before
        .iter()
        .find(|t| t.starts_with(':'))
        .expect("id")
        .clone();
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let after = all_text(&r);
    let id_after = after
        .iter()
        .find(|t| t.starts_with(':'))
        .expect("id")
        .clone();
    assert_eq!(
        id_before, id_after,
        "useId must be stable across re-renders: {before:?} -> {after:?}"
    );
}

#[test]
fn useid_unique_per_instance() {
    let src = r#"
        component Row() {
            let id = useId();
            return <p className="row">{id}</p>;
        }
        component App() {
            return <div><Row/><Row/></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let ids: Vec<String> = all_text(&r)
        .into_iter()
        .filter(|t| t.starts_with(':'))
        .collect();
    assert_eq!(ids.len(), 2, "two ids rendered: {ids:?}");
    assert_ne!(ids[0], ids[1], "ids must differ per instance: {ids:?}");
}

#[test]
fn useid_call_sites_differ_within_component() {
    let src = r#"
        component App() {
            let a = useId();
            let b = useId();
            return <div><p className="a">{a}</p><p className="b">{b}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let ids: Vec<String> = all_text(&r)
        .into_iter()
        .filter(|t| t.starts_with(':'))
        .collect();
    assert_eq!(ids.len(), 2, "two call sites: {ids:?}");
    assert_ne!(ids[0], ids[1], "call sites differ: {ids:?}");
}
