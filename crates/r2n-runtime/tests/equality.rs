//! M2-T05 acceptance: equality & coercion.
//!
//! ECMA-262 observable semantics through compiled components:
//! 1. `===` never coerces: "1" === 1 is false, 1 === 1 true; NaN !== NaN.
//! 2. `!==` is the negation, including across types.
//! 3. `==` null/undefined pair: null == undefined true, both == 0 false.
//! 4. `==` number/string coercion in both directions ("1" == 1, "" == 0).
//! 5. `==` boolean coerces FIRST (true == 1, true == "1", but true == "2"
//!    is false).
//! 6. `==` object identity: an object equals itself, not a lookalike;
//!    ToPrimitive via valueOf participates (obj == 5 with valueOf).
//! 7. ToNumber/ToString across types back the coercions (already exercised
//!    by value_model.rs; here via equality outcomes).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn texts_of(src: &str) -> Vec<String> {
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

fn one(src: &str) -> String {
    let t = texts_of(src);
    assert_eq!(t.len(), 1, "exactly one text node: {t:?}");
    t[0].clone()
}

#[test]
fn strict_eq_never_coerces() {
    let src = r#"
        component App() {
            let a = "1" === 1;
            let b = 1 === 1;
            let c = null === undefined;
            let d = 0 === false;
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p><p>{if c { "T" } else { "F" }}</p><p>{if d { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(t, vec!["F", "T", "F", "F"], "=== without coercion: {t:?}");
}

#[test]
fn strict_eq_nan_and_negative_zero() {
    let src = r#"
        component App() {
            let nan = 0 / 0;
            let a = nan === nan;
            let b = -0 === 0;
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(t, vec!["F", "T"], "NaN!==NaN but -0===0: {t:?}");
}

#[test]
fn strict_neq_is_negation() {
    let src = r#"
        component App() {
            let a = "1" !== 1;
            let b = 1 !== 1;
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(t, vec!["T", "F"], "!== negates ===: {t:?}");
}

#[test]
fn loose_null_undefined_pair() {
    let src = r#"
        component App() {
            let a = null == undefined;
            let b = null == 0;
            let c = undefined == "";
            let d = null == null;
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p><p>{if c { "T" } else { "F" }}</p><p>{if d { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(
        t,
        vec!["T", "F", "F", "T"],
        "null==undefined, not 0/\": {t:?}"
    );
}

#[test]
fn loose_number_string_coercion() {
    let src = r#"
        component App() {
            let a = "1" == 1;
            let b = 1 == "1";
            let c = "" == 0;
            let d = " 10 " == 10;
            let e = "abc" == 0;
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p><p>{if c { "T" } else { "F" }}</p><p>{if d { "T" } else { "F" }}</p><p>{if e { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(t, vec!["T", "T", "T", "T", "F"], "string<->number: {t:?}");
}

#[test]
fn loose_boolean_coerces_first() {
    let src = r#"
        component App() {
            let a = true == 1;
            let b = true == "1";
            let c = true == "2";
            let d = false == 0;
            let e = false == "";
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p><p>{if c { "T" } else { "F" }}</p><p>{if d { "T" } else { "F" }}</p><p>{if e { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(
        t,
        vec!["T", "T", "F", "T", "T"],
        "bool->number first: {t:?}"
    );
}

#[test]
fn loose_object_identity_and_valueof() {
    // A lookalike object is NOT == its twin; the same object IS == itself.
    // With a valueOf method, the object coerces through it (obj == 5).
    let src = r#"
        component App() {
            let obj = Object();
            obj.v = 5;
            obj.valueOf = () => obj.v;
            let twin = Object();
            twin.v = 5;
            let a = obj == 5;
            let b = obj == obj;
            let c = twin == obj;
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p><p>{if c { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(t, vec!["T", "T", "F"], "identity + valueOf: {t:?}");
}

#[test]
fn loose_object_without_methods_is_typeerror() {
    // `Object.create(null) == 1` raises TypeError in ECMA (no valueOf).
    // Our plain objects are null-prototype, so a methodless object must
    // error, not silently compare.
    let src = r#"
        component App() {
            let a = 1;
            let obj = Object.create(null);
            return <div><p>{if obj == 1 { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let err = rt
        .flush()
        .expect_err("methodless object ToPrimitive raises");
    assert!(
        err.to_string().contains("primitive"),
        "TypeError mentions primitive conversion: {err}"
    );
}

#[test]
fn loose_bigint_forms() {
    let src = r#"
        component App() {
            let a = BigInt(10) == "10";
            let b = BigInt(10) == 10;
            let c = BigInt(10) == "11";
            let d = "10" == BigInt(10);
            return <div><p>{if a { "T" } else { "F" }}</p><p>{if b { "T" } else { "F" }}</p><p>{if c { "T" } else { "F" }}</p><p>{if d { "T" } else { "F" }}</p></div>;
        }
        export default App;
    "#;
    let t = texts_of(src);
    assert_eq!(t, vec!["T", "T", "F", "T"], "BigInt math equality: {t:?}");
}

#[test]
fn equality_drives_react_logic() {
    // Realistic usage: `===` guards a render branch; `==` accepts a numeric
    // string (e.g. from an input) without an explicit parse.
    let src = r#"
        component App() {
            let n = useState(0);
            let count = n[0];
            let setCount = n[1];
            setCount("3");
            let label = if count === 3 { "three" } else { "other" };
            return <div><p>{label}</p></div>;
        }
        export default App;
    "#;
    assert_eq!(one(src), "other", "\"3\" === 3 is false in strict mode");
}
