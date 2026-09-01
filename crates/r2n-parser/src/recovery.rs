//! Multi-error diagnostics: parse with recovery.
//!
//! The strict parser (`parse`) stops at the first error — correct for the
//! compiler pipeline, where one error means no artifact. But a compiler tool
//! should tell the user about *all* their mistakes in one pass, not one per
//! edit-compile round-trip. This module runs the same grammar over a flat
//! token list with recovery: a failed statement inside a component body is
//! recorded and the parser re-syncs at the next statement boundary; a failed
//! top-level declaration is recorded and the parser re-syncs at the next
//! declaration keyword. The recovered AST must not be compiled when errors
//! exist — the output is the error list. When there are no errors, the AST
//! is byte-for-byte the same as the strict parser's (asserted in tests).

use crate::error::ParseError;
use crate::lexer::{Lexer, Token, TokenKind};
use r2n_ast::expr::{Element, Expr, Prop};
use r2n_ast::lit::Literal;
use r2n_ast::op::{BinOp, UnOp};
use r2n_ast::program::{
    ClassComponent, Component, Decl, ExportNamed, Import, Method, Program, Stmt,
};

/// The result of a recovering parse: a program (partial if any error
/// occurred) and every error found along the way.
pub struct Recovered {
    pub program: Program,
    pub errors: Vec<ParseError>,
}

impl Recovered {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Parse `src`, collecting every recoverable error instead of stopping at
/// the first. Lexer-level failures (unterminated string/comment) are still
/// fatal: there is no sane position to resume tokenization from.
pub fn parse_with_recovery(src: &str) -> Result<Recovered, ParseError> {
    let mut lexer = Lexer::new(src)?;
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token()?;
        let eof = matches!(tok.kind, TokenKind::Eof);
        tokens.push(tok);
        if eof {
            break;
        }
    }

    let mut errors = Vec::new();
    let mut program = Program::new();
    let mut i = 0usize;
    while i < tokens.len() && !matches!(tokens[i].kind, TokenKind::Eof) {
        let mut p = SpanParser {
            tokens: &tokens[i..],
            pos: 0,
            src,
            errors: &mut errors,
        };
        match p.parse_decl() {
            Ok(decl) => {
                let consumed = p.pos;
                if let Decl::ExportDefault(name) = &decl {
                    program.root = Some(name.clone());
                }
                program.decls.push(decl);
                // `consumed` is 0 only when the decl consumed nothing but
                // still succeeded — impossible for a real decl, but guard so
                // the loop always advances.
                i += consumed.max(1);
            }
            Err(err) => {
                errors.push(err);
                // Re-sync at the next declaration keyword or `;`. The token
                // at the failure point is always consumed (the error may be
                // about the current token itself, e.g. `expected an
                // identifier` — retrying from the same spot would loop).
                i += 1;
                while i < tokens.len() {
                    match &tokens[i].kind {
                        TokenKind::Semicolon | TokenKind::Eof => break,
                        TokenKind::Ident(kw)
                            if matches!(kw.as_str(), "import" | "component" | "export") =>
                        {
                            break
                        }
                        _ => i += 1,
                    }
                }
                // A `;` is a dead end, not a new declaration: consume it so
                // the next iteration sees what follows it.
                if matches!(tokens[i].kind, TokenKind::Semicolon) {
                    i += 1;
                }
            }
        }
    }

    if program.root.is_none() {
        let (l, c) = tokens.last().map(|t| (t.line, t.column)).unwrap_or((1, 0));
        errors.push(ParseError::new(l, c, "no `export default` component found"));
    }

    Ok(Recovered { program, errors })
}

/// A parser over a flat token list. Mirrors `parser::Parser`'s grammar
/// exactly (same productions, same messages) with two differences made
/// possible by the flat list: arrows are detected by direct token lookahead
/// (`arrow_follows`) instead of lexer re-lexing, and JSX text children are
/// sliced from `src` between token byte offsets — the same span the strict
/// parser's `rescan_jsx_text` produces. `errors` accumulates statement-level
/// failures inside component bodies; the caller handles declaration-level
/// re-sync.
struct SpanParser<'e, 'a> {
    tokens: &'a [Token],
    pos: usize,
    src: &'a str,
    errors: &'e mut Vec<ParseError>,
}

impl<'e, 'a> SpanParser<'e, 'a> {
    /// The token vector always ends with Eof, so `cur` never panics.
    fn cur(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn err(&self, msg: &str) -> ParseError {
        let t = self.cur();
        ParseError::new(t.line, t.column, msg.to_string())
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.cur().kind == *kind
    }

    fn bump(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Result<(), ParseError> {
        if self.cur().kind == kind {
            self.bump();
            Ok(())
        } else {
            Err(self.err(&format!(
                "expected {}, found {}",
                kind.describe(),
                self.cur().kind.describe()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        if let TokenKind::Ident(name) = &self.cur().kind {
            let n = name.clone();
            self.bump();
            Ok(n)
        } else {
            Err(self.err(&format!(
                "expected an identifier, found {}",
                self.cur().kind.describe()
            )))
        }
    }

    // ---- declarations ----

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match &self.cur().kind {
            TokenKind::Ident(kw) if kw == "import" => self.parse_import(),
            TokenKind::Ident(kw) if kw == "component" => {
                Ok(Decl::Component(self.parse_component()?))
            }
            TokenKind::Ident(kw) if kw == "class" => Ok(Decl::Class(self.parse_class()?)),
            TokenKind::Ident(kw) if kw == "export" => self.parse_export(),
            TokenKind::Ident(kw) if kw == "function" => self.parse_generator_fn(),
            _ => Err(self.err("expected a declaration (import/component/export)")),
        }
    }

    /// `function* name(params) { stmts }` (mirrors parser.rs).
    fn parse_generator_fn(&mut self) -> Result<Decl, ParseError> {
        self.bump(); // `function`
        if !self.check(&TokenKind::Star) {
            return Err(self
                .err("expected `function*` (only generator function declarations are supported)"));
        }
        self.bump(); // `*`
        let name = self.expect_ident()?;
        self.expect(TokenKind::LeftParen)?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.expect_ident()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
                params.push(self.expect_ident()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::LeftBrace)?;
        let mut body = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Decl::GeneratorFn(r2n_ast::program::GeneratorFn {
            name,
            params,
            body,
        }))
    }

    fn parse_class(&mut self) -> Result<ClassComponent, ParseError> {
        self.bump(); // "class"
        let name = self.expect_ident()?;
        if !matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "extends") {
            return Err(self.err("expected `extends` after class name"));
        }
        self.bump();
        // `extends X` — Component (React base) or any base class.
        let extends = match &self.cur().kind {
            TokenKind::Ident(kw) => {
                let e = kw.clone();
                self.bump();
                Some(e)
            }
            _ => None,
        };
        self.expect(TokenKind::LeftBrace)?;
        let mut state = None;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if let TokenKind::Ident(field) = &self.cur().kind {
                let fname = field.clone();
                self.bump();
                if self.check(&TokenKind::Equals) {
                    if fname != "state" {
                        return Err(self.err("only `state` may be a class field"));
                    }
                    self.bump();
                    let value = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon)?;
                    state = Some(value);
                    continue;
                }
                if self.check(&TokenKind::LeftParen) {
                    self.bump(); // '('
                    let mut params = Vec::new();
                    if !self.check(&TokenKind::RightParen) {
                        params.push(self.expect_ident()?);
                        while self.check(&TokenKind::Comma) {
                            self.bump();
                            params.push(self.expect_ident()?);
                        }
                    }
                    self.expect(TokenKind::RightParen)?;
                    self.expect(TokenKind::LeftBrace)?;
                    let mut body = Vec::new();
                    while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
                        body.push(self.parse_stmt()?);
                    }
                    self.expect(TokenKind::RightBrace)?;
                    methods.push(Method {
                        name: fname,
                        params,
                        body,
                    });
                    continue;
                }
                return Err(self.err("expected `=` (field) or `(` (method) after class member"));
            }
            return Err(self.err("expected a class member"));
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(ClassComponent {
            name,
            extends,
            state,
            methods,
        })
    }

    fn parse_import(&mut self) -> Result<Decl, ParseError> {
        self.bump(); // "import"
        let mut import = Import {
            default_: None,
            named: Vec::new(),
            namespace: None,
            path: String::new(),
        };
        match &self.cur().kind {
            // `import "path";` — side-effect only.
            TokenKind::String(_) => {
                import.path = self.expect_string()?;
            }
            // `import { a, b as c } from "path";`
            TokenKind::LeftBrace => {
                self.bump();
                self.parse_named_imports(&mut import.named)?;
                self.expect_from()?;
                import.path = self.expect_string()?;
            }
            // `import * as ns from "path";`
            TokenKind::Star => {
                self.bump();
                self.expect_keyword("as")?;
                import.namespace = Some(self.expect_ident()?);
                self.expect_from()?;
                import.path = self.expect_string()?;
            }
            // `import Def from "path";` — optionally combined with a named or
            // namespace clause: `import Def, { a } from ...` or
            // `import Def, * as ns from ...`.
            _ => {
                import.default_ = Some(self.expect_ident()?);
                if self.check(&TokenKind::Comma) {
                    self.bump();
                    match &self.cur().kind {
                        TokenKind::LeftBrace => {
                            self.bump();
                            self.parse_named_imports(&mut import.named)?;
                        }
                        TokenKind::Star => {
                            self.bump();
                            self.expect_keyword("as")?;
                            import.namespace = Some(self.expect_ident()?);
                        }
                        _ => return Err(self.err("expected `{` or `*` after `,`")),
                    }
                }
                self.expect_from()?;
                import.path = self.expect_string()?;
            }
        }
        self.expect(TokenKind::Semicolon)?;
        Ok(Decl::Import(import))
    }

    /// `{ a, b as c }` — named import bindings as `(imported, local)` pairs.
    /// The opening `{` must already be consumed; consumes the closing `}`.
    fn parse_named_imports(&mut self, out: &mut Vec<(String, String)>) -> Result<(), ParseError> {
        while !self.check(&TokenKind::RightBrace) {
            let imported = self.expect_ident()?;
            let local = if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "as") {
                self.bump();
                self.expect_ident()?
            } else {
                imported.clone()
            };
            out.push((imported, local));
            if self.check(&TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBrace)
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        if matches!(&self.cur().kind, TokenKind::Ident(k) if k == kw) {
            self.bump();
            Ok(())
        } else {
            Err(self.err(&format!("expected `{kw}`")))
        }
    }

    fn expect_from(&mut self) -> Result<(), ParseError> {
        self.expect_keyword("from")
    }

    fn expect_string(&mut self) -> Result<String, ParseError> {
        if let TokenKind::String(s) = &self.cur().kind {
            let s = s.clone();
            self.bump();
            Ok(s)
        } else {
            Err(self.err("expected a module specifier string"))
        }
    }

    fn parse_export(&mut self) -> Result<Decl, ParseError> {
        self.bump(); // "export"
        match &self.cur().kind {
            TokenKind::Ident(kw) if kw == "default" => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportDefault(name))
            }
            // `export { a, b as c };` — named exports of module-level
            // declarations (components, classes, generator fns), M2-T09.
            TokenKind::LeftBrace => {
                self.bump();
                let mut names = Vec::new();
                while !self.check(&TokenKind::RightBrace) {
                    let local = self.expect_ident()?;
                    let exported =
                        if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "as") {
                            self.bump();
                            self.expect_ident()?
                        } else {
                            local.clone()
                        };
                    names.push((local, exported));
                    if self.check(&TokenKind::Comma) {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RightBrace)?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportNamed(ExportNamed { names }))
            }
            _ => Err(self.err("expected `default` or `{` after `export`")),
        }
    }

    fn parse_component(&mut self) -> Result<Component, ParseError> {
        self.bump(); // "component"
        let name = self.expect_ident()?;
        self.expect(TokenKind::LeftParen)?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.expect_ident()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
                params.push(self.expect_ident()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::LeftBrace)?;
        let mut body = Vec::new();
        // Statement-level recovery: record each failed statement, re-sync at
        // the next statement start (`let`/`const`/`return`) or the closing
        // `}`. Statements that parse cleanly are kept, so a later valid
        // statement is not lost to an earlier bad one.
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            let before = self.pos;
            match self.parse_stmt() {
                Ok(stmt) => body.push(stmt),
                Err(stmt_err) => {
                    self.errors.push(stmt_err);
                    // Re-sync: skip tokens until a fresh statement keyword,
                    // a `;` (end of the bad statement's remains), or the
                    // component's closing `}`.
                    self.pos = self.pos.max(before); // ensure progress
                    self.sync_to_stmt_boundary();
                    // A `;` ends the bad statement: consume it so the next
                    // iteration starts at a real statement.
                    if self.check(&TokenKind::Semicolon) {
                        self.bump();
                    }
                }
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Component { name, params, body })
    }

    /// Skip tokens until a plausible statement/component boundary.
    fn sync_to_stmt_boundary(&mut self) {
        loop {
            match &self.cur().kind {
                TokenKind::Eof | TokenKind::Semicolon => return,
                TokenKind::RightBrace => return,
                TokenKind::Ident(kw) if matches!(kw.as_str(), "let" | "const" | "return") => return,
                _ => self.bump(),
            }
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match &self.cur().kind {
            TokenKind::Ident(kw) if kw == "let" => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Let { name, value })
            }
            TokenKind::Ident(kw) if kw == "const" => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Const { name, value })
            }
            TokenKind::Ident(kw) if kw == "return" => {
                self.bump();
                let value = self.parse_expr()?;
                // In JSX body form a trailing `;` is optional (strict parity).
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                }
                Ok(Stmt::Return(value))
            }
            _ => {
                let value = self.parse_expr()?;
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                }
                Ok(Stmt::Expr(value))
            }
        }
    }

    // ---- expressions (precedence climbing, mirroring parser.rs) ----

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_assign()
    }

    /// `target = value` — right-associative (mirrors parser.rs).
    fn parse_assign(&mut self) -> Result<Expr, ParseError> {
        let target = self.parse_ternary()?;
        if self.check(&TokenKind::Equals) {
            let is_assignable = matches!(&target, Expr::Ident { .. } | Expr::Member { .. });
            if !is_assignable {
                return Err(self.err("assignment target must be an identifier or a member access"));
            }
            self.bump();
            let value = self.parse_assign()?;
            return Ok(Expr::Assign {
                target: Box::new(target),
                value: Box::new(value),
            });
        }
        Ok(target)
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let cond = self.parse_or()?;
        if self.check(&TokenKind::Question) {
            self.bump();
            let then = self.parse_ternary()?;
            self.expect(TokenKind::Colon)?;
            let else_ = self.parse_ternary()?;
            Ok(Expr::Ternary {
                cond: Box::new(cond),
                then: Box::new(then),
                else_: Box::new(else_),
            })
        } else {
            Ok(cond)
        }
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.check(&TokenKind::PipePipe) {
            self.bump();
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality()?;
        while self.check(&TokenKind::AmpAmp) {
            self.bump();
            let right = self.parse_equality()?;
            left = Expr::Binary {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match &self.cur().kind {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::BangEq => BinOp::Neq,
                TokenKind::EqEqEq => BinOp::StrictEq,
                TokenKind::BangEqEq => BinOp::StrictNeq,
                _ => break,
            };
            self.bump();
            let right = self.parse_comparison()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match &self.cur().kind {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::LtEq => BinOp::Le,
                TokenKind::GtEq => BinOp::Ge,
                _ => break,
            };
            self.bump();
            let right = self.parse_additive()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match &self.cur().kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_multiplicative()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match &self.cur().kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.bump();
            let right = self.parse_unary()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match &self.cur().kind {
            TokenKind::Minus => {
                self.bump();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Bang => {
                self.bump();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            match &self.cur().kind {
                TokenKind::Dot => {
                    self.bump();
                    let prop = self.expect_ident()?;
                    expr = Expr::Member {
                        base: Box::new(expr),
                        prop,
                    };
                }
                TokenKind::LeftParen => {
                    if self.arrow_follows() {
                        // `(params) => body` as the sole call argument,
                        // e.g. `items.map((x) => <li/>)`.
                        self.bump(); // '('
                        let mut params = Vec::new();
                        if !self.check(&TokenKind::RightParen) {
                            params.push(self.expect_ident()?);
                            while self.check(&TokenKind::Comma) {
                                self.bump();
                                params.push(self.expect_ident()?);
                            }
                        }
                        self.expect(TokenKind::RightParen)?;
                        self.expect(TokenKind::Arrow)?;
                        let body = self.parse_arrow_body()?;
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args: vec![Expr::Arrow {
                                params,
                                body: Box::new(body),
                                async_: false,
                            }],
                        };
                        self.expect(TokenKind::RightParen)?;
                    } else {
                        self.bump();
                        let mut args = Vec::new();
                        if !self.check(&TokenKind::RightParen) {
                            args.push(self.parse_expr()?);
                            while self.check(&TokenKind::Comma) {
                                self.bump();
                                args.push(self.parse_expr()?);
                            }
                        }
                        self.expect(TokenKind::RightParen)?;
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args,
                        };
                    }
                }
                TokenKind::LeftBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    self.expect(TokenKind::RightBracket)?;
                    expr = Expr::Call {
                        callee: Box::new(Expr::Member {
                            base: Box::new(expr),
                            prop: "get".to_string(),
                        }),
                        args: vec![idx],
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// `( ident (, ident)* ) =>` starting at the current `(` — the
    /// token-list equivalent of the strict parser's lexer lookahead.
    /// Does `async` here begin an async arrow — `async (params) => ` or
    /// `async ident => `? (Token-vec lookahead; mirrors parser.rs.)
    fn async_arrow_follows(&self) -> bool {
        let mut j = self.pos + 1;
        match self.tokens.get(j).map(|t| &t.kind) {
            Some(TokenKind::LeftParen) => {
                let mut depth = 1;
                loop {
                    j += 1;
                    match self.tokens.get(j).map(|t| &t.kind) {
                        Some(TokenKind::LeftParen) => depth += 1,
                        Some(TokenKind::RightParen) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => return false,
                    }
                }
                matches!(
                    self.tokens.get(j + 1).map(|t| &t.kind),
                    Some(TokenKind::Arrow)
                )
            }
            Some(TokenKind::Ident(_)) => matches!(
                self.tokens.get(j + 1).map(|t| &t.kind),
                Some(TokenKind::Arrow)
            ),
            _ => false,
        }
    }

    fn arrow_follows(&self) -> bool {
        if !self.check(&TokenKind::LeftParen) {
            return false;
        }
        let mut j = self.pos + 1;
        loop {
            match self.tokens.get(j).map(|t| &t.kind) {
                Some(TokenKind::Ident(_)) | Some(TokenKind::Comma) => j += 1,
                Some(TokenKind::RightParen) => {
                    return matches!(
                        self.tokens.get(j + 1).map(|t| &t.kind),
                        Some(TokenKind::Arrow)
                    );
                }
                _ => return false,
            }
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.cur().kind.clone() {
            TokenKind::Int(i) => {
                self.bump();
                Ok(Expr::Literal(Literal::Int(i)))
            }
            TokenKind::Float(f) => {
                self.bump();
                Ok(Expr::Literal(Literal::Float(f)))
            }
            TokenKind::String(s) => {
                self.bump();
                Ok(Expr::Literal(Literal::String(s)))
            }
            TokenKind::Ident(name) if name == "true" => {
                self.bump();
                Ok(Expr::Literal(Literal::Bool(true)))
            }
            TokenKind::Ident(name) if name == "false" => {
                self.bump();
                Ok(Expr::Literal(Literal::Bool(false)))
            }
            TokenKind::Ident(name) if name == "new" => {
                // `new Callee(args...)` (mirrors parser.rs).
                self.bump();
                let callee = self.expect_ident()?;
                self.expect(TokenKind::LeftParen)?;
                let mut args = Vec::new();
                if !self.check(&TokenKind::RightParen) {
                    args.push(self.parse_expr()?);
                    while self.check(&TokenKind::Comma) {
                        self.bump();
                        args.push(self.parse_expr()?);
                    }
                }
                self.expect(TokenKind::RightParen)?;
                Ok(Expr::New {
                    callee: Box::new(Expr::Ident {
                        name: callee,
                        is_component: false,
                    }),
                    args,
                })
            }
            TokenKind::Ident(name) if name == "null" => {
                self.bump();
                Ok(Expr::Literal(Literal::Null))
            }
            TokenKind::LeftParen => {
                if self.arrow_follows() {
                    self.bump(); // '('
                    let mut params = Vec::new();
                    if !self.check(&TokenKind::RightParen) {
                        params.push(self.expect_ident()?);
                        while self.check(&TokenKind::Comma) {
                            self.bump();
                            params.push(self.expect_ident()?);
                        }
                    }
                    self.expect(TokenKind::RightParen)?;
                    self.expect(TokenKind::Arrow)?;
                    let body = self.parse_arrow_body()?;
                    Ok(Expr::Arrow {
                        params,
                        body: Box::new(body),
                        async_: false,
                    })
                } else {
                    self.bump();
                    let e = self.parse_expr()?;
                    self.expect(TokenKind::RightParen)?;
                    Ok(e)
                }
            }
            TokenKind::Ident(name) if name == "async" && self.async_arrow_follows() => {
                // `async (params) => body` / `async x => body` (mirrors parser.rs).
                self.bump(); // `async`
                let mut params = Vec::new();
                if self.check(&TokenKind::LeftParen) {
                    self.bump();
                    if !self.check(&TokenKind::RightParen) {
                        params.push(self.expect_ident()?);
                        while self.check(&TokenKind::Comma) {
                            self.bump();
                            params.push(self.expect_ident()?);
                        }
                    }
                    self.expect(TokenKind::RightParen)?;
                } else {
                    params.push(self.expect_ident()?);
                }
                self.expect(TokenKind::Arrow)?;
                let body = self.parse_arrow_body()?;
                Ok(Expr::Arrow {
                    params,
                    body: Box::new(body),
                    async_: true,
                })
            }
            TokenKind::Ident(name) if name == "await" => {
                // `await expr` (mirrors parser.rs).
                self.bump();
                let value = self.parse_expr()?;
                Ok(Expr::Await {
                    value: Box::new(value),
                    from_return: false,
                })
            }
            TokenKind::Ident(name) if name == "yield" => {
                // `yield` / `yield expr` (mirrors parser.rs).
                self.bump();
                let value = if self.check(&TokenKind::Semicolon)
                    || self.check(&TokenKind::RightBrace)
                    || self.check(&TokenKind::Eof)
                {
                    None
                } else {
                    Some(Box::new(self.parse_expr()?))
                };
                Ok(Expr::Yield {
                    value,
                    from_return: false,
                })
            }
            TokenKind::Ident(name) if name == "throw" => {
                // `throw value` (mirrors parser.rs).
                self.bump();
                let value = self.parse_expr()?;
                Ok(Expr::Throw(Box::new(value)))
            }
            TokenKind::Ident(name) if name == "try" => self.parse_try(),
            TokenKind::Ident(name) if name == "if" => {
                self.bump();
                let cond = self.parse_expr()?;
                self.expect(TokenKind::LeftBrace)?;
                let then = self.parse_expr()?;
                self.expect(TokenKind::RightBrace)?;
                if !matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "else") {
                    return Err(self.err("expected `else` after `if` block"));
                }
                self.bump();
                self.expect(TokenKind::LeftBrace)?;
                let else_ = self.parse_expr()?;
                self.expect(TokenKind::RightBrace)?;
                Ok(Expr::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    else_: Box::new(else_),
                })
            }
            TokenKind::Ident(name) if name == "import" => {
                // `import("path")` — dynamic import (M2-T09). Lookahead over
                // the token slice: dynamic import only when `(` follows
                // `import`; a bare `import` in expression position stays an
                // ordinary identifier.
                let is_call = matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::LeftParen)
                );
                if is_call {
                    self.bump(); // `import`
                    self.expect(TokenKind::LeftParen)?;
                    let specifier = match &self.cur().kind {
                        TokenKind::String(s) => s.clone(),
                        _ => {
                            return Err(
                                self.err("dynamic import specifier must be a string literal")
                            )
                        }
                    };
                    self.bump();
                    self.expect(TokenKind::RightParen)?;
                    Ok(Expr::DynImport { specifier })
                } else {
                    self.bump();
                    Ok(Expr::Ident {
                        name: "import".to_string(),
                        is_component: false,
                    })
                }
            }
            TokenKind::Ident(name) => {
                self.bump();
                let is_component = name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false);
                Ok(Expr::Ident { name, is_component })
            }
            TokenKind::LeftBracket => self.parse_array(),
            TokenKind::Lt => self.parse_element(),
            other => Err(self.err(&format!("unexpected {} in expression", other.describe()))),
        }
    }

    fn parse_arrow_body(&mut self) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::LeftBrace) {
            self.bump();
            let mut stmts = Vec::new();
            while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
                // `return expr;` is terminal: the expr is the block's VALUE
                // (cleanup-returning effect arrows). Mirrors parser.rs.
                if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "return") {
                    self.bump();
                    let v = self.parse_expr()?;
                    // `return await p` marker (mirrors parser.rs).
                    let v = match v {
                        Expr::Await { value, .. } => Expr::Await {
                            value,
                            from_return: true,
                        },
                        Expr::Yield { value, .. } => Expr::Yield {
                            value,
                            from_return: true,
                        },
                        other => other,
                    };
                    stmts.push(v);
                    if self.check(&TokenKind::Semicolon) {
                        self.bump();
                    }
                    break;
                }
                // `let`/`const` inside a block-bodied arrow: a scoped local
                // (mirrors parser.rs).
                if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "let" || kw == "const")
                {
                    self.bump();
                    let name = self.expect_ident()?;
                    self.expect(TokenKind::Equals)?;
                    let value = self.parse_expr()?;
                    if self.check(&TokenKind::Semicolon) {
                        self.bump();
                    }
                    stmts.push(Expr::Assign {
                        target: Box::new(Expr::Ident {
                            name,
                            is_component: false,
                        }),
                        value: Box::new(value),
                    });
                    continue;
                }
                stmts.push(self.parse_expr()?);
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                }
            }
            self.expect(TokenKind::RightBrace)?;
            Ok(Expr::Block(stmts))
        } else {
            self.parse_expr()
        }
    }

    /// Parse `{ stmts }` (mirrors parser.rs); consumes both braces.
    fn parse_block_stmts(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.expect(TokenKind::LeftBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "return") {
                self.bump();
                let v = self.parse_expr()?;
                // `return await p` marker (mirrors parser.rs).
                let v = match v {
                    Expr::Await { value, .. } => Expr::Await {
                        value,
                        from_return: true,
                    },
                    Expr::Yield { value, .. } => Expr::Yield {
                        value,
                        from_return: true,
                    },
                    other => other,
                };
                stmts.push(v);
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                }
                break;
            }
            if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "let" || kw == "const") {
                self.bump();
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                }
                stmts.push(Expr::Assign {
                    target: Box::new(Expr::Ident {
                        name,
                        is_component: false,
                    }),
                    value: Box::new(value),
                });
                continue;
            }
            stmts.push(self.parse_expr()?);
            if self.check(&TokenKind::Semicolon) {
                self.bump();
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(stmts)
    }

    /// `try { } catch (e) { } finally { }` (mirrors parser.rs).
    fn parse_try(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // `try`
        let block = self.parse_block_stmts()?;
        let (mut catch_param, mut catch, mut finally) = (None, None, None);
        if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "catch") {
            self.bump();
            if self.check(&TokenKind::LeftParen) {
                self.bump();
                catch_param = Some(self.expect_ident()?);
                self.expect(TokenKind::RightParen)?;
            }
            catch = Some(self.parse_block_stmts()?);
        }
        if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "finally") {
            self.bump();
            finally = Some(self.parse_block_stmts()?);
        }
        if catch.is_none() && finally.is_none() {
            return Err(self.err("try requires a catch or finally block"));
        }
        Ok(Expr::Try {
            block,
            catch_param,
            catch,
            finally,
        })
    }

    fn parse_array(&mut self) -> Result<Expr, ParseError> {
        self.expect(TokenKind::LeftBracket)?;
        let mut items = Vec::new();
        if !self.check(&TokenKind::RightBracket) {
            items.push(self.parse_expr()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
                if self.check(&TokenKind::RightBracket) {
                    break; // trailing comma
                }
                items.push(self.parse_expr()?);
            }
        }
        self.expect(TokenKind::RightBracket)?;
        Ok(Expr::Array(items))
    }

    fn parse_element(&mut self) -> Result<Expr, ParseError> {
        self.expect(TokenKind::Lt)?;
        let is_fragment = self.check(&TokenKind::Gt);
        let mut tag = if is_fragment {
            String::new()
        } else {
            self.expect_ident()?
        };
        // Dotted member tags (mirrors parser.rs): `<Ctx.Provider>`.
        if !is_fragment && self.check(&TokenKind::Dot) {
            self.bump();
            let member = self.expect_ident()?;
            tag = format!("{tag}.{member}");
        }
        // Mirrors parser.rs: a dotted tag is a member-access element, never a
        // host element, regardless of the base's case.
        let is_component = !is_fragment
            && (tag.contains('.')
                || tag
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false));
        let mut props = Vec::new();
        loop {
            match &self.cur().kind {
                TokenKind::Slash => {
                    self.bump();
                    self.expect(TokenKind::Gt)?;
                    return Ok(Expr::Element(Element {
                        tag,
                        is_component,
                        props,
                        children: Vec::new(),
                    }));
                }
                TokenKind::Gt => {
                    self.bump();
                    break;
                }
                TokenKind::Ident(name) => {
                    let pname = name.clone();
                    self.bump();
                    if self.check(&TokenKind::Equals) {
                        self.bump();
                        if self.check(&TokenKind::LeftBrace) {
                            self.bump();
                            let value = self.parse_expr()?;
                            self.expect(TokenKind::RightBrace)?;
                            props.push(Prop {
                                name: pname,
                                value: Some(value),
                            });
                        } else if let TokenKind::String(s) = &self.cur().kind {
                            let v = s.clone();
                            self.bump();
                            props.push(Prop {
                                name: pname,
                                value: Some(Expr::Literal(Literal::String(v))),
                            });
                        } else {
                            return Err(self.err("expected `{...}` or string after `=`"));
                        }
                    } else {
                        props.push(Prop {
                            name: pname,
                            value: None,
                        });
                    }
                }
                _ => return Err(self.err("unexpected token in element attributes")),
            }
        }
        let mut children = Vec::new();
        while !self.check(&TokenKind::LtSlash) && !self.check(&TokenKind::Eof) {
            if self.check(&TokenKind::LeftBrace) {
                self.bump();
                let e = self.parse_expr()?;
                self.expect(TokenKind::RightBrace)?;
                children.push(e);
            } else if self.check(&TokenKind::Lt) {
                children.push(self.parse_element()?);
            } else {
                // JSX text child: slice the source from the pending token's
                // byte offset up to the next `{` or `<` — the same span
                // `Lexer::rescan_jsx_text` yields in the strict parser.
                let start = self.cur().offset;
                let bytes = self.src.as_bytes();
                let mut end = start;
                while end < bytes.len() {
                    match bytes[end] {
                        b'{' | b'<' => break,
                        _ => end += 1,
                    }
                }
                let text = self.src[start..end].trim().to_string();
                // Skip every token inside the text span.
                while self.pos < self.tokens.len()
                    && !matches!(self.tokens[self.pos].kind, TokenKind::Eof)
                    && self.tokens[self.pos].offset < end
                {
                    self.pos += 1;
                }
                if text.is_empty() {
                    // Whitespace-only child: JSX drops it. The loop above
                    // may have stopped exactly at `</`; `check` handles it.
                    continue;
                }
                children.push(Expr::Literal(Literal::String(text)));
            }
        }
        self.expect(TokenKind::LtSlash)?;
        if is_fragment {
            // `</>` — fragment close has no tag (mirrors parser.rs).
            self.expect(TokenKind::Gt)?;
            return Ok(Expr::Element(Element {
                tag,
                is_component,
                props,
                children,
            }));
        }
        let mut close = self.expect_ident()?;
        if self.check(&TokenKind::Dot) {
            self.bump();
            let m = self.expect_ident()?;
            close = format!("{close}.{m}");
        }
        if close != tag {
            return Err(self.err(&format!(
                "mismatched closing tag: expected `{tag}`, found `{close}`"
            )));
        }
        self.expect(TokenKind::Gt)?;
        Ok(Expr::Element(Element {
            tag,
            is_component,
            props,
            children,
        }))
    }
}
