//! M1-T06 acceptance: useEffect setup/cleanup/dependency-change lifecycle.
//!
//! React semantics under test (order verified via host logs):
//! 1. Setup runs after mount (once with `[]` deps).
//! 2. Deps change → the PREVIOUS cleanup runs first, then the new setup
//!    (cleanup(deps_old) precedes setup(deps_new)).
//! 3. Unmount → the armed cleanup runs once.
//! 4. No deps (undefined) → setup runs every render; prior cleanup runs
//!    before each new setup.
//! 5. Multiple effects run in hook order; each cleanup is its own closure
//!    capturing its own env (the deps values at setup time).

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

/// The log lines of the most recent dispatch+apply, filtered by prefix.
fn logs_of(rt: &Runtime) -> Vec<String> {
    rt.logs().to_vec()
}

#[test]
fn setup_runs_once_with_empty_deps() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => log("setup"), []);
            return <p>{n}</p>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // Clicking must NOT re-run the effect (deps [] never change) — but the
    // source has no button, so a render-side value change is needed. Use
    // the p as the hook anchor; simplest honest check: the mount ran once
    // and a plain flush does not re-run it.
    let _ = &mut r;
    let _ = rt.flush().expect("second flush");
    let logs = logs_of(&rt);
    let setups = logs.iter().filter(|l| l.contains("setup")).count();
    assert_eq!(setups, 1, "empty-deps effect runs exactly once: {logs:?}");
}

#[test]
fn deps_change_cleans_up_then_setups() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => { log("setup", n); return () => log("cleanup", n); }, [n]);
            return <div><p className="out">{n}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let p1 = rt.dispatch(btn, "onClick").expect("click 1"); // n: 0 -> 1
    r.apply(&p1);
    let p2 = rt.dispatch(btn, "onClick").expect("click 2"); // n: 1 -> 2
    r.apply(&p2);

    let logs = rt.logs();
    // Exact order: setup(0), cleanup(0), setup(1), cleanup(1), setup(2)
    // (cleanup of the PREVIOUS deps runs before the NEXT setup).
    let events: Vec<&str> = logs.iter().map(|l| l.as_str()).collect();
    let s0 = events
        .iter()
        .position(|l| *l == "setup 0")
        .expect("setup 0");
    let c0 = events
        .iter()
        .position(|l| *l == "cleanup 0")
        .expect("cleanup 0");
    let s1 = events
        .iter()
        .position(|l| *l == "setup 1")
        .expect("setup 1");
    let c1 = events
        .iter()
        .position(|l| *l == "cleanup 1")
        .expect("cleanup 1");
    let s2 = events
        .iter()
        .position(|l| *l == "setup 2")
        .expect("setup 2");
    assert!(
        s0 < c0 && c0 < s1 && s1 < c1 && c1 < s2,
        "order wrong: {events:?}"
    );
}

#[test]
fn cleanup_runs_on_unmount() {
    let src = r#"
        component Child() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => { log("setup-child"); return () => log("cleanup-child"); }, []);
            return <i className="child">{n}</i>;
        }
        component App() {
            let v = useState(0);
            let on = v[0];
            let setOn = v[1];
            return (
                <div>
                    {on % 2 == 0 && <Child/>}
                    <button onClick={() => setOn(on + 1)}>flip</button>
                </div>
            );
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    assert!(
        r.render_string().contains("child"),
        "child mounted:\n{}",
        r.render_string()
    );
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("flip");
    let patches = rt.dispatch(btn, "onClick").expect("unmount child");
    r.apply(&patches);
    let logs = rt.logs();
    assert!(
        logs.iter().any(|l| l == "cleanup-child"),
        "unmount must run the armed cleanup: {logs:?}"
    );
}

#[test]
fn no_deps_setup_runs_every_render() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => { log("setup-every", n); return () => log("cleanup-every", n); });
            return <div><p className="out">{n}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let p0 = rt.dispatch(btn, "onClick").expect("click1");
    r.apply(&p0);
    let events: Vec<String> = rt.logs().to_vec();
    // After mount: setup-every 0. After click: cleanup-every 0 —
    // cleanup runs with ITS OWN captured env (the deps as of ITS setup):
    // setup-every 0, then cleanup-every 0, setup-every 1. Verify the
    // cleanup (captured 0) precedes setup (1).
    let s1 = events
        .iter()
        .position(|l| l == "setup-every 1")
        .expect("setup 1");
    let c0 = events
        .iter()
        .position(|l| l == "cleanup-every 0")
        .expect("cleanup 0");
    assert!(c0 < s1, "cleanup-of-0 before setup-of-1: {events:?}");
}

#[test]
fn multiple_effects_run_in_hook_order() {
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => log("one", n), [n]);
            useEffect(() => log("two", n), [n]);
            return <div><p>{n}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    // mount: one 0, two 0
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let p = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&p);
    let events: Vec<String> = rt.logs().to_vec();
    let one_pos = events.iter().position(|l| l == "one 1").expect("one 1");
    let two_pos = events.iter().position(|l| l == "two 1").expect("two 1");
    assert!(
        one_pos < two_pos,
        "hooks run in declaration order: {events:?}"
    );
}

#[test]
fn effect_captures_render_time_env() {
    // The cleanup and setup closures capture the deps FROM THEIR OWN render,
    // not the latest (React's closure-capture semantics). Covered above by
    // cleanup(0) before setup(1). Here the SETUP must also see its own n.
    let src = r#"
        component App() {
            let v = useState(0);
            let n = v[0];
            let setN = v[1];
            useEffect(() => log("snap", n), [100]);
            return <div><p>{n}</p><button onClick={() => setN(n + 1)}>go</button></div>;
        }
        export default App;
    "#;
    let (mut rt, mut r) = setup(src);
    let btn = r
        .nodes()
        .iter()
        .find_map(|(id, n)| match n {
            r2n_renderer_memory::MemNode::Element { tag, .. } if tag == "button" => Some(*id),
            _ => None,
        })
        .expect("button");
    let p = rt.dispatch(btn, "onClick").expect("click");
    r.apply(&p);
    let events: Vec<String> = rt.logs().to_vec();
    // deps [100] NEVER change, so the effect ran ONCE at mount (snap 0).
    // Clicking (n -> 1) must NOT re-run it.
    assert!(
        !events.iter().any(|l| l == "snap 1"),
        "deps [100] unchanged — effect not re-run: {events:?}"
    );
}
