//! Tests for the R2N parser: it is a real recursive-descent parser, so we
//! assert both that valid programs parse and that malformed input produces
//! precise errors.

use r2n_parser::parse;

#[test]
fn parses_counter_component() {
    let src = r#"
        component Counter() {
            let count = useState(0);
            let n = count[0];
            return <div className="counter"><h1>{n}</h1></div>;
        }
        export default Counter;
    "#;
    let prog = parse(src).expect("should parse");
    assert_eq!(prog.root.as_deref(), Some("Counter"));
    assert_eq!(prog.decls.len(), 2);
}

#[test]
fn parses_arrow_in_map() {
    let src = r#"
        component List() {
            let items = ["a", "b"];
            return <ul>{items.map((x) => <li key={x}>{x}</li>)}</ul>;
        }
        export default List;
    "#;
    let prog = parse(src).expect("should parse");
    assert_eq!(prog.root.as_deref(), Some("List"));
}

#[test]
fn parses_if_else_blocks() {
    let src = r#"
        component App() {
            let n = 3;
            return <p>{if n < 5 { "small" } else { "big" }}</p>;
        }
        export default App;
    "#;
    let prog = parse(src).expect("should parse");
    assert_eq!(prog.decls.len(), 2);
}

#[test]
fn rejects_missing_export_default() {
    let src = "component A() { return <div/>; }";
    assert!(parse(src).is_err());
}

#[test]
fn rejects_unterminated_element() {
    let src = r#"
        component A() {
            return <div>;
        }
        export default A;
    "#;
    assert!(parse(src).is_err());
}

#[test]
fn reports_position_for_unexpected_token() {
    let src = r#"
        component A() {
            let x = ;
        }
        export default A;
    "#;
    let err = parse(src).expect_err("should error");
    // The error must carry a line/column (precise positioning).
    assert!(err.line >= 3);
    assert!(err.column > 0);
}
