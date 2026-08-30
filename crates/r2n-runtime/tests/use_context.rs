//! M1-T10 acceptance: useContext — Context / Provider / Consumer.
//!
//! React semantics under test:
//! 1. `<Ctx.Provider value={v}>` makes `useContext(Ctx)` return `v` for
//!    everything below it in the tree, regardless of component depth.
//! 2. Without a provider, useContext falls back to its default argument.
//! 3. Nested providers shadow: innermost value wins.
//! 4. Value changes propagate: a provider whose value comes from state
//!    re-renders its consumers when the state changes.
//! 5. Contexts are independent (two different handles don't interfere).

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
fn provider_value_reaches_deep_descendant() {
    let src = r#"
        component App() {
            let Ctx = createContext("default");
            return (
                <Ctx.Provider value="theme-dark">
                    <div className="card">
                        <p className="inner">{useContext(Ctx)}</p>
                    </div>
                </Ctx.Provider>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("theme-dark"),
        "useContext in a descendant sees the provider value:\n{tree}"
    );
}

#[test]
fn context_default_when_no_provider() {
    let src = r#"
        component App() {
            let Ctx = createContext("fallback");
            return <div className="plain">{useContext(Ctx)}</div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("fallback"),
        "no provider -> default arg:\n{tree}"
    );
}

#[test]
fn nested_providers_shadow() {
    let src = r#"
        component App() {
            let Ctx = createContext("base");
            return (
                <Ctx.Provider value="outer">
                    <div>
                        <p className="a">{useContext(Ctx)}</p>
                        <Ctx.Provider value="inner">
                            <p className="b">{useContext(Ctx)}</p>
                        </Ctx.Provider>
                    </div>
                </Ctx.Provider>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("outer") && tree.contains("inner"),
        "inner shadows outer; both visible in their scopes:\n{tree}"
    );
    // Order: outer's <p> (className a) renders "outer", inner's (b) renders
    // "inner" — b's text must come after a's in document order.
    let outer_at = tree.find("outer").expect("outer");
    let inner_at = tree.find("inner").expect("inner");
    assert!(outer_at < inner_at, "document order outer -> inner: {tree}");
}

#[test]
fn context_value_changes_propagate_to_consumers() {
    let src = r#"
        component App() {
            let v = useState("morning");
            let mode = v[0];
            let setMode = v[1];
            let Ctx = createContext("seed");
            return (
                <Ctx.Provider value={mode}>
                    <div>
                        <p className="out">{useContext(Ctx)}</p>
                        <button onClick={() => setMode("night")}>go</button>
                    </div>
                </Ctx.Provider>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string().contains("morning"),
        "initial:\n{}",
        r.render_string()
    );
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree.contains("night"),
        "provider value change reaches the consumer:\n{tree}"
    );
    // Minimal patch: a SetText on the consumer's text node.
    let set_text = patches
        .iter()
        .any(|p| matches!(p, r2n_runtime::Patch::SetText { .. }));
    assert!(set_text, "value change is a SetText: {patches:?}");
}

#[test]
fn providers_in_components_propagate_to_siblings_siblings() {
    let src = r#"
        component Wrap(value) {
            let Ctx = createContext("local");
            return (
                <Ctx.Provider value={value}>
                    <div className="wrap">{useContext(Ctx)}</div>
                </Ctx.Provider>
            );
        }
        component App() {
            return (
                <main>
                    <Wrap value="first"/>
                    <Wrap value="second"/>
                </main>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("first") && tree.contains("second"),
        "per-instance provider isolation:\n{tree}"
    );
    assert!(
        tree.find("first").unwrap() < tree.find("second").unwrap(),
        "document order preserved:\n{tree}"
    );
}

#[test]
fn two_contexts_are_independent() {
    let src = r#"
        component App() {
            let A = createContext("a-default");
            let B = createContext("b-default");
            return (
                <A.Provider value="A">
                    <B.Provider value="B">
                        <div className="both">
                            <p>{useContext(A)}</p>
                            <p>{useContext(B)}</p>
                        </div>
                    </B.Provider>
                </A.Provider>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("A") && tree.contains("B"),
        "each handle resolves its own provider value:\n{tree}"
    );
}
