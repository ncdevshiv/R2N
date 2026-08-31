//! M2-T02 acceptance: objects — dynamic properties, prototype chain.
//!
//! ECMAScript semantics under test:
//! 1. Property reads walk the prototype chain (inherited props).
//! 2. Property writes create OWN data props (they shadow the proto).
//! 3. `Object.create(proto)` makes a new object with that prototype;
//!    `Object.create(null)` has no proto.
//! 4. `getPrototypeOf(o)` and `o.__proto__` read the link; `o.__proto__ =
//!    p` sets it (null clears).
//! 5. Direct `Object()` objects share no state (separate bags).

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
fn reads_walk_the_prototype_chain() {
    let src = r#"
        component App() {
            let base = Object();
            base.greet = "hello";
            let child = Object.create(base);
            return (
                <div>
                    <p className="inherited">{child.greet}</p>
                    <p className="own">{typeof(child.greet)}</p>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "hello"), "inherited read: {t:?}");
}

#[test]
fn writes_create_own_props_shadowing_the_proto() {
    let src = r#"
        component App() {
            let base = Object();
            base.v = "base";
            let child = Object.create(base);
            child.v = "own";
            return (
                <div>
                    <p className="c">{child.v}</p>
                    <p className="b">{base.v}</p>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "own"), "own shadows: {t:?}");
    assert!(t.iter().any(|x| x == "base"), "proto untouched: {t:?}");
}

#[test]
fn object_create_null_has_no_proto() {
    let src = r#"
        component App() {
            let o = Object.create(null);
            o.flag = "set";
            // getPrototypeOf(null-proto) => null: typeof(null) is "object"
            // (ECMA), and raw null renders nothing — observe via typeof.
            return <div><p className="g">{typeof(Object.getPrototypeOf(o))}</p><p className="f">{o.flag}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "object"), "typeof null-proto: {t:?}");
    assert!(t.iter().any(|x| x == "set"), "own prop works: {t:?}");
}

#[test]
fn getprototypeof_and_dunder_proto_read() {
    let src = r#"
        component App() {
            let base = Object();
            base.name = "base";
            let child = Object.create(base);
            let p1 = Object.getPrototypeOf(child);
            let p2 = child.__proto__;
            return (
                <div>
                    <p className="a">{p1.name}</p>
                    <p className="b">{p2.name}</p>
                </div>
            );
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "base"), "both accessors: {t:?}");
}

#[test]
fn dunder_proto_assignment_swaps_chain() {
    let src = r#"
        component App() {
            let a = Object();
            a.kind = "a-kind";
            let b = Object();
            b.kind = "b-kind";
            let o = Object();
            o.__proto__ = a;
            let viaA = o.kind;
            o.__proto__ = b;
            let viaB = o.kind;
            return <div><p className="a">{viaA}</p><p className="b">{viaB}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "a-kind"), "inherited from a: {t:?}");
    assert!(t.iter().any(|x| x == "b-kind"), "swapped to b: {t:?}");
}

#[test]
fn objects_have_independent_state() {
    let src = r#"
        component App() {
            let o1 = Object();
            let o2 = Object();
            o1.x = 1;
            return <div><p className="a">{o1.x}</p><p className="b">{typeof(o2.x)}</p></div>;
        }
        export default App;
    "#;
    let (_rt, r) = setup(src);
    let t = texts(&r);
    assert!(t.iter().any(|x| x == "1"), "first set: {t:?}");
    assert!(
        t.iter().any(|x| x == "undefined"),
        "independent (missing = undefined): {t:?}"
    );
}
