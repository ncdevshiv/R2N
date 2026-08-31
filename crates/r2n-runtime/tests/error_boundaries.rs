//! M1-T13 acceptance: error boundaries — capture, fallback, recovery.
//!
//! React semantics under test:
//! 1. A class with `getDerivedStateFromError` + `componentDidCatch`
//!    captures an error thrown while rendering a DESCENDANT; the boundary
//!    re-renders with the derived state (fallback UI).
//! 2. Both hooks receive the error (observable via log).
//! 3. Without a boundary the error propagates to the top.
//! 4. A class without the boundary hooks does NOT catch.
//! 5. A bounday in the middle of a tree catches errors from nested
//!    components below it, while siblings above are unaffected.

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

// Error sources: a component that raises a runtime error during render.
// The engine surfaces runtime errors as Result Err — the boundary pattern
// requires a DESCENDANT to fail. Use a function component that calls
// `bump()` — a runtime error path (e.g. calling a non-function or an
// undefined operation). The cleanest injectable failure: `this.state + 1`
// is fine; instead call `missingFn()` -> unbound variable error.

#[test]
fn boundary_captures_descendant_error_and_renders_fallback() {
    let src = r#"
        class Boundary extends Component {
            state = 0;
            getDerivedStateFromError(err) { log("derived", err); return 1; }
            componentDidCatch(err) { log("caught", err); }
            render() {
                return this.state == 1
                    ? <p className="fallback">boom</p>
                    : <div className="kids">{children}</div>;
            }
        }
        component Bomber() {
            let X = 1;
            return <p>{nope()}</p>;
        }
        component App() {
            return <Boundary><Bomber/></Boundary>;
        }
        export default App;
    "#;
    let (rt, r) = setup(src);
    let tree = r.render_string();
    assert!(tree.contains("boom"), "boundary shows the fallback: {tree}");
    assert!(!tree.contains("kids"), "normal tree replaced: {tree}");
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l.starts_with("derived")),
        "getDerivedStateFromError ran: {logs:?}"
    );
    assert!(
        logs.iter().any(|l| l.starts_with("caught")),
        "componentDidCatch ran: {logs:?}"
    );
}

#[test]
fn no_boundary_error_propagates_to_flush() {
    let src = r#"
        component Bomber() {
            let n = 1;
            return <p>{nope()}</p>;
        }
        component App() {
            return <div><Bomber/></div>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let res = rt.flush();
    assert!(res.is_err(), "uncaught render error propagates");
}

#[test]
fn non_boundary_class_does_not_catch() {
    let src = r#"
        class Plain extends Component {
            render() { return <div>{children}</div>; }
        }
        component Bomber() {
            let n = 1;
            return <p>{nope()}</p>;
        }
        component App() {
            return <Plain><Bomber/></Plain>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let res = rt.flush();
    assert!(res.is_err(), "class without hooks is not a boundary");
}

#[test]
fn mid_tree_boundary_protects_descendants() {
    let src = r#"
        class Boundary extends Component {
            state = 0;
            getDerivedStateFromError(err) { log("derived"); return 1; }
            componentDidCatch(err) { log("caught"); }
            render() {
                return this.state == 1
                    ? <p className="fallback">safe</p>
                    : <div className="kids">{children}</div>;
            }
        }
        component Bomber() {
            let n = 1;
            return <p>{nope()}</p>;
        }
        component Hdr() {
            return <h1 className="head">title</h1>;
        }
        component App() {
            return <div><Hdr/><Boundary><Bomber/></Boundary><footer/></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let tree = r.render_string();
    assert!(
        tree.contains("title"),
        "sibling above the boundary still renders: {tree}"
    );
    assert!(
        tree.contains("safe"),
        "boundary fallback replaces only its subtree: {tree}"
    );
}
