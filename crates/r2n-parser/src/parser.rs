//! Recursive-descent parser: tokens -> `r2n_ast::Program`.
//!
//! Grammar (subset, precedence climbing):
//!
//! ```text
//! program     := decl* EOF
//! decl        := import | component | export-default
//! import      := "import" "{" ident ("," ident)* "}" "from" string ";"
//! component   := "component" ident "(" params? ")" "{" stmt* "}"
//! params      := ident ("," ident)*
//! stmt        := ("let" | "const") ident "=" expr ";"
//!              | "return" expr ";"
//! export      := "export" "default" ident ";"
//!
//! expr        := ternary
//! ternary     := or ("?" ternary ":" ternary)?
//! or          := and ("||" and)*
//! and         := equality ("&&" equality)*
//! equality    := comparison (("=="|"!=") comparison)*
//! comparison  := additive (("<"|">"|"<="|">=") additive)*
//! additive    := multiplicative (("+"|"-") multiplicative)*
//! multiplicative := unary (("*"|"/"|"%") unary)*
//! unary       := ("-"|"!") unary | postfix
//! postfix     := primary ("." ident | "(" args? ")" | "[" expr "]")*
//! primary     := literal | ident | arrow | "(" expr ")"
//!              | element | array
//! element     := "<" tag (attr)* ("/>" | ">" children "</" tag ">")
//! attr        := ident ("=" expr)?   (where expr may be `{...}`)
//! arrow       := "(" params? ")" "=>" expr
//! ```

use crate::error::ParseError;
use crate::lexer::{Lexer, Token, TokenKind};
use r2n_ast::expr::{Element, Expr, Prop};
use r2n_ast::op::{BinOp, UnOp};
use r2n_ast::program::{ClassComponent, Component, Decl, Import, Method, Program, Stmt};

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Result<Self, ParseError> {
        let mut lexer = Lexer::new(src)?;
        let current = lexer.next_token()?;
        Ok(Self { lexer, current })
    }

    fn advance(&mut self) -> Result<Token, ParseError> {
        let prev = self.current.clone();
        self.current = self.lexer.next_token()?;
        Ok(prev)
    }

    fn pos(&self) -> (usize, usize) {
        (self.current.line, self.current.column)
    }

    fn err(&self, msg: &str) -> ParseError {
        let (l, c) = self.pos();
        ParseError::new(l, c, msg.to_string())
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.current.kind == *kind
    }

    fn expect(&mut self, kind: TokenKind) -> Result<(), ParseError> {
        if self.current.kind == kind {
            self.advance()?;
            Ok(())
        } else {
            Err(self.err(&format!(
                "expected {}, found {}",
                kind.describe(),
                self.current.kind.describe()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        if let TokenKind::Ident(name) = &self.current.kind {
            let n = name.clone();
            self.advance()?;
            Ok(n)
        } else {
            Err(self.err(&format!(
                "expected an identifier, found {}",
                self.current.kind.describe()
            )))
        }
    }

    fn is_component_name(name: &str) -> bool {
        name.chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false)
    }

    // ---- program ----

    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut program = Program::new();
        while !self.check(&TokenKind::Eof) {
            let decl = self.parse_decl()?;
            if let Decl::ExportDefault(name) = &decl {
                program.root = Some(name.clone());
            }
            program.decls.push(decl);
        }
        if program.root.is_none() {
            return Err(self.err("no `export default` component found"));
        }
        Ok(program)
    }

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(kw) if kw == "import" => self.parse_import(),
            TokenKind::Ident(kw) if kw == "component" => {
                Ok(Decl::Component(self.parse_component()?))
            }
            TokenKind::Ident(kw) if kw == "class" => Ok(Decl::Class(self.parse_class()?)),
            TokenKind::Ident(kw) if kw == "export" => self.parse_export(),
            _ => Err(self.err("expected a declaration (import/component/export)")),
        }
    }

    fn parse_class(&mut self) -> Result<ClassComponent, ParseError> {
        self.advance()?; // "class"
        let name = self.expect_ident()?;
        if !matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "extends") {
            return Err(self.err("expected `extends` after class name"));
        }
        self.advance()?;
        if self.check(&TokenKind::LeftBrace) {
            return Err(self.err("expected `extends` after class name"));
        }
        // `extends X` — Component (React base) or any base class.
        let extends = match &self.current.kind {
            TokenKind::Ident(kw) => {
                let e = kw.clone();
                self.advance()?;
                Some(e)
            }
            _ => None,
        };
        self.expect(TokenKind::LeftBrace)?;
        let mut state = None;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            // Field: `state = expr;` (an ident NOT followed by `(`).
            if let TokenKind::Ident(field) = &self.current.kind {
                let fname = field.clone();
                self.advance()?;
                if self.check(&TokenKind::Equals) {
                    if fname != "state" {
                        return Err(self.err("only `state` may be a class field"));
                    }
                    self.advance()?;
                    let value = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon)?;
                    state = Some(value);
                    continue;
                }
                if self.check(&TokenKind::LeftParen) {
                    // Method: `name(params) { body }`.
                    self.advance()?; // '('
                    let mut params = Vec::new();
                    if !self.check(&TokenKind::RightParen) {
                        params.push(self.expect_ident()?);
                        while self.check(&TokenKind::Comma) {
                            self.advance()?;
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
        self.expect(TokenKind::Ident("import".to_string()))?;
        self.expect(TokenKind::LeftBrace)?;
        let mut names = Vec::new();
        if !self.check(&TokenKind::RightBrace) {
            names.push(self.expect_ident()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                names.push(self.expect_ident()?);
            }
        }
        self.expect(TokenKind::RightBrace)?;
        if !matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "from") {
            return Err(self.err("expected `from` after import names"));
        }
        self.advance()?;
        let path = match &self.current.kind {
            TokenKind::String(s) => s.clone(),
            _ => return Err(self.err("expected module path string")),
        };
        self.advance()?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Decl::Import(Import { names, path }))
    }

    fn parse_export(&mut self) -> Result<Decl, ParseError> {
        self.expect(TokenKind::Ident("export".to_string()))?;
        if !matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "default") {
            return Err(self.err("only `export default` is supported"));
        }
        self.advance()?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::Semicolon)?;
        Ok(Decl::ExportDefault(name))
    }

    fn parse_component(&mut self) -> Result<Component, ParseError> {
        self.expect(TokenKind::Ident("component".to_string()))?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::LeftParen)?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.expect_ident()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                params.push(self.expect_ident()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::LeftBrace)?;
        let mut body = Vec::new();
        while !self.check(&TokenKind::RightBrace) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Component { name, params, body })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(kw) if kw == "let" => {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Let { name, value })
            }
            TokenKind::Ident(kw) if kw == "const" => {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Const { name, value })
            }
            TokenKind::Ident(kw) if kw == "return" => {
                self.advance()?;
                let value = self.parse_expr()?;
                // In JSX body form, a trailing `;` is optional.
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                }
                Ok(Stmt::Return(value))
            }
            // Bare expression statement (side effects, e.g. `useEffect(...);`).
            _ => {
                let value = self.parse_expr()?;
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                }
                Ok(Stmt::Expr(value))
            }
        }
    }

    // ---- expressions (precedence climbing) ----

    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_assign()
    }

    /// `target = value` — right-associative, lowest precedence. Target must
    /// be an identifier or a member access.
    fn parse_assign(&mut self) -> Result<Expr, ParseError> {
        let target = self.parse_ternary()?;
        if self.check(&TokenKind::Equals) {
            let is_assignable = matches!(&target, Expr::Ident { .. } | Expr::Member { .. });
            if !is_assignable {
                return Err(self.err("assignment target must be an identifier or a member access"));
            }
            self.advance()?;
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
            self.advance()?;
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
            self.advance()?;
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
            self.advance()?;
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
            let op = match &self.current.kind {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::BangEq => BinOp::Neq,
                TokenKind::EqEqEq => BinOp::StrictEq,
                TokenKind::BangEqEq => BinOp::StrictNeq,
                _ => break,
            };
            self.advance()?;
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
            let op = match &self.current.kind {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::LtEq => BinOp::Le,
                TokenKind::GtEq => BinOp::Ge,
                _ => break,
            };
            self.advance()?;
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
            let op = match &self.current.kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance()?;
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
            let op = match &self.current.kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance()?;
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
        match &self.current.kind {
            TokenKind::Minus => {
                self.advance()?;
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Bang => {
                self.advance()?;
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
            match &self.current.kind {
                TokenKind::Dot => {
                    self.advance()?;
                    let prop = self.expect_ident()?;
                    expr = Expr::Member {
                        base: Box::new(expr),
                        prop,
                    };
                }
                TokenKind::LeftParen => {
                    // An arrow `(params) => body` can appear in call position
                    // (e.g. `arr.map((x) => <li/>)`). Detect it before treating
                    // this as a function call.
                    if self.looks_like_arrow() {
                        self.advance()?; // consume '('
                        let mut params = Vec::new();
                        if !self.check(&TokenKind::RightParen) {
                            params.push(self.expect_ident()?);
                            while self.check(&TokenKind::Comma) {
                                self.advance()?;
                                params.push(self.expect_ident()?);
                            }
                        }
                        self.expect(TokenKind::RightParen)?;
                        self.expect(TokenKind::Arrow)?;
                        let body = self.parse_arrow_body()?;
                        // This arrow is the sole argument of the call we are
                        // currently parsing (e.g. `arr.map((x) => ...)`). Wrap it.
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args: vec![Expr::Arrow {
                                params,
                                body: Box::new(body),
                                async_: false,
                            }],
                        };
                        // Consume the call's own closing `)` (the `)` of `.map(`
                        // distinct from the arrow's `(x)`).
                        self.expect(TokenKind::RightParen)?;
                    } else {
                        self.advance()?;
                        let mut args = Vec::new();
                        if !self.check(&TokenKind::RightParen) {
                            args.push(self.parse_expr()?);
                            while self.check(&TokenKind::Comma) {
                                self.advance()?;
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
                    self.advance()?;
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

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match &self.current.kind {
            TokenKind::Int(i) => {
                let v = *i;
                self.advance()?;
                Ok(Expr::Literal(r2n_ast::lit::Literal::Int(v)))
            }
            TokenKind::Float(f) => {
                let v = *f;
                self.advance()?;
                Ok(Expr::Literal(r2n_ast::lit::Literal::Float(v)))
            }
            TokenKind::String(s) => {
                let v = s.clone();
                self.advance()?;
                Ok(Expr::Literal(r2n_ast::lit::Literal::String(v)))
            }
            TokenKind::Ident(name) if name == "true" => {
                self.advance()?;
                Ok(Expr::Literal(r2n_ast::lit::Literal::Bool(true)))
            }
            TokenKind::Ident(name) if name == "false" => {
                self.advance()?;
                Ok(Expr::Literal(r2n_ast::lit::Literal::Bool(false)))
            }
            TokenKind::Ident(name) if name == "new" => {
                // `new Callee(args...)` — the callee is an identifier; the
                // args are parenthesized (M2-T04).
                self.advance()?;
                let callee = self.expect_ident()?;
                self.expect(TokenKind::LeftParen)?;
                let mut args = Vec::new();
                if !self.check(&TokenKind::RightParen) {
                    args.push(self.parse_expr()?);
                    while self.check(&TokenKind::Comma) {
                        self.advance()?;
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
                self.advance()?;
                Ok(Expr::Literal(r2n_ast::lit::Literal::Null))
            }
            // arrow function: "(" params? ")" "=>" expr
            TokenKind::LeftParen => {
                if self.looks_like_arrow() {
                    self.advance()?; // consume the '('
                    let mut params = Vec::new();
                    if !self.check(&TokenKind::RightParen) {
                        params.push(self.expect_ident()?);
                        while self.check(&TokenKind::Comma) {
                            self.advance()?;
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
                    self.advance()?; // consume the '('
                    let e = self.parse_expr()?;
                    self.expect(TokenKind::RightParen)?;
                    Ok(e)
                }
            }
            TokenKind::Ident(name) if name == "async" && self.looks_like_async_arrow() => {
                // `async (params) => body` / `async x => body` (M2-T07).
                self.advance()?; // `async`
                let mut params = Vec::new();
                if self.check(&TokenKind::LeftParen) {
                    self.advance()?;
                    if !self.check(&TokenKind::RightParen) {
                        params.push(self.expect_ident()?);
                        while self.check(&TokenKind::Comma) {
                            self.advance()?;
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
                // `await expr` — the lowerer restricts it to async statement
                // positions (a precise compile error elsewhere).
                self.advance()?;
                let value = self.parse_expr()?;
                Ok(Expr::Await {
                    value: Box::new(value),
                    from_return: false,
                })
            }
            TokenKind::Ident(name) if name == "throw" => {
                // `throw value` — an expression form; the value raises to the
                // nearest enclosing try (eval turns it into a thrown Value).
                self.advance()?;
                let value = self.parse_expr()?;
                Ok(Expr::Throw(Box::new(value)))
            }
            TokenKind::Ident(name) if name == "try" => self.parse_try(),
            TokenKind::Ident(name) if name == "if" => {
                // `if cond { then } else { else }` -> ternary.
                self.advance()?;
                let cond = self.parse_expr()?;
                self.expect(TokenKind::LeftBrace)?;
                let then = self.parse_expr()?;
                self.expect(TokenKind::RightBrace)?;
                if !matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "else") {
                    return Err(self.err("expected `else` after `if` block"));
                }
                self.advance()?;
                self.expect(TokenKind::LeftBrace)?;
                let else_ = self.parse_expr()?;
                self.expect(TokenKind::RightBrace)?;
                Ok(Expr::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    else_: Box::new(else_),
                })
            }
            TokenKind::Ident(name) => {
                let n = name.clone();
                self.advance()?;
                let is_component = Self::is_component_name(&n);
                Ok(Expr::Ident {
                    name: n,
                    is_component,
                })
            }
            TokenKind::LeftBracket => self.parse_array(),
            TokenKind::Lt => self.parse_element(),
            _ => Err(self.err(&format!(
                "unexpected {} in expression",
                self.current.kind.describe()
            ))),
        }
    }

    /// Cheap multi-token lookahead to decide whether `(` begins an arrow
    /// function `(params) => expr` rather than a parenthesized expression.
    /// When this is called, `self.current` is `LeftParen` and `self.lexer`
    /// (which is `Copy`) is positioned to emit the token *after* `(`.
    fn looks_like_arrow(&self) -> bool {
        let mut l = self.lexer; // copy: does not disturb parser state
        loop {
            let tok = match l.next_token() {
                Ok(t) => t,
                Err(_) => return false,
            };
            match &tok.kind {
                TokenKind::RightParen => {
                    // After `)`, the next token must be `=>`.
                    let nxt = match l.next_token() {
                        Ok(t) => t,
                        Err(_) => return false,
                    };
                    return matches!(nxt.kind, TokenKind::Arrow);
                }
                TokenKind::Ident(_) | TokenKind::Comma => continue,
                _ => return false,
            }
        }
    }

    /// Cheap lookahead: does `async` here begin an async arrow —
    /// `async (params) => ` or `async ident => `? All scan errors -> false
    /// (then `async` is an ordinary identifier).
    fn looks_like_async_arrow(&self) -> bool {
        let mut l = self.lexer; // copy: does not disturb parser state
        match l.next_token() {
            Ok(t) => match t.kind {
                TokenKind::LeftParen => {
                    let mut depth = 1;
                    loop {
                        match l.next_token() {
                            Ok(t) => match t.kind {
                                TokenKind::LeftParen => depth += 1,
                                TokenKind::RightParen => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            },
                            Err(_) => return false,
                        }
                    }
                    matches!(l.next_token().map(|t| t.kind), Ok(TokenKind::Arrow))
                }
                TokenKind::Ident(_) => {
                    matches!(l.next_token().map(|t| t.kind), Ok(TokenKind::Arrow))
                }
                _ => false,
            },
            Err(_) => false,
        }
    }

    /// Parse the body of an arrow function: either a single expression
    /// (`x => x + 1`) or a block of expression statements (`() => { a(); b(); }`).
    fn parse_arrow_body(&mut self) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::LeftBrace) {
            self.advance()?;
            let mut stmts = Vec::new();
            while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
                // `return expr;` as the (terminal) statement of a block-bodied
                // arrow: the returned expr becomes the block's VALUE — the
                // block terminates here (React's effect cleanup form:
                // `() => { setup(); return () => cleanup(); }`).
                if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "return") {
                    self.advance()?;
                    let v = self.parse_expr()?;
                    // `return await p` — resolved value completes the async
                    // fn (mirrors parse_block_stmts_inner).
                    let v = match v {
                        Expr::Await { value, .. } => Expr::Await {
                            value,
                            from_return: true,
                        },
                        other => other,
                    };
                    stmts.push(v);
                    if self.check(&TokenKind::Semicolon) {
                        self.advance()?;
                    }
                    break;
                }
                // `let`/`const` inside a block-bodied arrow: a scoped local.
                if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "let" || kw == "const")
                {
                    let _is_let = matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "let");
                    self.advance()?;
                    let name = self.expect_ident()?;
                    self.expect(TokenKind::Equals)?;
                    let value = self.parse_expr()?;
                    if self.check(&TokenKind::Semicolon) {
                        self.advance()?;
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
                    self.advance()?;
                }
            }
            self.expect(TokenKind::RightBrace)?;
            Ok(Expr::Block(stmts))
        } else {
            self.parse_expr()
        }
    }

    /// Parse `{ stmts }` as a list of statement-level expressions (the shared
    /// body shape of try/catch/finally blocks); consumes both braces.
    fn parse_block_stmts(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.expect(TokenKind::LeftBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "return") {
                self.advance()?;
                let v = self.parse_expr()?;
                // `return await p` — the resolved value completes the async
                // fn (marked; a bare terminal `await p;` only suspends).
                let v = match v {
                    Expr::Await { value, .. } => Expr::Await {
                        value,
                        from_return: true,
                    },
                    other => other,
                };
                stmts.push(v);
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                }
                break;
            }
            if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "let" || kw == "const") {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
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
                self.advance()?;
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(stmts)
    }

    /// `try { } catch (e) { } finally { }` — catch and finally are both
    /// optional but at least one must be present (ECMA grammar).
    fn parse_try(&mut self) -> Result<Expr, ParseError> {
        self.advance()?; // `try`
        let block = self.parse_block_stmts()?;
        let (mut catch_param, mut catch, mut finally) = (None, None, None);
        if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "catch") {
            self.advance()?;
            if self.check(&TokenKind::LeftParen) {
                self.advance()?;
                catch_param = Some(self.expect_ident()?);
                self.expect(TokenKind::RightParen)?;
            }
            catch = Some(self.parse_block_stmts()?);
        }
        if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "finally") {
            self.advance()?;
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
                self.advance()?;
                // allow trailing comma
                if self.check(&TokenKind::RightBracket) {
                    break;
                }
                items.push(self.parse_expr()?);
            }
        }
        self.expect(TokenKind::RightBracket)?;
        Ok(Expr::Array(items))
    }

    fn parse_element(&mut self) -> Result<Expr, ParseError> {
        self.expect(TokenKind::Lt)?;
        // Fragment shorthand `<>...</>`: no tag. Modeled as an Element with an
        // empty tag (the lowering turns it into a React Fragment node).
        let is_fragment = self.check(&TokenKind::Gt);
        let mut tag = if is_fragment {
            String::new()
        } else {
            self.expect_ident()?
        };
        // Dotted member tags: `<Ctx.Provider>`, `<Ctx.Consumer>` — the
        // context API's JSX form.
        if !is_fragment && self.check(&TokenKind::Dot) {
            self.advance()?;
            let member = self.expect_ident()?;
            tag = format!("{tag}.{member}");
        }
        let is_component = !is_fragment && Self::is_component_name(&tag);
        let mut props = Vec::new();
        // attributes until "/>" or ">"
        loop {
            match &self.current.kind {
                TokenKind::Slash => {
                    self.advance()?;
                    self.expect(TokenKind::Gt)?;
                    return Ok(Expr::Element(Element {
                        tag,
                        is_component,
                        props,
                        children: Vec::new(),
                    }));
                }
                TokenKind::Gt => {
                    self.advance()?;
                    break;
                }
                TokenKind::Ident(name) => {
                    let pname = name.clone();
                    self.advance()?;
                    if self.check(&TokenKind::Equals) {
                        self.advance()?;
                        // value is `{ expr }` or a string literal
                        if self.check(&TokenKind::LeftBrace) {
                            self.advance()?;
                            let value = self.parse_expr()?;
                            self.expect(TokenKind::RightBrace)?;
                            props.push(Prop {
                                name: pname,
                                value: Some(value),
                            });
                        } else if let TokenKind::String(s) = &self.current.kind {
                            let v = s.clone();
                            self.advance()?;
                            props.push(Prop {
                                name: pname,
                                value: Some(Expr::Literal(r2n_ast::lit::Literal::String(v))),
                            });
                        } else {
                            return Err(self.err("expected `{...}` or string after `=`"));
                        }
                    } else {
                        // boolean shorthand
                        props.push(Prop {
                            name: pname,
                            value: None,
                        });
                    }
                }
                _ => return Err(self.err("unexpected token in element attributes")),
            }
        }
        // children
        let mut children = Vec::new();
        while !self.check(&TokenKind::LtSlash) && !self.check(&TokenKind::Eof) {
            if self.check(&TokenKind::LeftBrace) {
                self.advance()?;
                let e = self.parse_expr()?;
                self.expect(TokenKind::RightBrace)?;
                children.push(e);
            } else if self.check(&TokenKind::Lt) {
                children.push(self.parse_element()?);
            } else {
                // JSX text child: everything from the pending token up to the
                // next `{`/`<` is literal text (e.g. `+1`, `Hello, world`).
                // Rescan from the pending token's own start offset, then
                // re-lex so `current` becomes the boundary token.
                let start = self.current.offset;
                let text = self.lexer.rescan_jsx_text(start);
                let text = text.trim().to_string();
                if text.is_empty() {
                    // Whitespace-only child: JSX drops it; re-lex and continue.
                    self.current = self.lexer.next_token()?;
                    continue;
                }
                children.push(Expr::Literal(r2n_ast::lit::Literal::String(text)));
                // Refresh the pending token: it may now be `{`, `<`, or `</`.
                self.current = self.lexer.next_token()?;
            }
        }
        self.expect(TokenKind::LtSlash)?;
        if is_fragment {
            // `</>` — the fragment close has no tag name.
            self.expect(TokenKind::Gt)?;
        } else {
            let mut close = self.expect_ident()?;
            if self.check(&TokenKind::Dot) {
                self.advance()?;
                let m = self.expect_ident()?;
                close = format!("{close}.{m}");
            }
            if close != tag {
                return Err(self.err(&format!(
                    "mismatched closing tag: expected {tag}, found {close}"
                )));
            }
            self.expect(TokenKind::Gt)?;
        }
        Ok(Expr::Element(Element {
            tag,
            is_component,
            props,
            children,
        }))
    }
}
