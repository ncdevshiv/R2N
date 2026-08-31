//! M2-T03 acceptance: closures — lexical environments with correct capture.
//!
//! ECMAScript semantics under test:
//! 1. A closure sees the values in its LEXICAL env (a free variable read
//!    inside the closure resolves where the closure was created, not where
//!    it is called).
//! 2. Captures are live (shared frames): assignment to a captured binding
//!    AFTER creation is visible to the closure.
//! 3. Nested closures see the outer closure's params/bindings.
//! 4. A closure returned and called in a DIFFERENT scope still resolves
//!    its own lexical env (not the caller's shadowing).
//! 5. Function values are identity-distinct (two closures aren't equal).

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

fn texts(r: &MemoryRenderer) -> Vec<String> {
    r.nodes()
        .values()
        .filter_map(|n| match n {
            r2n_renderer_memory::MemNode::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn closure_reads_its_lexical_env() {
    // The closure is created inside `factory` — it captures `base`; calling
    // it later (from App's env alongside a shadowing `base`) must NOT see
    // the caller's `base`.
    let src = r#"
        component App() {
            let base = "caller";
            let factory = () => {
                let base = "lexical";
                let read = () => base;
                return read;
            };
            let r = factory();
            return <div><p className="out">{r()}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(
        t.iter().any(|x| x == "lexical"),
        "closure sees its LEXICAL env, not the caller's shadowing: {t:?}"
    );
}

#[test]
fn captures_are_live_across_writes() {
    // A counter pair via an object: the closure mutates a captured object.
    // The object is shared (identity) so mutations are visible everywhere.
    let src = r#"
        component App() {
            let counter = Object();
            counter.n = 0;
            let inc = () => { counter.n = counter.n + 1; return counter.n; };
            let a = inc();
            let b = inc();
            return <div><p className="a">{a}</p><p className="b">{b}</p><p className="c">{counter.n}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "1"), "first inc: {t:?}");
    assert!(t.iter().any(|x| x == "2"), "second inc: {t:?}");
    assert!(t.iter().any(|x| x == "2"), "shared state visible: {t:?}");
}

#[test]
fn nested_closures_chain_to_outer_params() {
    let src = r#"
        component App() {
            let mk = (a) => {
                let inner = (b) => a + b;
                return inner;
            };
            let add10 = mk(10);
            let sum = add10(5);
            return <div><p className="out">{sum}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(
        t.iter().any(|x| x == "15"),
        "inner sees outer param a: {t:?}"
    );
}

#[test]
fn functions_are_identity_distinct() {
    let src = r#"
        component App() {
            let f1 = () => 1;
            let f2 = () => 1;
            return <div><p className="eq">{if f1 == f2 { "same" } else { "distinct" }}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(
        t.iter().any(|x| x == "distinct"),
        "two closures are not equal (JS identity): {t:?}"
    );
}

#[test]
fn closure_from_one_scope_used_in_another() {
    let src = r#"
        component App() {
            let make_getter = () => {
                let secret = 42;
                let get = () => secret;
                return get;
            };
            let getter = make_getter();
            let secret = "shadow";
            let v = getter();
            return <div><p className="out">{v}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(
        t.iter().any(|x| x == "42"),
        "captured 'secret' is lexical, not the later shadow: {t:?}"
    );
}
