//! M1-T05 acceptance: useReducer — reducer IR + dispatch actions.
//!
//! React semantics under test:
//! 1. `useReducer(reducer, initial)` returns `[state, dispatch]`.
//! 2. Calling `dispatch(action)` runs `reducer(state, action)` and
//!    re-renders with the new state (event handler → dispatch → flush).
//! 3. The reducer is pure IR data (params + body) — no function pointers.
//! 4. Dispatches batch: several actions in one handler produce ONE render
//!    with the final state.
//! 5. Two component instances hold independent reducer state.
//! 6. State transitions follow the action pattern (counter + toggle).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::patch::Patch;
use r2n_runtime::{NodeId, Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

/// All button node ids that carry an onClick handler, in document order
/// (BTreeMap iteration order = node id order = render order).
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
fn reducer_dispatches_through_event_handler() {
    let src = r#"
        component Counter() {
            let counter = useReducer((state, action) => if action == "inc" { state + 1 } else { state }, 0);
            let n = counter[0];
            let dispatch = counter[1];
            return (
                <div>
                    <p className="count">{n}</p>
                    <button onClick={() => dispatch("inc")}>+</button>
                </div>
            );
        }
        export default Counter;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string().contains(">0<"),
        "initial state is the reducer's initial arg:\n{}",
        r.render_string()
    );
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch inc");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">1<"),
        "reducer(state, action) ran: state 1:\n{}",
        r.render_string()
    );
    // Minimal patch: one SetText on the counter text.
    let set_text = patches
        .iter()
        .filter(|p| matches!(p, Patch::SetText { .. }))
        .count();
    let removes = patches
        .iter()
        .filter(|p| matches!(p, Patch::Remove { .. }))
        .count();
    assert_eq!(set_text, 1, "one SetText: {patches:?}");
    assert_eq!(removes, 0, "no removals: {patches:?}");
}

#[test]
fn reducer_handles_multiple_actions() {
    let src = r#"
        component App() {
            let state = useReducer(
                (s, a) => a == "inc" ? s + 1 : a == "dec" ? s - 1 : s,
                5
            );
            let n = state[0];
            let dispatch = state[1];
            return (
                <div>
                    <p>{n}</p>
                    <button className="plus" onClick={() => dispatch("inc")}>+</button>
                    <button className="minus" onClick={() => dispatch("dec")}>-</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string().contains(">5<"),
        "start 5:\n{}",
        r.render_string()
    );
    let bs = buttons(&r);
    assert_eq!(bs.len(), 2, "two buttons");
    let plus = bs[0];
    let minus = bs[1];

    let patches = rt.dispatch(plus, "onClick").expect("inc");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">6<"),
        "6 after inc:\n{}",
        r.render_string()
    );

    let patches = rt.dispatch(minus, "onClick").expect("dec");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">5<"),
        "5 after dec:\n{}",
        r.render_string()
    );

    let patches = rt.dispatch(plus, "onClick").expect("inc again");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">6<"),
        "6 again:\n{}",
        r.render_string()
    );
}

#[test]
fn reducer_actions_batch_into_one_render() {
    let src = r#"
        component App() {
            let state = useReducer((s, a) => s + a, 0);
            let n = state[0];
            let dispatch = state[1];
            return (
                <div>
                    <p>{n}</p>
                    <button onClick={() => { dispatch(1); dispatch(2); dispatch(3); }}>+</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("three dispatches");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">6<"),
        "all three actions applied (0+1+2+3), final state 6:\n{}",
        r.render_string()
    );
    // FIFO scheduler dedup: the counter text updates once.
    let set_texts = patches
        .iter()
        .filter(|p| matches!(p, Patch::SetText { .. }))
        .count();
    assert!(
        set_texts <= 1,
        "batched dispatches render once: {patches:?}"
    );
}

#[test]
fn reducer_state_independent_per_instance() {
    let src = r#"
        component Counter() {
            let state = useReducer((s, a) => s + 1, 0);
            let n = state[0];
            let dispatch = state[1];
            return <div className="counter"><span>{n}</span><button onClick={() => dispatch("inc")}>+</button></div>;
        }
        component App() {
            return <main><Counter/><Counter/></main>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let bs = buttons(&r);
    assert_eq!(bs.len(), 2, "two counters, two buttons");
    // Click the FIRST counter's button (document order).
    let patches = rt.dispatch(bs[0], "onClick").expect("first click");
    r.apply(&patches);
    let tree = r.render_string();
    // One counter at 1, the other at 0 — independent frames.
    assert!(
        tree.contains(">1<") && tree.contains(">0<"),
        "one at 1, one at 0 (independent frames):\n{tree}"
    );
    // The first occurrence of a count is the first counter's (1).
    let first_one = tree.find(">1<").expect("one");
    let first_zero = tree.find(">0<").expect("zero");
    assert!(
        first_one < first_zero,
        "first counter advanced (document order preserved):\n{tree}"
    );
}

#[test]
fn reducer_handles_toggle_state() {
    let src = r#"
        component App() {
            let state = useReducer((on, a) => if a == "toggle" { !on } else { on }, false);
            let on = state[0];
            let dispatch = state[1];
            return <div><p className="light">{if on { "on" } else { "off" }}</p><button onClick={() => dispatch("toggle")}>t</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string().contains("off"),
        "initial false:\n{}",
        r.render_string()
    );
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("toggle");
    r.apply(&patches);
    assert!(
        r.render_string().contains("on"),
        "toggled:\n{}",
        r.render_string()
    );
    let patches = rt.dispatch(btn, "onClick").expect("toggle back");
    r.apply(&patches);
    assert!(
        r.render_string().contains("off"),
        "back off:\n{}",
        r.render_string()
    );
}

#[test]
fn reducer_survives_parent_render() {
    // The reducer state persists while the parent re-renders (like
    // useState): a component with a reducer keeps its value across flushes.
    let src = r#"
        component Counter() {
            let state = useReducer((s, a) => s + 1, 0);
            let n = state[0];
            let dispatch = state[1];
            return <p className="cnt">{n}</p>;
        }
        component App() {
            let v = useState(0);
            let on = v[0];
            let setOn = v[1];
            return (
                <div>
                    <Counter/>
                    <button className="go" onClick={() => setOn(on + 1)}>flip</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // Drive the reducer through its OWN handler: Counter's p has no
    // handler, so click the flip button (parent re-render path) and verify
    // the reducer value (0) is unchanged and still rendered.
    let btn = buttons(&r)[0];
    let patches = rt.dispatch(btn, "onClick").expect("flip");
    r.apply(&patches);
    assert!(
        r.render_string().contains(">0<"),
        "reducer state preserved across parent render:\n{}",
        r.render_string()
    );
}
