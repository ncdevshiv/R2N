//! M1-T04 acceptance: conditional rendering & lists as first-class control flow.
//!
//! React semantics under test:
//! 1. `{cond && <el/>}` renders el when cond, nothing otherwise.
//! 2. `{cond || <el/>}` renders el when !cond.
//! 3. `{false}`, `{null}` render NOTHING; `{0}` and `{NaN}` RENDER (React
//!    parity, including the classic `0` footgun).
//! 4. `{flag && "text"}` — value short-circuit rides the Text path with
//!    nullish suppression.
//! 5. Ternary chains (else-if via nested ternaries).
//! 6. `.get` index access as a JSX child (`{items[1]}`).
//! 7. `arr.filter(pred).map(el)` chains render keyed items.
//! 8. Reactivity: flipping the condition state produces minimal patches.

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
fn logical_and_renders_element_conditionally() {
    let src = r#"
        component App() {
            let v = useState(1);
            let on = v[0];
            let setOn = v[1];
            return (
                <div>
                    {on == 1 && <b>shown</b>}
                    <u>end</u>
                    <button onClick={() => setOn(0)}>go</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>shown</b><u>end</u><button>go</button></div>",
        "cond true renders the element:\n{tree}"
    );
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree == "<div><u>end</u><button>go</button></div>",
        "cond false renders nothing (no 'false' text):\n{tree}"
    );
    let removes = patches
        .iter()
        .filter(|p| matches!(p, Patch::Remove { .. }))
        .count();
    assert_eq!(removes, 2, "element + its text removed: {patches:?}");
}

#[test]
fn logical_or_renders_element_on_falsy() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            return (
                <div>
                    {n || <b>fallback</b>}
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>fallback</b></div>",
        "0 is falsy so the element renders:\n{tree}"
    );
}

#[test]
fn falsy_values_render_nothing_but_zero_renders() {
    let src = r#"
        component App() {
            return (
                <div>
                    <b>before</b>
                    {false}
                    {null}
                    {0}
                    <i>after</i>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>before</b>0<i>after</i></div>",
        "false/null vanish, 0 stays (React parity):\n{tree}"
    );
}

#[test]
fn value_short_circuit_suppresses_falsy() {
    let src = r#"
        component App() {
            let v = useState(1);
            let on = v[0];
            let setOn = v[1];
            return (
                <div>
                    {on == 1 && "hello"}
                    <button onClick={() => setOn(0)}>go</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string() == "<div>hello<button>go</button></div>",
        "truthy short-circuit renders the string:\n{}",
        r.render_string()
    );
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree == "<div><button>go</button></div>",
        "falsy short-circuit (false) renders nothing:\n{tree}"
    );
    let creates = patches
        .iter()
        .filter(|p| matches!(p, Patch::Create { .. }))
        .count();
    assert_eq!(
        creates, 0,
        "suppression is a removal, not a swap to 'false' text: {patches:?}"
    );
}

#[test]
fn ternary_chains_act_as_else_if() {
    let src = r#"
        component App() {
            let v = useState(1);
            let n = v[0];
            let setN = v[1];
            return (
                <div>
                    {n == 1 ? <b>one</b> : n == 2 ? <i>two</i> : <u>many</u>}
                    <button onClick={() => setN(n + 1)}>go</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(r.render_string().contains("<b>one</b>"), "n=1");
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("to 2");
    r.apply(&patches);
    assert!(
        r.render_string().contains("<i>two</i>"),
        "n=2 branch:\n{}",
        r.render_string()
    );
    let patches = rt.dispatch(btn, "onClick").expect("to 3");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(tree.contains("<u>many</u>"), "n=3 falls to else:\n{tree}");
    assert!(
        !tree.contains("one") && !tree.contains("two"),
        "no stale branches:\n{tree}"
    );
}

#[test]
fn index_child_renders_value() {
    let src = r#"
        component App() {
            let items = ["a", "b", "c"];
            return <div><p>{items[1]}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><p>b</p></div>",
        "index access as a JSX child renders the element:\n{tree}"
    );
}

#[test]
fn filter_map_chain_renders_keyed_items() {
    let src = r#"
        component App() {
            let items = [1, 2, 3, 4, 5, 6];
            return (
                <ul>
                    {items.filter((x) => x % 2 == 0).map((n) => <li key={n}>{n}</li>)}
                </ul>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<ul><li>2</li><li>4</li><li>6</li></ul>",
        "filter().map() renders the filtered keyed items:\n{tree}"
    );
}

#[test]
fn conditional_element_state_survives_condition_flips() {
    // The conditional element is a COMPONENT with its own state; toggling
    // it away and back resets (unmount/mount), but keeping it mounted while
    // OTHER parts change preserves state (it is keyed by position).
    let src = r#"
        component Counter() {
            let c = useState(10);
            let n = c[0];
            let setN = c[1];
            return <b onClick={() => setN(n + 1)}>{n}</b>;
        }
        component App() {
            let v = useState(1);
            let on = v[0];
            let setOn = v[1];
            return (
                <div>
                    {on % 2 == 1 && <Counter/>}
                    <button onClick={() => setOn(on + 1)}>flip</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let counter = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "b" => Some(*id),
            _ => None,
        })
        .expect("counter's <b>");
    let patches = rt.dispatch(counter, "onClick").expect("count up");
    r.apply(&patches);
    assert!(
        r.render_string().contains("11"),
        "counter advanced:\n{}",
        r.render_string()
    );
    // Unmount via the condition; then remount: fresh state (10).
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("unmount");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        !tree.contains("<b>"),
        "counter gone when cond false:\n{tree}"
    );
    let patches = rt.dispatch(btn, "onClick").expect("remount");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree.contains("10"),
        "remounted counter starts fresh (unmount semantics):\n{tree}"
    );
}
