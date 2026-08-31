//! M1-T15 acceptance: Suspense — Active → Suspended → Resolved.
//!
//! React semantics under test:
//! 1. A component reading a `useResource` Pending value suspends; the
//!    nearest `<Suspense fallback>` shows the fallback (Suspended).
//! 2. Resolving the resource (real state flip) renders the actual content
//!    (Resolved) with MINIMAL patches (no duplicate trees).
//! 3. After resolution, the fallback is gone.
//! 4. Suspense is per-instance: two Suspense boundaries resolve
//!    independently.
//! 5. Unresolved inside a huge tree: only the suspense subtree falls back;
//!    siblings render normally.

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

fn buttons(r: &MemoryRenderer) -> Vec<NodeId> {
    r.nodes()
        .iter()
        .filter_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, props } if tag == "button" => {
                let clickable = props.iter().any(|(k, v)| {
                    k == "onClick" && matches!(v, r2n_runtime::Value::Handler { .. })
                });
                clickable.then_some(*id)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn suspense_shows_fallback_then_resolves() {
    let src = r#"
        component App() {
            let res = useResource(0);
            let value = res[0];
            let resolve = res[1];
            return (
                <div>
                    <Suspense fallback={<p className="load">loading</p>}>
                        <p className="data">{value}</p>
                    </Suspense>
                    <button onClick={() => resolve(42)}>resolve</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // Suspended: fallback visible, data not.
    let tree = r.render_string();
    assert!(tree.contains("loading"), "fallback shown: {tree}");
    assert!(!tree.contains("data"), "data not yet: {tree}");
    let btn = buttons(&r)[0];
    let patches = rt.dispatch(btn, "onClick").expect("resolve");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(tree.contains(">42<"), "resolved content rendered: {tree}");
    assert!(!tree.contains("loading"), "fallback gone: {tree}");
    // Minimal patches: no Remove/Create churn for the swap.
    let removes = patches
        .iter()
        .filter(|p| matches!(p, r2n_runtime::Patch::Remove { .. }))
        .count();
    let creates = patches
        .iter()
        .filter(|p| matches!(p, r2n_runtime::Patch::Create { .. }))
        .count();
    assert_eq!(removes, 0, "no removals: {patches:?}");
    assert_eq!(creates, 0, "no creates: {patches:?}");
}

#[test]
fn siblings_render_while_suspended() {
    let src = r#"
        component App() {
            let res = useResource(0);
            let value = res[0];
            return (
                <div>
                    <h1 className="head">title</h1>
                    <Suspense fallback={<p className="load">wait</p>}>
                        <p className="data">{value}</p>
                    </Suspense>
                    <footer>foot</footer>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(tree.contains("title"), "sibling above: {tree}");
    assert!(tree.contains("foot"), "sibling below: {tree}");
    assert!(tree.contains("wait"), "fallback: {tree}");
}

#[test]
fn two_suspense_boundaries_independent() {
    let src = r#"
        component Widget(label) {
            let res = useResource(0);
            let value = res[0];
            let resolve = res[1];
            return (
                <div className={label}>
                    <Suspense fallback={<p className="load">{label}{"-loading"}</p>}>
                        <p className="data">{label}{"="}{value}</p>
                    </Suspense>
                    <button onClick={() => resolve(7)}>{label}{"-go"}</button>
                </div>
            );
        }
        component App() {
            return <div><Widget label="a"/><Widget label="b"/></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("a-loading") && tree.contains("b-loading"),
        "both fallbacks: {tree}"
    );
    let bs = buttons(&r);
    assert_eq!(bs.len(), 2, "two resolve buttons");
    let patches = rt.dispatch(bs[0], "onClick").expect("resolve a");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(tree.contains("a=7"), "a resolved: {tree}");
    assert!(tree.contains("b-loading"), "b still suspended: {tree}");
}

#[test]
fn unresolved_suspense_keeps_fallback_until_resolve() {
    let src = r#"
        component App() {
            let res = useResource(0);
            let value = res[0];
            // No resolve button: stays suspended across re-renders of a
            // state change elsewhere.
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            return (
                <div>
                    <Suspense fallback={<p className="load">loading</p>}>
                        <p className="data">{value}</p>
                    </Suspense>
                    <button onClick={() => setN(n + 1)}>noop</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(r.render_string().contains("loading"), "suspended initially");
    let btn = buttons(&r)[0];
    let patches = rt.dispatch(btn, "onClick").expect("state change");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree.contains("loading"),
        "stays suspended after unrelated re-render: {tree}"
    );
}
