//! Tests for the IR: lowering AST -> RuntimeTemplate and the language-neutral
//! serialization round-trip (the artifact that a conformant runtime consumes).

use r2n_compiler::compile_source;
use r2n_ir::ser;

#[test]
fn lowers_counter_to_template() {
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            return <div className="counter"><h1>{n}</h1></div>;
        }
        export default Counter;
    "#;
    let t = compile_source(src).expect("compile");
    assert_eq!(t.components.len(), 1);
    assert_eq!(t.root, 0);
    assert_eq!(t.components[0].name, "Counter");
}

#[test]
fn lowers_list_into_keyed_list_node() {
    let src = r#"
        component List() {
            let items = ["a", "b"];
            return <ul>{items.map((x) => <li key={x}>{x}</li>)}</ul>;
        }
        export default List;
    "#;
    let t = compile_source(src).expect("compile");
    // The body should be a Host <ul> whose sole child is a List node.
    let body = &t.components[0].body;
    match body {
        r2n_ir::react::ReactNode::Host { children, .. } => {
            assert_eq!(children.len(), 1);
            assert!(matches!(children[0], r2n_ir::react::ReactNode::List { .. }));
        }
        other => panic!("expected host root, got {other:?}"),
    }
}

#[test]
fn artifact_round_trips_through_json() {
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            return <div className="counter"><h1>{n}</h1></div>;
        }
        export default Counter;
    "#;
    let t = compile_source(src).expect("compile");
    let json = ser::to_json(&t).expect("serialize");
    let back = ser::from_json_bytes(json.as_bytes()).expect("deserialize");
    assert_eq!(t, back, "serialization round-trip must be lossless");
}

#[test]
fn unknown_component_is_a_lower_error() {
    let src = r#"
        component App() {
            return <Missing/>;
        }
        export default App;
    "#;
    // `Missing` is never defined -> lowering must fail (not panic, not fake).
    assert!(compile_source(src).is_err());
}

#[test]
fn artifact_carries_version_stamps() {
    // M0.3-T09: every compiled artifact carries its manifest — format
    // version + compiler version — and it survives the JSON round-trip, so
    // any consumer can check compatibility before executing.
    let src = r#"
        component App() { return <div>{"x"}</div>; }
        export default App;
    "#;
    let t = compile_source(src).expect("compile");
    assert_eq!(
        t.manifest.format_version,
        r2n_ir::runtime::ARTIFACT_FORMAT_VERSION
    );
    assert_eq!(
        t.manifest.compiler_version,
        (0, 1, 0),
        "workspace version stamp"
    );

    let json = ser::to_json(&t).expect("serialize");
    assert!(
        json.contains("\"format_version\""),
        "manifest serialized: {json}"
    );
    let back = ser::from_json_bytes(json.as_bytes()).expect("deserialize");
    assert_eq!(back.manifest, t.manifest, "stamps round-trip");
}

#[test]
fn compiled_ir_is_deterministic_snapshots_are_stable() {
    // Snapshot-test foundation (M0.3-T09): the same source compiles to a
    // byte-identical JSON artifact every time — no map-iteration order,
    // pointer values, or ambient state leaking in. Golden files can diff
    // against this output.
    let src = r#"
        component App() {
            let s = useState(0);
            let n = s[0];
            return <ul>{["a","b"].map((x) => <li key={x}>{x}</li>)}</ul>;
        }
        export default App;
    "#;
    let j1 = ser::to_json(&compile_source(src).expect("c1")).expect("s1");
    let j2 = ser::to_json(&compile_source(src).expect("c2")).expect("s2");
    assert_eq!(j1, j2, "compiled IR must be deterministic");
    assert!(
        j1.contains("\"key_expr\""),
        "IR structure is inspectable: {j1}"
    );
}
