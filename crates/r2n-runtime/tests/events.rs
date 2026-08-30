//! The M0.2/M0.3 exit tests: the full reactive loop driven by *events* —
//! `onClick={() => setN(n + 1)}` → dispatch → setter → dirty → flush →
//! minimal patch, with no parent recreation. Also: two independent component
//! instances, `useEffect` (mount + deps), and the Todo E2E.

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

/// Find the node id of the first element with `tag` (optionally whose text
/// child equals `text`).
fn find_node(r: &MemoryRenderer, tag: &str, text: Option<&str>) -> Option<NodeId> {
    r.nodes().iter().find_map(|(id, n)| match n {
        MemNode::Element { tag: t, .. } if t == tag => {
            let kids = r.children_of().get(&Some(*id)).cloned().unwrap_or_default();
            let matches_text = match text {
                None => true,
                Some(want) => kids.iter().any(|k| match r.nodes().get(k) {
                    Some(MemNode::Text { text }) => text == want,
                    _ => false,
                }),
            };
            if matches_text {
                Some(*id)
            } else {
                None
            }
        }
        _ => None,
    })
}

const COUNTER: &str = r#"
    component Counter() {
        let count = useState(0);
        let n = count[0];
        let setN = count[1];
        return (
            <div className="counter">
                <h1>{n}</h1>
                <button onClick={() => setN(n + 1)}>+1</button>
            </div>
        );
    }
    export default Counter;
"#;

#[test]
fn click_counter_emits_single_settext() {
    let (mut rt, mut r) = setup(COUNTER);
    assert_eq!(
        r.render_string(),
        r#"<div className="counter"><h1>0</h1><button>+1</button></div>"#
    );
    let btn = find_node(&r, "button", None).expect("button node");
    // The click handler must produce exactly ONE SetText patch — the h1's
    // text changes; the div, h1, and button are NOT recreated.
    for step in 1..=3 {
        let patches = rt.dispatch(btn, "onClick").expect("dispatch");
        assert_eq!(patches.len(), 1, "step {step}: {patches:?}");
        assert!(
            matches!(patches[0], Patch::SetText { .. }),
            "step {step}: {patches:?}"
        );
        r.apply(&patches);
        assert_eq!(
            r.render_string(),
            format!(r#"<div className="counter"><h1>{step}</h1><button>+1</button></div>"#)
        );
    }
}

#[test]
fn dispatch_unknown_event_is_an_error() {
    let (mut rt, _) = setup(COUNTER);
    let btn = NodeId(2); // may not be the button; must still error cleanly
    assert!(rt.dispatch(btn, "onHover").is_err());
}

#[test]
fn two_instances_hold_independent_state() {
    // Two Counter children of App: each has its own useState frame, so
    // clicking one must not change the other.
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            return (
                <div className="counter">
                    <h1>{n}</h1>
                    <button onClick={() => setN(n + 1)}>+1</button>
                </div>
            );
        }
        component App() {
            return <div><Counter/><Counter/></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert_eq!(
        r.render_string(),
        r#"<div><div className="counter"><h1>0</h1><button>+1</button></div><div className="counter"><h1>0</h1><button>+1</button></div></div>"#
    );
    // Click the FIRST counter's button twice.
    let buttons: Vec<NodeId> = r
        .nodes()
        .iter()
        .filter(|(_, n)| matches!(n, MemNode::Element { tag, .. } if tag == "button"))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(buttons.len(), 2);
    for _ in 0..2 {
        let ps = rt.dispatch(buttons[0], "onClick").expect("dispatch");
        r.apply(&ps);
    }
    assert_eq!(
        r.render_string(),
        r#"<div><div className="counter"><h1>2</h1><button>+1</button></div><div className="counter"><h1>0</h1><button>+1</button></div></div>"#,
        "first counter advanced, second untouched"
    );
}

#[test]
fn use_effect_runs_on_mount_and_on_deps_change() {
    // Effects run after commit. With no deps they run every render; with
    // deps they run only when the deps change. We observe via console.log.
    let src = r#"
        component App() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            useEffect(() => { console.log("render"); }, [n]);
            return <button onClick={() => setN(n + 1)}>{n}</button>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert_eq!(rt.logs(), ["render"], "mount effect ran exactly once");
    let btn = find_node(&r, "button", Some("0")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        rt.logs(),
        ["render", "render"],
        "effect re-ran because deps [n] changed"
    );
    // A second click changes n again -> effect runs again.
    let btn = find_node(&r, "button", Some("1")).expect("button after first click");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(rt.logs(), ["render", "render", "render"]);
    // No-op render with same deps must NOT re-run: dispatch on a node whose
    // handler sets n to its current value.
    let src_noop = r#"
        component App() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            useEffect(() => { console.log("render"); }, [n]);
            return <button onClick={() => setN(n)}>{n}</button>;
        }
        export default App;
    "#;
    let (mut rt2, r2) = setup(src_noop);
    let btn = find_node(&r2, "button", Some("0")).expect("button");
    let _ = rt2.dispatch(btn, "onClick").expect("dispatch");
    let _ = &r2;
    assert_eq!(rt2.logs(), ["render"], "same deps -> effect not re-run");
}

#[test]
fn todo_app_e2e_through_full_reactive_loop() {
    // The M0.2 stretch goal: add an item via a click handler; the keyed list
    // must reconcile to the new state with minimal patches.
    let src = r#"
        component TodoApp() {
            let items = ["a", "b"];
            let next = ["a", "b", "c"];
            let state = useState(0);
            let mode = state[0];
            let setMode = state[1];
            let list = if mode == 0 { items } else { next };
            return (
                <div>
                    <button onClick={() => setMode(1)}>add c</button>
                    <ul>{list.map((x) => <li key={x}>{x}</li>)}</ul>
                </div>
            );
        }
        export default TodoApp;
    "#;
    let (mut rt, mut r) = setup(src);
    assert_eq!(
        r.render_string(),
        r#"<div><button>add c</button><ul><li>a</li><li>b</li></ul></div>"#
    );
    let btn = find_node(&r, "button", Some("add c")).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    assert_eq!(
        r.render_string(),
        r#"<div><button>add c</button><ul><li>a</li><li>b</li><li>c</li></ul></div>"#
    );
    // The `key` prop must never surface on rendered nodes.
    assert!(!r.render_string().contains("key=\"c\""));
    // Minimal: existing a/b nodes are untouched; only <li>c</li> is created.
    assert!(patches
        .iter()
        .any(|p| matches!(p, Patch::Create { tag, .. } if tag == "li")));
    assert!(
        !patches.iter().any(|p| matches!(p, Patch::Remove { .. })),
        "no removals expected when appending an item"
    );
}
