//! R2N recursive-descent parser.

mod error;
mod lexer;
mod parser;
mod recovery;

pub use error::ParseError;
pub use lexer::{Lexer, Token, TokenKind};
pub use parser::Parser;
pub use recovery::{parse_with_recovery, Recovered};

/// Parse a complete R2N source program string into an AST.
pub fn parse(src: &str) -> Result<r2n_ast::program::Program, ParseError> {
    let mut p = Parser::new(src)?;
    p.parse_program()
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2n_ast::expr::Expr;

    #[test]
    fn parses_counter_component() {
        let src = r#"
            component Counter() {
                return <div className="x">{1 + 2}</div>;
            }
            export default Counter;
        "#;
        let prog = parse(src).expect("should parse");
        assert_eq!(prog.root.as_deref(), Some("Counter"));
        assert_eq!(prog.decls.len(), 2);
    }

    #[test]
    fn parses_number_and_string() {
        let src = r#"
            component App() {
                let n = 3.5;
                return <span title="hi">{n}</span>;
            }
            export default App;
        "#;
        let prog = parse(src).expect("should parse");
        assert_eq!(prog.decls.len(), 2);
    }

    #[test]
    fn reports_missing_export() {
        let src = "component A() { return <div/>; }";
        let err = parse(src);
        assert!(err.is_err());
    }

    #[test]
    fn parses_arrow_in_map() {
        let src = r#"
            component List() {
                return <ul>{items.map((x) => <li key={x}/>)}</ul>;
            }
            export default List;
        "#;
        let prog = parse(src).expect("should parse");
        // ensure it parsed as a call whose callee is a member access
        if let r2n_ast::program::Decl::Component(c) = &prog.decls[0] {
            assert!(matches!(
                c.body.last(),
                Some(r2n_ast::program::Stmt::Return(Expr::Element(_)))
            ));
        } else {
            panic!("expected component");
        }
        // The inner arrow must have been captured as an Arrow expr.
        let body = &prog.decls[0];
        if let r2n_ast::program::Decl::Component(c) = body {
            if let Some(r2n_ast::program::Stmt::Return(Expr::Element(e))) = c.body.last() {
                // The ul's single child is a Call to items.map with an Arrow.
                if let r2n_ast::expr::Expr::Call { callee, args } = &e.children[0] {
                    assert!(matches!(
                        callee.as_ref(),
                        r2n_ast::expr::Expr::Member { .. }
                    ));
                    assert!(matches!(args[0], r2n_ast::expr::Expr::Arrow { .. }));
                } else {
                    panic!("expected call to map");
                }
            }
        }
    }
}
