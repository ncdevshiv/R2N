//! M1-T08 acceptance: useMemo / useCallback with dependency-tracked caching.
//!
//! React semantics under test:
//! 1. useMemo computes on first render; candidate recompute only when deps
//!    change; the VALUE is reused otherwise (counted via a log).
//! 2. useMemo deps [] → computes exactly once.
//! 3. useMemo with no deps → recomputes every render.
//! 4. useCallback returns the SAME handler identity across renders when
//!    deps are unchanged — observable via an effect-dep array containing
//!    the callback (effect does not re-fire on parent re-render).
//! 5. useCallback works as an onClick target (event dispatch).
//! 6. useCallback with changed deps re-registers (effect re-fires).

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

fn click(r: &MemoryRenderer) -> Option<r2n_runtime::NodeId> {
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
fn memo_computes_once_until_deps_change() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            let double = useMemo(() => { log("compute", n); return n * 2; }, [n]);
            return <div><p className="out">{double}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // mount: computed once (n=0).
    let computes = rt
        .logs()
        .iter()
        .filter(|l| l.starts_with("compute"))
        .count();
    assert_eq!(computes, 1, "mount computed once: {:?}", rt.logs());
    assert!(
        rt.logs().iter().any(|l| l.as_str() == "compute 0"),
        "computed n=0: {:?}",
        rt.logs()
    );

    // Click 1: n 0->1, deps changed -> recompute.
    let btn = click(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click1");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l.as_str() == "compute 1"),
        "recomputed on deps change: {logs:?}"
    );

    // Click 2: same deps? n 1->2 -> recompute (deps [n] changed).
    let patches = rt.dispatch(btn, "onClick").expect("click2");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l.as_str() == "compute 2"),
        "recomputed again: {logs:?}"
    );
    let computes = logs.iter().filter(|l| l.starts_with("compute")).count();
    assert_eq!(computes, 3, "one compute per distinct deps: {logs:?}");
}

#[test]
fn memo_with_empty_deps_computes_once() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            let constant = useMemo(() => { log("compute-const"); return 42; }, []);
            return <div><p className="out">{constant}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = click(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let computes = rt
        .logs()
        .iter()
        .filter(|l| l.as_str() == "compute-const")
        .count();
    assert_eq!(computes, 1, "[] deps never recompute: {:?}", rt.logs());
}

#[test]
fn memo_without_deps_recomputes_every_render() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            let fresh = useMemo(() => { log("compute-fresh"); return n; });
            return <div><p className="out">{fresh}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = click(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let computes = rt
        .logs()
        .iter()
        .filter(|l| l.as_str() == "compute-fresh")
        .count();
    assert_eq!(
        computes,
        2,
        "no-deps memo recomputes each render: {:?}",
        rt.logs()
    );
}

#[test]
fn callback_identity_stable_when_deps_unchanged() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            let cb = useCallback(() => log("cb", n), [n]);
            // Effect deps on the callback: fires when cb identity changes.
            useEffect(() => log("effect-fires", n), [cb]);
            return <div><p className="out">{n}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // mount: effect fires once (cb fresh).
    let mut effect_fires = rt
        .logs()
        .iter()
        .filter(|l| l.starts_with("effect-fires"))
        .count();
    assert_eq!(effect_fires, 1, "mount: one effect fire: {:?}", rt.logs());
    let btn = click(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    effect_fires = rt
        .logs()
        .iter()
        .filter(|l| l.starts_with("effect-fires"))
        .count();
    // cb deps [n] changed (0->1) so cb re-registered -> effect re-fired.
    assert_eq!(
        effect_fires,
        2,
        "deps changed -> cb new -> effect re-fire: {:?}",
        rt.logs()
    );

    // second click 1->2: same thing (deps changed).
    let patches = rt.dispatch(btn, "onClick").expect("click2");
    r.apply(&patches);
    effect_fires = rt
        .logs()
        .iter()
        .filter(|l| l.starts_with("effect-fires"))
        .count();
    assert_eq!(effect_fires, 3, "deps changed again: {:?}", rt.logs());
}

#[test]
fn callback_identity_stable_when_deps_unchanged_across_static_render() {
    // With deps [100] (never change), the callback identity is stable even
    // across parent re-renders -> the dep-array effect must NOT re-fire.
    let src = r#"
        component Child() {
            let cb = useCallback(() => log("child"), [100]);
            useEffect(() => log("child-effect"), [cb]);
            return <b className="kid">k</b>;
        }
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            return <div><Child/><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert_eq!(
        rt.logs()
            .iter()
            .filter(|l| l.as_str() == "child-effect")
            .count(),
        1,
        "mount fires once: {:?}",
        rt.logs()
    );
    let btn = click(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    assert_eq!(
        rt.logs()
            .iter()
            .filter(|l| l.as_str() == "child-effect")
            .count(),
        1,
        "callback identity stable -> effect stays quiet: {:?}",
        rt.logs()
    );
}

#[test]
fn callback_works_as_onclick_target() {
    let src = r#"
        component App() {
            let cb = useCallback(() => log("cb-fired"), []);
            return <div><button onClick={cb}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = click(&r).expect("button");
    let patches = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l.as_str() == "cb-fired"),
        "useCallback as onClick: {logs:?}"
    );
}
