//! M2-T06 acceptance: exceptions — throw/try/catch/finally with value
//! propagation across calls.
//!
//! ECMA-262 observable semantics through compiled components:
//! 1. `throw x` raises ANY value; `catch (e)` binds it verbatim.
//! 2. `new Error(msg)` is an Error-shaped object; `e.message` reads back.
//! 3. Internal runtime errors (e.g. unbound variables — ECMA ReferenceError)
//!    are catchable too; the catch binds their message string.
//! 4. `finally` runs on success, after catch, and on uncaught paths; an error
//!    raised IN finally replaces the pending outcome.
//! 5. Uncaught throws surface as flush/dispatch errors (error boundaries
//!    still see the message via componentDidCatch).
//! 6. Throws propagate across function calls to the nearest enclosing try.

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn render_texts(src: &str) -> Vec<String> {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    r.nodes()
        .values()
        .filter_map(|n| match n {
            r2n_renderer_memory::MemNode::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn one_text(src: &str) -> String {
    let t = render_texts(src);
    assert_eq!(t.len(), 1, "exactly one text node: {t:?}");
    t[0].clone()
}

#[test]
fn catch_binds_thrown_string() {
    let src = r#"
        component App() {
            let msg = "none";
            try {
                throw "boom";
            } catch (e) {
                msg = e;
            }
            return <div><p>{msg}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one_text(src), "boom");
}

#[test]
fn catch_binds_thrown_number_and_object() {
    let src = r#"
        component App() {
            let a = 0;
            let b = "";
            try {
                throw 42;
            } catch (e) {
                a = e;
            }
            try {
                throw new Error("bad input");
            } catch (err) {
                b = err.message;
            }
            return <div><p>{a}</p><p>{b}</p></div>;
        }
        export default App;
    "#;
    let t = render_texts(src);
    assert_eq!(
        t,
        vec!["42", "bad input"],
        "thrown value + Error.message: {t:?}"
    );
}

#[test]
fn internal_errors_are_catchable() {
    // ECMA: an unbound variable raises ReferenceError — catchable. Our
    // internal errors bind as their message string (no Error class yet).
    let src = r#"
        component App() {
            let out = "";
            try {
                out = neverDefined + 1;
            } catch (e) {
                out = "caught:" + e;
            }
            return <div><p>{out}</p></div>;
        }
        export default App;
    "#;
    let text = one_text(src);
    assert!(
        text.starts_with("caught:unbound variable 'neverDefined'"),
        "internal error caught with message: {text}"
    );
}

#[test]
fn finally_runs_on_success_and_after_catch() {
    let src = r#"
        component App() {
            let log = "";
            try {
                log = log + "T";
            } finally {
                log = log + "F1";
            }
            try {
                throw "x";
            } catch (e) {
                log = log + "C";
            } finally {
                log = log + "F2";
            }
            return <div><p>{log}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one_text(src), "TF1CF2");
}

#[test]
fn finally_runs_on_uncaught_path_via_outer_catch() {
    // The inner try has no catch: finally still runs, then the throw
    // propagates to the OUTER catch (this is the propagation-across-blocks
    // half of the task).
    let src = r#"
        component App() {
            let log = "";
            try {
                try {
                    throw "inner";
                } finally {
                    log = log + "F";
                }
            } catch (e) {
                log = log + "C" + e;
            }
            return <div><p>{log}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one_text(src), "FCinner");
}

#[test]
fn error_in_finally_replaces_outcome() {
    // Catch handled the original throw, but finally raises: its error wins.
    let src = r#"
        component App() {
            try {
                throw "first";
            } catch (e) {
                let ignored = e;
            } finally {
                throw "second";
            }
            return <div><p>{"unreachable"}</p></div>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let err = rt.flush().expect_err("finally's throw replaces the catch");
    assert!(
        err.to_string().contains("second"),
        "finally error wins: {err}"
    );
}

#[test]
fn uncaught_throw_surfaces_at_flush() {
    let src = r#"
        component App() {
            throw "fatal";
            return <div/>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let err = rt.flush().expect_err("uncaught throw surfaces");
    assert!(
        err.to_string().contains("fatal"),
        "thrown message surfaces: {err}"
    );
}

#[test]
fn throw_propagates_across_function_calls() {
    // THE core task item: the throw happens two calls deep (b -> a); it
    // unwinds both frames into the component's try.
    let src = r#"
        component App() {
            let caught = "";
            let deep = () => { throw "deep-boom"; };
            let outer = () => { deep(); };
            try {
                outer();
            } catch (e) {
                caught = "got:" + e;
            }
            return <div><p>{caught}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one_text(src), "got:deep-boom");
}

#[test]
fn nested_try_and_rethrow() {
    // Inner try catches only ITS throw kind; the rethrown error reaches the
    // outer catch (throw inside a catch block propagates).
    let src = r#"
        component App() {
            let log = "";
            try {
                try {
                    throw "inner-boom";
                } catch (e) {
                    log = log + "inner-saw-" + e;
                    throw "rethrown";
                }
            } catch (e2) {
                log = log + "|outer-saw-" + e2;
            }
            return <div><p>{log}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one_text(src), "inner-saw-inner-boom|outer-saw-rethrown");
}

#[test]
fn optional_catch_binding() {
    // ES2019: `catch { }` without a parameter.
    let src = r#"
        component App() {
            let out = "ok";
            try {
                throw "ignored-value";
            } catch {
                out = "caught-without-binding";
            }
            return <div><p>{out}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one_text(src), "caught-without-binding");
}

#[test]
fn throw_inside_event_handler_surfaces_at_dispatch() {
    let src = r#"
        component App() {
            return <button onClick={() => { throw "handler-boom"; }}>Go</button>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("initial flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    // Find the button node and click it.
    let btn = r
        .nodes()
        .iter()
        .find(|(_, n)| matches!(n, r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button"))
        .map(|(id, _)| *id)
        .expect("button node");
    // Handlers register under their prop name (the runtime's event key).
    let err = rt
        .dispatch(btn, "onClick")
        .expect_err("handler throw surfaces at dispatch");
    assert!(
        err.to_string().contains("handler-boom"),
        "handler throw message: {err}"
    );
}

#[test]
fn error_boundary_still_catches_thrown_errors() {
    // Interplay with M1: a child that throws (a real JS `throw`) during
    // render is captured by the boundary; the thrown value's message flows
    // through err (same as internal errors) into the fallback UI.
    let src = r#"
        class BadChild extends Component {
            render() {
                throw "child-fatal";
                return <p>never</p>;
            }
        }
        class Boundary extends Component {
            state = 0;
            getDerivedStateFromError(err) { return err; }
            componentDidCatch(err) { log("caught", err); }
            render() {
                return this.state == 0
                    ? <BadChild/>
                    : <p>{this.state}</p>;
            }
        }
        component App() {
            return <Boundary/>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("boundary absorbs the throw");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    let text = r
        .nodes()
        .values()
        .filter_map(|n| match n {
            r2n_renderer_memory::MemNode::Text { text } => Some(text.clone()),
            _ => None,
        })
        .find(|t| t == "child-fatal")
        .expect("thrown message rendered via the boundary fallback");
    assert_eq!(text, "child-fatal");
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l.contains("child-fatal")),
        "componentDidCatch saw the thrown message: {logs:?}"
    );
}
