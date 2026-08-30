//! Acceptance tests for M0.3-T08 diagnostics: multi-error reporting with
//! recovery, friendly messages, and rendered carets.
//!
//! Rules under test:
//! 1. On error-free sources the recovering parse yields the *identical*
//!    program to the strict parser (grammar parity — recovery must never
//!    accept or reject differently).
//! 2. Multiple errors in one pass: a statement error does not stop the
//!    parser; later errors are still reported.
//! 3. Recovery re-syncs: a valid statement after a bad one is kept, and a
//!    valid declaration after a bad one is still parsed.
//! 4. Messages are friendly: punctuation is named `` `;` ``, not `Semicolon`.
//! 5. `render` produces a caret pointing at the error column.

use r2n_parser::{parse, parse_with_recovery, TokenKind};

/// Sources that must parse identically through both parsers. Covers every
/// grammar production: imports, exports, let/const/return, ternary, binary
/// ops, unary, member/call/index, arrows (both positions), arrays, if/else
/// blocks, JSX elements with attrs, expression children, raw-text children,
/// self-closing tags, nested elements, and the real examples.
fn parity_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("counter", include_str!("../../../examples/counter.r2n")),
        ("list", include_str!("../../../examples/list.r2n")),
        ("hello", include_str!("../../../examples/hello.r2n")),
        (
            "full grammar",
            r#"
                import { useState } from "./lib";
                component Outer() {
                    let x = 1 + 2 * 3;
                    let flag = x > 4 && !false;
                    const arr = [1, 2.5, "s", true, null];
                    return <div className="wrap" enabled onClick={() => setN(n + 1)}>
                        <Header title="hi" count={x} />
                        {arr.map((item) => <li key={item}>{item}</li>)}
                        {if flag { "yes" } else { "no" }}
                        plain text child
                        <span>{x == 1 ? "a" : "b"}</span>
                    </div>;
                }
                export default Outer;
            "#,
        ),
    ]
}

#[test]
fn recovery_yields_identical_ast_on_valid_sources() {
    for (name, src) in parity_sources() {
        let strict = parse(src).unwrap_or_else(|e| panic!("{name}: strict parse failed: {e}"));
        let rec = parse_with_recovery(src)
            .unwrap_or_else(|e| panic!("{name}: recovering parse failed: {e}"));
        assert!(
            rec.errors.is_empty(),
            "{name}: recovering parse reported errors on a valid source: {:?}",
            rec.errors
        );
        assert_eq!(
            strict, rec.program,
            "{name}: recovering parser produced a different AST"
        );
    }
}

#[test]
fn collects_multiple_statement_errors_in_one_pass() {
    let src = r#"
        component A() {
            let x = ;
            let y = 2;
            return <div>{y}</div>;
        }
        export default A;
    "#;
    let rec = parse_with_recovery(src).expect("lex ok");
    assert!(!rec.errors.is_empty(), "the bad statement must be reported");
    // The error points at the right place: line 3 has `let x = ;`.
    assert_eq!(rec.errors[0].line, 3, "error must be at line 3");
    assert!(
        rec.errors[0].message.contains("expression"),
        "message was: {}",
        rec.errors[0].message
    );
}

#[test]
fn recovery_keeps_valid_statement_after_bad_one() {
    let src = r#"
        component A() {
            let x = ;
            let y = 2;
            return <div>{y}</div>;
        }
        export default A;
    "#;
    let rec = parse_with_recovery(src).expect("lex ok");
    // The component must still be in the (partial) program, and the valid
    // statements after the bad one must have been kept.
    let comp = rec
        .program
        .decls
        .iter()
        .find_map(|d| match d {
            r2n_ast::program::Decl::Component(c) => Some(c),
            _ => None,
        })
        .expect("component A survived recovery");
    let names: Vec<&str> = comp
        .body
        .iter()
        .filter_map(|s| match s {
            r2n_ast::program::Stmt::Let { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        names.contains(&"y"),
        "valid statement after the bad one must be kept; got {names:?}"
    );
    let has_return = comp
        .body
        .iter()
        .any(|s| matches!(s, r2n_ast::program::Stmt::Return(_)));
    assert!(has_return, "the return statement must be kept");
}

#[test]
fn collects_errors_across_multiple_components() {
    let src = r#"
        component A() {
            let x = ;
        }
        component B() {
            return <div/>;
        }
        export default B;
    "#;
    let rec = parse_with_recovery(src).expect("lex ok");
    assert!(
        !rec.errors.is_empty(),
        "error in component A must be reported"
    );
    // Component B must still have been parsed after A failed.
    let has_b = rec.program.decls.iter().any(|d| match d {
        r2n_ast::program::Decl::Component(c) => c.name == "B",
        _ => false,
    });
    assert!(has_b, "component B must survive A's failure");
}

#[test]
fn statement_errors_within_one_component_all_reported() {
    let src = r#"
        component A() {
            let a = 1;
            let x = ;
            let b = 2;
            let y = ;
            return <div/>;
        }
        export default A;
    "#;
    let rec = parse_with_recovery(src).expect("lex ok");
    let bad: Vec<usize> = rec
        .errors
        .iter()
        .map(|e| e.line)
        .filter(|l| *l != 0)
        .collect();
    assert_eq!(
        bad,
        vec![4, 6],
        "both bad statements (lines 4 and 6) must be reported; got lines {bad:?}"
    );
}

#[test]
fn missing_export_default_reported_once() {
    let src = "component A() { return <div/>; }";
    let rec = parse_with_recovery(src).expect("lex ok");
    assert_eq!(rec.errors.len(), 1);
    assert!(rec.errors[0].message.contains("export default"));
}

#[test]
fn messages_are_friendly() {
    let src = r#"
        component A() {
            let x = 1
            return x;
        }
        export default A;
    "#;
    let strict_err = parse(src).expect_err("missing `;` is an error");
    assert!(
        strict_err.message.contains("`;`"),
        "message must name `;` with backticks, was: {}",
        strict_err.message
    );
    assert!(
        !strict_err.message.contains("Semicolon"),
        "message must not use the Debug token name, was: {}",
        strict_err.message
    );
}

#[test]
fn describe_names_tokens_friendly() {
    assert_eq!(TokenKind::Semicolon.describe(), "`;`");
    assert_eq!(TokenKind::Arrow.describe(), "`=>`");
    assert_eq!(TokenKind::Eof.describe(), "end of file");
    assert_eq!(TokenKind::Ident("if".into()).describe(), "`if`");
    assert_eq!(TokenKind::Int(3).describe(), "number `3`");
}

#[test]
fn render_draws_caret_at_error_column() {
    let src = "component A() {\n    let x = ;\n}\nexport default A;\n";
    let err = parse(src).expect_err("bad statement");
    let rendered = err.render(src);
    // The offending line is shown...
    assert!(rendered.contains("let x = ;"), "was:\n{rendered}");
    // ...and the caret lands under the `;` (13th char of line 2). The caret
    // line carries a `  | ` gutter (line-number width + ` | `); the caret's
    // offset after that gutter must equal the `;` offset in the line text.
    let line_text = "    let x = ;";
    let semicol_col = line_text.find(';').unwrap();
    let caret_line = rendered
        .lines()
        .find(|l| l.contains('^'))
        .expect("rendered output must contain a caret line");
    // Gutter = spaces for the line number + " | ".
    let gutter_width = " ".repeat(err.line.to_string().len()) + " | ";
    let caret_in_line = caret_line
        .strip_prefix(gutter_width.as_str())
        .unwrap_or(caret_line)
        .find('^')
        .unwrap();
    assert_eq!(
        caret_in_line, semicol_col,
        "caret must align under the offending token; was:\n{rendered}"
    );
    // The line number appears in the gutter.
    assert!(
        rendered.contains("2 |"),
        "line number must be shown; was:\n{rendered}"
    );
}

#[test]
fn render_handles_multiline_sources() {
    // The error sits on line 4; the caret must render for deep positions.
    let src = "\n\ncomponent A() {\n    let x = ;\n}\nexport default A;\n";
    let err = parse(src).expect_err("must error");
    assert_eq!(err.line, 4, "error must be on line 4");
    let rendered = err.render(src);
    assert!(rendered.contains("let x = ;"), "was:\n{rendered}");
    assert!(rendered.contains('^'), "was:\n{rendered}");
}

#[test]
fn strict_parse_still_stops_at_first_error() {
    // The strict parser's semantics are unchanged: one error only.
    let src = r#"
        component A() {
            let x = ;
            let y = ;
        }
        export default A;
    "#;
    assert!(parse(src).is_err());
}
