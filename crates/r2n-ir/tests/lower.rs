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
