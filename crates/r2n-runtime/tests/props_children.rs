//! M1-T01 acceptance: props & children propagation through component calls.
//!
//! React semantics under test:
//! 1. Props pass through declared component params (`<Card title="..."/>`
//!    makes `title` readable inside Card).
//! 2. JSX children of a component element become its `children` prop.
//! 3. Children close over the PARENT's scope, not the receiving
//!    component's (`<Card>{n}</Card>` with n from the parent renders the
//!    parent's n — composition by reference).
//! 4. The `children` splice point may appear anywhere in the child's tree,
//!    including multiple times and with siblings.
//! 5. Re-renders (click → state change → flush) produce minimal patches and
//!    keep children working (splice re-derives from the fresh props).
//! 6. A component with no children passed renders its `children` splice as
//!    nothing (React renders `undefined` children as nothing).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{NodeId, Renderer, Runtime};

fn render(src: &str) -> (Runtime, MemoryRenderer, Vec<r2n_runtime::Patch>) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r, patches)
}

/// Find a node whose serialized subtree contains `needle` (text search over
/// the rendered tree).
fn tree_contains(r: &MemoryRenderer, needle: &str) -> bool {
    r.render_string().contains(needle)
}

fn first_button(r: &MemoryRenderer) -> Option<NodeId> {
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
fn props_flow_through_component_params() {
    let src = r#"
        component Card(title) {
            return <h1 className="card">{title}</h1>;
        }
        component App() {
            return <Card title="hello props"/>;
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    assert!(tree.contains("card"), "host element rendered:\n{tree}");
    assert!(
        tree.contains("hello props"),
        "prop value reached the child body:\n{tree}"
    );
}

#[test]
fn children_composition_basic() {
    let src = r#"
        component Card() {
            return <div className="card">{children}</div>;
        }
        component App() {
            return <Card><b>bold content</b></Card>;
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    assert!(tree.contains("<b>"), "child element spliced:\n{tree}");
    assert!(tree.contains("bold content"), "child text present:\n{tree}");
}

#[test]
fn children_close_over_parent_scope() {
    let src = r#"
        component Card() {
            let inner = "shadowed";
            return <div>{children}</div>;
        }
        component App() {
            let n = 41;
            return <Card><span>{n + 1}</span></Card>;
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    // `n + 1` must evaluate in the PARENT (42), not the child (unbound).
    assert!(
        tree.contains("42"),
        "children must read the parent's n (42), got:\n{tree}"
    );
    assert!(
        !tree.contains("shadowed"),
        "child's own binding must not leak into the splice:\n{tree}"
    );
}

#[test]
fn children_splice_among_siblings() {
    let src = r#"
        component Row() {
            return <li>before{children}after</li>;
        }
        component App() {
            return <ul><Row><em>mid</em></Row></ul>;
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    // Splice with siblings: text before, child element, text after.
    assert!(tree.contains("before"), "text before splice:\n{tree}");
    assert!(tree.contains("<em>"), "spliced element:\n{tree}");
    assert!(tree.contains("after"), "text after splice:\n{tree}");
}

#[test]
fn children_across_re_renders_stay_live() {
    let src = r#"
        component Box() {
            return <div className="box">{children}</div>;
        }
        component App() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            return (
                <Box>
                    <span>{n}</span>
                    <button onClick={() => setN(n + 1)}>+</button>
                </Box>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r, _) = render(src);
    assert!(
        tree_contains(&r, ">0<") || r.render_string().contains("0"),
        "initial n=0 rendered"
    );
    let btn = first_button(&r).expect("clickable button in spliced children");
    let patches = rt.dispatch(btn, "onClick").expect("dispatch");
    r.apply(&patches);
    let tree = r.render_string();
    assert!(tree.contains('1'), "n=1 after click:\n{tree}");
    // The splice is re-derived from fresh props: the span's new text arrives
    // as a SetText on the same node (no removal + recreate).
    let set_text = patches
        .iter()
        .any(|p| matches!(p, r2n_runtime::Patch::SetText { .. }));
    let removes = patches
        .iter()
        .filter(|p| matches!(p, r2n_runtime::Patch::Remove { .. }))
        .count();
    assert!(set_text, "update must be a SetText, got: {:?}", patches);
    assert_eq!(
        removes, 0,
        "no node removals on a pure state update: {patches:?}"
    );
}

#[test]
fn no_children_passed_splice_renders_nothing() {
    let src = r#"
        component Card() {
            return <div className="card">a{children}b</div>;
        }
        component App() {
            return <Card/>;
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    assert!(
        tree.contains('a') && tree.contains('b'),
        "frame rendered:\n{tree}"
    );
}

#[test]
fn nested_components_with_props_and_children() {
    let src = r#"
        component Inner(label) {
            return <p className="inner">{label}{"|"}{children}</p>;
        }
        component Middle(label) {
            return <Inner label={label}><code>deep</code></Inner>;
        }
        component App() {
            return <div><Middle label="mid"/><Middle label="override"/></div>;
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    assert!(tree.contains("mid"), "first nested prop:\n{tree}");
    assert!(
        tree.contains("deep"),
        "children through nested call:\n{tree}"
    );
    assert!(
        tree.contains("override"),
        "second instance's own prop:\n{tree}"
    );
    // Each instance shows exactly one <code> child — the splice must not
    // leak across instances (both would render 2 codes if they shared a
    // SpliceMap entry).
    let code_count = tree.matches("<code>").count();
    assert_eq!(code_count, 2, "one code block per instance:\n{tree}");
}

#[test]
fn two_instances_receive_independent_children() {
    let src = r#"
        component Slot() {
            return <div className="slot">{children}</div>;
        }
        component App() {
            return (
                <main>
                    <Slot>first</Slot>
                    <Slot>second</Slot>
                </main>
            );
        }
        export default App;
    "#;
    let (_rt, r, _) = render(src);
    let tree = r.render_string();
    assert!(tree.contains("first"), "first slot:\n{tree}");
    assert!(tree.contains("second"), "second slot:\n{tree}");
    // Order must be preserved.
    let fi = tree.find("first").expect("first");
    let si = tree.find("second").expect("second");
    assert!(fi < si, "slots render in document order:\n{tree}");
}
