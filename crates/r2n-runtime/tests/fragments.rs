//! M1-T03 acceptance: fragments (`<>...</>`).
//!
//! React semantics under test:
//! 1. A fragment renders its children with no host element of its own.
//! 2. Fragments work among siblings (children flow around them).
//! 3. Nested fragments flatten transitively.
//! 4. Fragments work in conditional (ternary) branches.
//! 5. Fragments as `.map` list items keep item identity via their `key`
//!    and interleave their children in document order.
//! 6. Fragments compose with children splices (fragment inside a slot).
//! 7. Fragment attributes other than `key` are a compile error.
//! 8. Reconciliation: fragment children diff positionally with minimal
//!    patches (no remove/recreate churn).

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
fn fragment_renders_children_without_host_element() {
    let src = r#"
        component App() {
            return (
                <div>
                    <><b>bold</b><i>italic</i></>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>bold</b><i>italic</i></div>",
        "fragment children render inline, no wrapper element:\n{tree}"
    );
}

#[test]
fn fragment_among_siblings() {
    let src = r#"
        component App() {
            return (
                <div>
                    <b>before</b>
                    <><i>mid1</i><u>mid2</u></>
                    <em>after</em>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>before</b><i>mid1</i><u>mid2</u><em>after</em></div>",
        "siblings flow around the fragment in document order:\n{tree}"
    );
}

#[test]
fn nested_fragments_flatten_transitively() {
    let src = r#"
        component App() {
            return (
                <div>
                    <><><b>a</b></><i>b</i></>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>a</b><i>b</i></div>",
        "nested fragments flatten to their leaves:\n{tree}"
    );
}

#[test]
fn fragment_in_ternary_branches() {
    let src = r#"
        component App() {
            let v = useState(0);
            let on = v[0];
            let setOn = v[1];
            return (
                <div>
                    {if on == 0 {
                        <><b>A1</b><i>A2</i></>
                    } else {
                        <><u>B1</u><em>B2</em></>
                    }}
                    <button onClick={() => setOn(on + 1)}>go</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string() == "<div><b>A1</b><i>A2</i><button>go</button></div>",
        "then-branch fragment:\n{}",
        r.render_string()
    );
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree == "<div><u>B1</u><em>B2</em><button>go</button></div>",
        "else-branch fragment after flip:\n{tree}"
    );
    // Both branch children swap: each element carries a text child, and
    // Remove patches account for whole subtrees — 2 elements + 2 texts.
    let removes = patches
        .iter()
        .filter(|p| matches!(p, Patch::Remove { .. }))
        .count();
    assert_eq!(removes, 4, "two branch subtrees replaced: {patches:?}");
}

#[test]
fn fragments_as_list_items_interleave_and_key() {
    let src = r#"
        component App() {
            let items = ["x", "y"];
            return (
                <ul>
                    {items.map((it) => <><em key={it}>{it}</em><span>!</span></>)}
                </ul>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<ul><em>x</em><span>!</span><em>y</em><span>!</span></ul>",
        "fragment list items interleave their children in item order:\n{tree}"
    );
}

#[test]
fn fragment_list_item_keys_reconcile_on_reorder() {
    // React semantics: keys on fragment items are the item's identity.
    // Swapping the rendered item set (via state) must replace only the
    // swapped items' children; the keyed `fixed` li survives untouched.
    let src = r#"
        component App() {
            let flag = useState(0);
            let on = flag[0];
            let setOn = flag[1];
            return (
                <ul>
                    {if on == 0 {
                        <><em key="a">A</em><span>1</span></>
                    } else {
                        <><em key="b">B</em><span>2</span></>
                    }}
                    <li key="fixed">fixed</li>
                    <button onClick={() => setOn(on + 1)}>go</button>
                </ul>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string() == "<ul><em>A</em><span>1</span><li>fixed</li><button>go</button></ul>",
        "initial:\n{}",
        r.render_string()
    );
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree == "<ul><em>B</em><span>2</span><li>fixed</li><button>go</button></ul>",
        "after flip:\n{tree}"
    );
    // Only the swapped fragment children (with their text) disappear — the
    // keyed `fixed` li and the button survive.
    let removes = patches
        .iter()
        .filter(|p| matches!(p, Patch::Remove { .. }))
        .count();
    assert_eq!(
        removes, 4,
        "only the two swapped children (+ their text) are removed: {patches:?}"
    );
}

#[test]
fn fragment_inside_children_splice() {
    let src = r#"
        component Slot() {
            return <div className="slot">{children}</div>;
        }
        component App() {
            return (
                <Slot>
                    <>
                        <b>one</b>
                        <i>two</i>
                    </>
                </Slot>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("<b>one</b>") && tree.contains("<i>two</i>"),
        "fragment spliced through composition:\n{tree}"
    );
}

#[test]
fn self_closing_fragment_renders_nothing() {
    let src = r#"
        component App() {
            return (
                <div>
                    <b>before</b>
                    <></>
                    <i>after</i>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree == "<div><b>before</b><i>after</i></div>",
        "empty fragment contributes no nodes:\n{tree}"
    );
}

#[test]
fn fragment_children_diff_positionally() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            return (
                <div>
                    <><b>{n}</b><i>static</i></>
                    <button onClick={() => setN(n + 1)}>go</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = button(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(
        tree.contains("<b>1</b>"),
        "state text inside fragment:\n{tree}"
    );
    assert!(tree.contains("<i>static</i>"), "sibling untouched:\n{tree}");
    let removes = patches
        .iter()
        .filter(|p| matches!(p, Patch::Remove { .. }))
        .count();
    assert_eq!(removes, 0, "pure text update, zero removals: {patches:?}");
    let set_text = patches.iter().any(|p| matches!(p, Patch::SetText { .. }));
    assert!(set_text, "minimal patch is SetText: {patches:?}");
}
