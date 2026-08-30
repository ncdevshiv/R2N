//! Integration tests for the R2N runtime engine: rendering, the reactive
//! loop (minimal patches on state change), and keyed list reconciliation.

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::patch::Patch;
use r2n_runtime::Renderer;
use r2n_runtime::Runtime;

fn render(src: &str) -> (Runtime, MemoryRenderer, String) {
    let t = compile_source(src).expect("compile");
    let mut rt = Runtime::new(t);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    let tree = r.render_string();
    (rt, r, tree)
}

#[test]
fn counter_renders_initial_state() {
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            return <div className="counter"><h1>{n}</h1></div>;
        }
        export default Counter;
    "#;
    let (_, _, tree) = render(src);
    assert_eq!(tree, r#"<div className="counter"><h1>0</h1></div>"#);
}

#[test]
fn reactive_loop_emits_only_text_patch() {
    // The same reactive loop as the events suite, driven through the real
    // event path: click → handler → setter → dirty → flush → ONE SetText.
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            let setN = count[1];
            return <div className="counter"><h1>{n}</h1><button onClick={() => setN(n + 1)}>+1</button></div>;
        }
        export default Counter;
    "#;
    let (mut rt, mut r, tree) = render(src);
    assert_eq!(
        tree,
        r#"<div className="counter"><h1>0</h1><button>+1</button></div>"#
    );
    let btn = *r
        .nodes()
        .iter()
        .find(|(_, n)| matches!(n, r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button"))
        .map(|(id, _)| id)
        .expect("button");
    // Each click must emit exactly one SetText patch (the count text) —
    // proving minimal, deterministic reconciliation with no parent recreation.
    for step in 1..=5u64 {
        let patches = rt.dispatch(btn, "onClick").expect("dispatch");
        assert_eq!(patches.len(), 1, "expected exactly 1 patch at step {step}");
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
fn conditional_renders_both_branches() {
    let small = r#"
        component App() {
            let count = useState(2);
            let n = count[0];
            return <p>{if n < 5 { "small" } else { "big" }}</p>;
        }
        export default App;
    "#;
    let (_, _, small_tree) = render(small);
    assert_eq!(small_tree, "<p>small</p>");

    let big = r#"
        component App() {
            let count = useState(9);
            let n = count[0];
            return <p>{if n < 5 { "small" } else { "big" }}</p>;
        }
        export default App;
    "#;
    let (_, _, big_tree) = render(big);
    assert_eq!(big_tree, "<p>big</p>");
}

#[test]
fn keyed_list_reconciles_without_duplication() {
    let src = r#"
        component List() {
            let items = ["a", "b", "c"];
            return <ul>{items.map((x) => <li key={x}>{x}</li>)}</ul>;
        }
        export default List;
    "#;
    let (_, _, tree) = render(src);
    // `key` is reconciliation metadata: it must NOT render (React strips it).
    assert_eq!(tree, r#"<ul><li>a</li><li>b</li><li>c</li></ul>"#);
}

#[test]
fn nested_components_inline_with_state() {
    let src = r#"
        component Inner() {
            let s = useState(7);
            let v = s[0];
            return <span>{v}</span>;
        }
        component Outer() {
            return <div><Inner/></div>;
        }
        export default Outer;
    "#;
    let (_, _, tree) = render(src);
    assert_eq!(tree, "<div><span>7</span></div>");
}

#[test]
fn string_concatenation_in_text() {
    let src = r#"
        component App() {
            let a = "Hello, ";
            let b = "world";
            return <h1>{a + b}</h1>;
        }
        export default App;
    "#;
    let (_, _, tree) = render(src);
    assert_eq!(tree, "<h1>Hello, world</h1>");
}
