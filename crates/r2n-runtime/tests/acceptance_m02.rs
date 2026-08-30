//! M0.2 acceptance criteria — the formal 14-test suite (roadmap task
//! M0.2-T13). A milestone closes only when these pass; they encode the
//! observable behaviors the roadmap's exit criteria name:
//! mount/unmount, batching, identity stability, keyed reconciliation,
//! minimal patches, independent instance state, effects lifecycle,
//! and conditional/fragment correctness.

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

fn find_tag(r: &MemoryRenderer, tag: &str, text: Option<&str>) -> Option<NodeId> {
    r.nodes().iter().find_map(|(id, n)| match n {
        MemNode::Element { tag: t, .. } if t == tag => {
            let kids = r.children_of().get(&Some(*id)).cloned().unwrap_or_default();
            let ok = match text {
                None => true,
                Some(want) => kids.iter().any(
                    |k| matches!(r.nodes().get(k), Some(MemNode::Text { text }) if text == want),
                ),
            };
            if ok {
                Some(*id)
            } else {
                None
            }
        }
        _ => None,
    })
}

// 1. Mount: initial render produces the full tree via Create patches.
#[test]
fn ac01_mount_creates_full_tree() {
    let (rt, r) = setup(
        r#"
        component App() { return <div><p>{"hello"}</p></div>; }
        export default App;
    "#,
    );
    assert_eq!(r.render_string(), r#"<div><p>hello</p></div>"#);
    // The renderer has the nodes; the runtime produced them from a patch
    // stream (Create + CreateText ops).
    assert!(r.nodes().len() >= 2);
    let _ = rt;
}

// 2. Unmount: removing an element emits Remove and the node disappears.
#[test]
fn ac02_unmount_removes_subtree() {
    let (mut rt2, mut r2) = setup(
        r#"
        component App() {
            let mode = useState(1);
            let m = mode[0];
            let setM = mode[1];
            return (
                <div>
                    {if m == 1 { <p>{"kept"}</p> } else { <span>{"gone"}</span> }}
                    <button onClick={() => setM(0)}>flip</button>
                </div>
            );
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r2, "button", Some("flip")).expect("button");
    let ps = rt2.dispatch(btn, "onClick").expect("dispatch");
    r2.apply(&ps);
    // The <p> was unmounted and replaced by the <span> branch.
    assert!(!r2.render_string().contains("kept"));
    assert!(r2.render_string().contains("gone"));
    assert!(
        ps.iter().any(|p| matches!(p, Patch::Remove { .. })),
        "unmount must emit Remove: {ps:?}"
    );
}

// 3. State update: a setter re-renders the owning component only.
#[test]
fn ac03_state_update_rerenders_owner() {
    let (mut rt, mut r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <div><h1>{n}</h1><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("go")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        r#"<div><h1>1</h1><button>go</button></div>"#
    );
    // Minimal: exactly one patch, a SetText — nothing else re-created.
    assert_eq!(ps.len(), 1);
    assert!(matches!(&ps[0], Patch::SetText { text, .. } if text == "1"));
}

// 4. Batching: multiple setter calls in one handler = one render pass.
#[test]
fn ac04_batched_setters_single_pass() {
    let (mut rt, mut r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <div><h1>{n}</h1><button onClick={() => { setN(7); setN(9); }}>b</button></div>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("b")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        r#"<div><h1>9</h1><button>b</button></div>"#
    );
    assert_eq!(ps.len(), 1, "one batched pass only: {ps:?}");
}

// 5. Identity stability: unchanged siblings keep their node ids across renders.
#[test]
fn ac05_identity_stability_across_renders() {
    let (mut rt2, mut r2) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <div><a>{"a"}</a><b>{n}</b><c>{"c"}</c><button onClick={() => setN(1)}>x</button></div>;
        }
        export default App;
    "#,
    );
    let b2 = find_tag(&r2, "button", Some("x")).expect("button");
    let ids_before: Vec<NodeId> = r2.nodes().keys().copied().collect();
    let ps = rt2.dispatch(b2, "onClick").expect("dispatch");
    r2.apply(&ps);
    let ids_after: Vec<NodeId> = r2.nodes().keys().copied().collect();
    // Same node set (the <b> text changed, nothing was created/removed).
    assert_eq!(ids_before.len(), ids_after.len());
    for id in &ids_before {
        assert!(
            ids_after.contains(id),
            "node {id} must survive the re-render"
        );
    }
}

// 6. Keyed lists: same keys reorder without remove+recreate.
#[test]
fn ac06_keyed_reorder_uses_moves_not_recreates() {
    let (mut rt, mut r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let m = s[0];
            let setM = s[1];
            let items = if m == 0 { ["a", "b", "c"] } else { ["c", "b", "a"] };
            return <ul>
                {items.map((x) => <li key={x}>{x}</li>)}
                <button onClick={() => setM(1)}>rev</button>
            </ul>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("rev")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        "<ul><li>c</li><li>b</li><li>a</li><button>rev</button></ul>"
    );
    assert!(
        ps.iter().any(|p| matches!(p, Patch::Move { .. })),
        "reorder must use Move: {ps:?}"
    );
    assert!(
        !ps.iter().any(|p| matches!(p, Patch::Remove { .. })),
        "reorder must not remove nodes: {ps:?}"
    );
}

// 7. Keyed append: existing items untouched, new item created once.
#[test]
fn ac07_keyed_append_is_minimal() {
    let (mut rt, mut r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let m = s[0];
            let setM = s[1];
            let items = if m == 0 { ["a", "b"] } else { ["a", "b", "c"] };
            return <ul>
                {items.map((x) => <li key={x}>{x}</li>)}
                <button onClick={() => setM(1)}>add</button>
            </ul>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("add")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        "<ul><li>a</li><li>b</li><li>c</li><button>add</button></ul>"
    );
    let creates = ps
        .iter()
        .filter(|p| matches!(p, Patch::Create { tag, .. } if tag == "li"))
        .count();
    assert_eq!(creates, 1, "exactly one new <li>: {ps:?}");
    assert!(!ps.iter().any(|p| matches!(p, Patch::Remove { .. })));
}

// 8. Keyed removal: dropping a middle item removes only it.
#[test]
fn ac08_keyed_removal_only_removes_dropped() {
    let (mut rt, mut r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let m = s[0];
            let setM = s[1];
            let items = if m == 0 { ["a", "b", "c"] } else { ["a", "c"] };
            return <ul>
                {items.map((x) => <li key={x}>{x}</li>)}
                <button onClick={() => setM(1)}>drop</button>
            </ul>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("drop")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        "<ul><li>a</li><li>c</li><button>drop</button></ul>"
    );
    // Removing a keyed item removes the <li> AND its text child (subtree
    // removal) — but exactly one <li> element, and nothing else.
    let removed_ids: Vec<NodeId> = ps
        .iter()
        .filter_map(|p| match p {
            Patch::Remove { id } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(removed_ids.len(), 2, "<li> + its text: {ps:?}");
    for id in &removed_ids {
        // Every removed node must be gone from the live tree.
        assert!(r.nodes().get(id).is_none(), "removed node {id} still live");
    }
    // 'b' text must be gone from the rendered output.
    assert!(!r.render_string().contains("<li>b</li>"));
}

// 9. Two instances of the same component hold independent state.
#[test]
fn ac09_instances_independent() {
    let (mut rt, mut r) = setup(
        r#"
        component Counter() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <button onClick={() => setN(n + 1)}>{n}</button>;
        }
        component App() { return <div><Counter/><Counter/></div>; }
        export default App;
    "#,
    );
    let btns: Vec<NodeId> = r
        .nodes()
        .iter()
        .filter(|(_, n)| matches!(n, MemNode::Element { tag, .. } if tag == "button"))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(btns.len(), 2);
    let ps = rt.dispatch(btns[0], "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        "<div><button>1</button><button>0</button></div>"
    );
}

// 10. Props flow: parent state changes propagate into child renders.
#[test]
fn ac10_props_propagate() {
    let (mut rt, mut r) = setup(
        r#"
        component Badge(label) {
            return <span>{label}</span>;
        }
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <div><Badge label={n}/><button onClick={() => setN(n + 1)}>p</button></div>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("p")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(
        r.render_string(),
        r#"<div><span>1</span><button>p</button></div>"#
    );
}

// 11. useEffect runs on mount; with deps, runs only on change.
#[test]
fn ac11_effect_mount_and_deps_lifecycle() {
    let (mut rt, r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            useEffect(() => { console.log("render"); }, [n]);
            return <button onClick={() => setN(n + 1)}>{n}</button>;
        }
        export default App;
    "#,
    );
    assert_eq!(rt.logs(), ["render"]);
    let btn = find_tag(&r, "button", Some("0")).expect("button");
    let _ = rt.dispatch(btn, "onClick").expect("dispatch");
    assert_eq!(
        rt.logs(),
        ["render", "render"],
        "deps changed -> effect re-ran"
    );
    // No-op click (same value) must NOT re-run the effect.
    let (mut rt2, r2) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            useEffect(() => { console.log("render"); }, [n]);
            return <button onClick={() => setN(n)}>{n}</button>;
        }
        export default App;
    "#,
    );
    let b2 = find_tag(&r2, "button", Some("0")).expect("button");
    let _ = rt2.dispatch(b2, "onClick").expect("dispatch");
    assert_eq!(rt2.logs(), ["render"], "same deps -> no re-run");
}

// 12. Conditional render swaps branches with minimal patches.
#[test]
fn ac12_conditional_swap_minimal() {
    let (mut rt, mut r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <div>{if n == 0 { "off" } else { "on" }}<button onClick={() => setN(1)}>t</button></div>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("t")).expect("button");
    let ps = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&ps);
    assert_eq!(r.render_string(), r#"<div>on<button>t</button></div>"#);
    // The branch swap is one SetText (same text-slot path) or remove+create;
    // either way it must be small and not touch the button.
    assert!(ps.len() <= 3, "branch swap must be minimal: {ps:?}");
    assert!(!ps
        .iter()
        .any(|p| matches!(p, Patch::Create { tag, .. } if tag == "button")));
}

// 13. Event dispatch on an unknown event errors deterministically (no panic).
#[test]
fn ac13_unknown_event_is_clean_error() {
    let (mut rt, r) = setup(
        r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            let setN = s[1];
            return <button onClick={() => setN(n + 1)}>x</button>;
        }
        export default App;
    "#,
    );
    let btn = find_tag(&r, "button", Some("x")).expect("button");
    let err = rt.dispatch(btn, "onDrag").unwrap_err();
    assert!(err.to_string().contains("no 'onDrag' handler"));
}

// 14. Full loop determinism: same event sequence → identical patch streams.
#[test]
fn ac14_patch_stream_deterministic_across_runs() {
    let run = || {
        let (mut rt, mut r) = setup(
            r#"
            component App() {
                let s = useState(0);
                let n = s[0];
                let setN = s[1];
                return <div><h1>{n}</h1><ul>{["a","b","c"].map((x) => <li key={x}>{x}</li>)}</ul><button onClick={() => setN(n + 1)}>go</button></div>;
            }
            export default App;
        "#,
        );
        let btn = find_tag(&r, "button", Some("go")).expect("button");
        let mut streams = Vec::new();
        for _ in 0..3 {
            let ps = rt.dispatch(btn, "onClick").expect("dispatch");
            r.apply(&ps);
            streams.push(format!("{ps:?}"));
        }
        streams
    };
    assert_eq!(
        run(),
        run(),
        "identical event sequence must produce identical patch streams"
    );
}
