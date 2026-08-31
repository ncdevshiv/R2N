//! M1-T16 acceptance: StrictMode — dev-only double-invoke kept out of
//! production artifacts.
//!
//! React semantics under test:
//! 1. In DEV builds (`compile_source_dev`), effects inside `<StrictMode>`
//!    run the double-invoke: setup → cleanup → setup (log-observable).
//! 2. In PRODUCTION builds (`compile_source`), the same source runs
//!    effects ONCE and the artifact contains NO StrictMode marker.
//! 3. The production artifact JSON does not carry strict_mode.

use r2n_compiler::{compile_source, compile_source_dev};
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn setup_dev(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source_dev(src).expect("dev compile");
    assert!(template.strict_mode, "dev artifact carries strict_mode");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

fn setup_prod(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("prod compile");
    assert!(
        !template.strict_mode,
        "production artifact has no strict_mode"
    );
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

const SRC: &str = r#"
    component App() {
        useEffect(() => { log("setup"); return () => log("cleanup"); }, []);
        return <p className="out">hi</p>;
    }
    export default App;
"#;

#[test]
fn dev_double_invokes_effects() {
    let (rt, _r) = setup_dev(SRC);
    let logs = rt.logs();
    // setup -> cleanup -> setup: double invoke, two setups, one cleanup
    // BETWEEN them (the slot's armed cleanup runs on the double pass).
    let setups = logs.iter().filter(|l| *l == "setup").count();
    assert_eq!(setups, 2, "dev double-invoke: {logs:?}");
    assert!(
        logs.iter().any(|l| *l == "cleanup"),
        "cleanup ran: {logs:?}"
    );
}

#[test]
fn prod_runs_effects_once() {
    let (rt, _r) = setup_prod(SRC);
    let logs = rt.logs();
    let setups = logs.iter().filter(|l| *l == "setup").count();
    assert_eq!(setups, 1, "prod single-invoke: {logs:?}");
}

#[test]
fn production_artifact_has_no_strict_mode() {
    let src = r#"
        component App() {
            return (
                <StrictMode>
                    <p className="in">content</p>
                </StrictMode>
            );
        }
        export default App;
    "#;
    let template = compile_source(src).expect("prod compile");
    assert!(!template.strict_mode, "no dev flag in the artifact");
    // The StrictMode wrapper is transparent in production: content renders
    // directly, and no StrictMode node survives.
    let json = r2n_ir::ser::to_json(&template).expect("serialize");
    assert!(
        !json.contains("StrictMode"),
        "StrictMode node stripped from the production artifact"
    );
    let (_rt, r) = setup_prod(src);
    let tree = r.render_string();
    assert!(tree.contains("content"), "content renders: {tree}");
}

#[test]
fn dev_artifact_keeps_the_wrapper_marker() {
    let src = r#"
        component App() {
            return (
                <StrictMode>
                    <p className="in">content</p>
                </StrictMode>
            );
        }
        export default App;
    "#;
    let template = compile_source_dev(src).expect("dev compile");
    assert!(template.strict_mode, "dev flag set");
    let json = r2n_ir::ser::to_json(&template).expect("serialize");
    assert!(
        json.contains("StrictMode"),
        "dev artifact retains the marker"
    );
}
