//! Tests for the in-memory renderer: it consumes the same `Patch` stream every
//! backend does, and must apply create/update/remove/move correctly.

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::patch::Patch;
use r2n_runtime::Renderer;
use r2n_runtime::Runtime;

fn render_once(src: &str) -> String {
    let t = compile_source(src).unwrap();
    let mut rt = Runtime::new(t);
    let patches = rt.flush().unwrap();
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    r.render_string()
}

#[test]
fn renders_nested_tree() {
    let tree = render_once(
        r#"
        component App() {
            return <div><span>{"hi"}</span><span>{"there"}</span></div>;
        }
        export default App;
    "#,
    );
    assert_eq!(tree, "<div><span>hi</span><span>there</span></div>");
}

#[test]
fn applies_remove_patch_cleanly() {
    // Render with two children, then simulate removal by replaying patches
    // against a fresh renderer to confirm the Remove op drops the subtree.
    let t = compile_source(
        r#"
        component App() {
            return <ul><li>{"a"}</li><li>{"b"}</li></ul>;
        }
        export default App;
    "#,
    )
    .unwrap();
    let mut rt = Runtime::new(t);
    let p1 = rt.flush().unwrap();
    let mut r = MemoryRenderer::new();
    r.apply(&p1);
    assert_eq!(r.render_string(), r#"<ul><li>a</li><li>b</li></ul>"#);

    // Find the id of the second <li> ("b") from the live node map (don't rely
    // on hardcoded ids) and confirm the Remove patch drops it.
    let second_li_id = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "li" => {
                let kids = r.children_of().get(&Some(*id)).cloned().unwrap_or_default();
                // The <li> whose text child is "b".
                let is_b = kids.iter().any(|k| match r.nodes().get(k) {
                    Some(r2n_renderer_memory::MemNode::Text { text }) => text == "b",
                    _ => false,
                });
                if is_b {
                    Some(*id)
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("second li present");
    r.apply(&[Patch::Remove { id: second_li_id }]);
    assert_eq!(r.render_string(), r#"<ul><li>a</li></ul>"#);
}

#[test]
fn patch_stream_is_deterministic_across_renders() {
    // Two independent compiles+renders of the same source yield identical trees.
    let src = r#"
        component App() {
            let items = [1, 2, 3];
            return <ol>{items.map((x) => <li key={x}>{x}</li>)}</ol>;
        }
        export default App;
    "#;
    assert_eq!(render_once(src), render_once(src));
}
