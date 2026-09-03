//! Recursive-descent parser: tokens -> `r2n_ast::Program`.
//!
//! Grammar (subset, precedence climbing):
//!
//! ```text
//! program     := decl* EOF
//! decl        := import | component | export-default | export-named
//! import      := "import" import-clause "from" string ";"
//!              | "import" string ";"
//! import-clause := "{" ident ("as" ident)? ("," ident ("as" ident)?)* "}"
//!              | "*" "as" ident
//!              | ident ("," ("{" ident ... "}" | "*" "as" ident))?
//! component   := "component" ident "(" params? ")" "{" stmt* "}"
//! params      := ident ("," ident)*
//! stmt        := ("let" | "const") ident "=" expr ";"
//!              | "return" expr ";"
//! export      := "export" "default" ident ";"
//!              | "export" "{" ident ("as" ident)? ("," ident ("as" ident)?)* "}" ";"
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
use r2n_ast::program::{
    ClassComponent, Component, Decl, DeclKind, ExportDecl, ExportNamed, FuncDecl, Import, Method,
    ObjectProp, Param, Pattern, Program, Stmt,
};

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

    /// Parse an imported module: declarations without the `export default`
    /// requirement (only the app's entry module must declare a root
    /// component; the linker verifies that itself, M2-T09).
    pub fn parse_module(&mut self) -> Result<Program, ParseError> {
        let mut program = Program::new();
        while !self.check(&TokenKind::Eof) {
            program.decls.push(self.parse_decl()?);
        }
        Ok(program)
    }

    /// `(p1, p2, ...)` — full parameter patterns: `x`, `x = dflt`,
    /// `{a, b: c}`, `[x, y]`, `...rest`. A bare `...rest` must be last.
    fn parse_param_patterns(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(TokenKind::LeftParen)?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.parse_param()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                if self.check(&TokenKind::RightParen) {
                    break; // trailing comma
                }
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        // `...rest` must be last.
        for (i, p) in params.iter().enumerate() {
            if p.rest && i + 1 != params.len() {
                return Err(self.err("rest parameter must be last"));
            }
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        if self.check(&TokenKind::DotDotDot) {
            self.advance()?;
            let name = self.expect_ident()?;
            return Ok(Param {
                pattern: Pattern::Name {
                    name,
                    default: None,
                },
                default: None,
                rest: true,
            });
        }
        let pattern = self.parse_pattern()?;
        let default = if self.check(&TokenKind::Equals) {
            self.advance()?;
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Param {
            pattern,
            default,
            rest: false,
        })
    }

    /// A binding pattern: `x`, `x = dflt`, `{a, b: c = d, ...rest}`,
    /// `[a, , b = d, ...rest]`. `in_binding`: when true, a bare `name`
    /// followed by `=` is a PLAIN name (the `=` starts the value — `let x
    /// = v`); the default form `x = d` only applies inside patterns
    /// (params, nested positions, array/object patterns).
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.parse_pattern_inner(false)
    }

    /// `let`/`const` binding position: `let x = v` (plain) vs `let {a} = v`
    /// / `let [x] = v` (destructuring).
    fn parse_binding_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.parse_pattern_inner(true)
    }

    fn parse_pattern_inner(&mut self, in_binding: bool) -> Result<Pattern, ParseError> {
        match &self.current.kind {
            TokenKind::LeftBrace => self.parse_object_pattern(),
            TokenKind::LeftBracket => self.parse_array_pattern(),
            _ => {
                let name = self.expect_ident()?;
                let default = if self.check(&TokenKind::Equals) && !in_binding {
                    self.advance()?;
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                Ok(Pattern::Name { name, default })
            }
        }
    }

    fn parse_object_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.expect(TokenKind::LeftBrace)?;
        let mut props = Vec::new();
        let mut rest = None;
        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::DotDotDot) {
                self.advance()?;
                rest = Some(self.expect_ident()?);
                if self.check(&TokenKind::Comma) {
                    self.advance()?;
                }
                break;
            }
            let key = self.expect_ident()?;
            let alias = if self.check(&TokenKind::Colon) {
                self.advance()?;
                Some(self.parse_pattern()?)
            } else if self.check(&TokenKind::Equals) {
                // `{a = d}` — shorthand with default.
                self.advance()?;
                let d = self.parse_expr()?;
                Some(Pattern::Name {
                    name: key.clone(),
                    default: Some(d),
                })
            } else {
                None
            };
            props.push(ObjectProp { key, alias });
            if self.check(&TokenKind::Comma) {
                self.advance()?;
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Pattern::Object { props, rest })
    }

    fn parse_array_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.expect(TokenKind::LeftBracket)?;
        let mut items: Vec<Option<Pattern>> = Vec::new();
        let mut rest = None;
        while !self.check(&TokenKind::RightBracket) {
            if self.check(&TokenKind::Comma) {
                // A hole: `[a, , b]`.
                self.advance()?;
                items.push(None);
                continue;
            }
            if self.check(&TokenKind::DotDotDot) {
                self.advance()?;
                rest = Some(self.expect_ident()?);
                if self.check(&TokenKind::Comma) {
                    self.advance()?;
                }
                break;
            }
            items.push(Some(self.parse_pattern()?));
            if self.check(&TokenKind::Comma) {
                self.advance()?;
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBracket)?;
        Ok(Pattern::Array { items, rest })
    }

    /// `(pattern, value)` for `let`/`const` — plain `name = expr` or a
    /// destructuring pattern.
    fn parse_binding(&mut self) -> Result<(Pattern, Expr), ParseError> {
        let pattern = self.parse_binding_pattern()?;
        self.expect(TokenKind::Equals)?;
        let value = self.parse_expr()?;
        Ok((pattern, value))
    }

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(kw) if kw == "import" => self.parse_import(),
            TokenKind::Ident(kw) if kw == "component" => {
                Ok(Decl::Component(self.parse_component()?))
            }
            TokenKind::Ident(kw) if kw == "class" => Ok(Decl::Class(self.parse_class()?)),
            TokenKind::Ident(kw) if kw == "export" => self.parse_export(),
            TokenKind::Ident(kw) if kw == "function" => self.parse_function_decl(),
            // Top-level `let`/`const` (module-scope bindings, T09b): the
            // declaration lives at module scope; the linker binds it into the
            // global env in source order.
            TokenKind::Ident(kw) if kw == "let" || kw == "const" => {
                let kind = if kw == "let" {
                    DeclKind::Let
                } else {
                    DeclKind::Const
                };
                self.advance()?;
                let (pattern, value) = self.parse_binding()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::TopLevel {
                    kind,
                    pattern,
                    value,
                })
            }
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
                        params.push(plain_param(self.expect_ident()?));
                        while self.check(&TokenKind::Comma) {
                            self.advance()?;
                            params.push(plain_param(self.expect_ident()?));
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
        self.advance()?; // `import`
        let mut import = Import {
            default_: None,
            named: Vec::new(),
            namespace: None,
            path: String::new(),
        };
        match &self.current.kind {
            // `import "path";` — side-effect only.
            TokenKind::String(_) => {
                import.path = self.expect_string()?;
            }
            // `import { a, b as c } from "path";`
            TokenKind::LeftBrace => {
                self.advance()?;
                self.parse_named_imports(&mut import.named)?;
                self.expect_from()?;
                import.path = self.expect_string()?;
            }
            // `import * as ns from "path";`
            TokenKind::Star => {
                self.advance()?;
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
                    self.advance()?;
                    match &self.current.kind {
                        TokenKind::LeftBrace => {
                            self.advance()?;
                            self.parse_named_imports(&mut import.named)?;
                        }
                        TokenKind::Star => {
                            self.advance()?;
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
            let local = if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "as") {
                self.advance()?;
                self.expect_ident()?
            } else {
                imported.clone()
            };
            out.push((imported, local));
            if self.check(&TokenKind::Comma) {
                self.advance()?;
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBrace)
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        if matches!(&self.current.kind, TokenKind::Ident(k) if k == kw) {
            self.advance()?;
            Ok(())
        } else {
            Err(self.err(&format!("expected `{kw}`")))
        }
    }

    fn expect_from(&mut self) -> Result<(), ParseError> {
        self.expect_keyword("from")
    }

    fn expect_string(&mut self) -> Result<String, ParseError> {
        if let TokenKind::String(s) = &self.current.kind {
            let s = s.clone();
            self.advance()?;
            Ok(s)
        } else {
            Err(self.err("expected a module specifier string"))
        }
    }

    fn parse_export(&mut self) -> Result<Decl, ParseError> {
        self.advance()?; // `export`
        match &self.current.kind {
            TokenKind::Ident(kw) if kw == "default" => {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportDefault(name))
            }
            // `export { a, b as c };` — named exports of module-level
            // declarations (components, classes, generator fns), M2-T09.
            TokenKind::LeftBrace => {
                self.advance()?;
                let mut names = Vec::new();
                while !self.check(&TokenKind::RightBrace) {
                    let local = self.expect_ident()?;
                    let exported = if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "as")
                    {
                        self.advance()?;
                        self.expect_ident()?
                    } else {
                        local.clone()
                    };
                    names.push((local, exported));
                    if self.check(&TokenKind::Comma) {
                        self.advance()?;
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RightBrace)?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportNamed(ExportNamed { names }))
            }
            // `export function name(params) { ... }` — inline-exported
            // function: declares the binding AND registers a named export.
            TokenKind::Ident(kw) if kw == "function" => {
                match self.parse_function_decl()? {
                    Decl::FuncDecl(f) => Ok(Decl::ExportDecl(ExportDecl::Function(f))),
                    Decl::GeneratorFn(g) => {
                        // `export function* g() {}` — generator with export.
                        Ok(Decl::GeneratorFn(g))
                    }
                    _ => unreachable!("parse_function_decl returns FuncDecl/GeneratorFn"),
                }
            }
            // `export const name = expr;` — inline-exported const binding.
            TokenKind::Ident(kw) if kw == "const" => {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportDecl(ExportDecl::Const {
                    name: name.clone(),
                    value,
                }))
            }
            // `export let name = expr;` — accepted like const (immutability is
            // not enforced beyond the declaration).
            TokenKind::Ident(kw) if kw == "let" => {
                self.advance()?;
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportDecl(ExportDecl::Const {
                    name: name.clone(),
                    value,
                }))
            }
            _ => Err(self.err("expected `default` or `{` after `export`")),
        }
    }

    /// `function name(params) { stmts }` or `function* name(params) { stmts }`:
    /// a plain function (M2-T10) or a top-level generator (M2-T08).
    fn parse_function_decl(&mut self) -> Result<Decl, ParseError> {
        self.advance()?; // `function`
        if self.check(&TokenKind::Star) {
            return self.parse_generator_fn_tail();
        }
        let name = self.expect_ident()?;
        let params = self.parse_param_patterns()?;
        let body = self.parse_stmt_block()?;
        Ok(Decl::FuncDecl(FuncDecl { name, params, body }))
    }
    /// `function* name(params) { stmts }` — a top-level generator (M2-T08).
    /// Split out so `parse_function_decl` can dispatch on the `*`.
    fn parse_generator_fn_tail(&mut self) -> Result<Decl, ParseError> {
        self.advance()?; // `*`
        let name = self.expect_ident()?;
        let params = self.parse_param_patterns()?;
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

    fn parse_component(&mut self) -> Result<Component, ParseError> {
        self.expect(TokenKind::Ident("component".to_string()))?;
        let name = self.expect_ident()?;
        let params = self.parse_param_patterns()?;
        self.expect(TokenKind::LeftBrace)?;
        let mut body = Vec::new();
        while !self.check(&TokenKind::RightBrace) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Component { name, params, body })
    }

    /// `{ stmts }` — a brace block of statements (statement grammar); consumes
    /// both braces.
    fn parse_stmt_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(TokenKind::LeftBrace)?;
        let mut body = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(body)
    }

    /// A loop/branch body: either a `{ stmts }` block or a single statement
    /// (`if (c) return x;`, `while (c) i = i + 1;`).
    fn parse_stmt_or_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.check(&TokenKind::LeftBrace) {
            self.parse_stmt_block()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    /// `for (init; cond; update) body` — C-style loop. `init` is an optional
    /// `let`/`const` (no trailing `;` — the first `;` separates) or an
    /// optional expression; `cond` defaults to `true`; `update` is optional.
    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        self.advance()?; // `for`
        self.expect(TokenKind::LeftParen)?;
        let init = if self.check(&TokenKind::Semicolon) {
            self.advance()?;
            None
        } else if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "let" || kw == "const")
        {
            let kind = match &self.current.kind {
                TokenKind::Ident(kw) if kw == "let" => DeclKind::Let,
                _ => DeclKind::Const,
            };
            self.advance()?;
            let (pattern, value) = self.parse_binding()?;
            self.expect(TokenKind::Semicolon)?;
            let stmt = match pattern {
                Pattern::Name { name, .. } => match kind {
                    DeclKind::Let => Stmt::Let { name, value },
                    DeclKind::Const => Stmt::Const { name, value },
                },
                pattern => Stmt::Destructure {
                    kind,
                    pattern,
                    value,
                },
            };
            Some(Box::new(stmt))
        } else {
            let e = self.parse_expr()?;
            self.expect(TokenKind::Semicolon)?;
            Some(Box::new(Stmt::Expr(e)))
        };
        let cond = if self.check(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon)?;
        let update = if self.check(&TokenKind::RightParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::RightParen)?;
        let body = self.parse_stmt_or_block()?;
        Ok(Stmt::For {
            init,
            cond,
            update,
            body,
        })
    }

    /// `switch (disc) { case e: stmts...; default: stmts... }` — cases run in
    /// source order with fall-through; `default` may appear anywhere.
    fn parse_switch(&mut self) -> Result<Stmt, ParseError> {
        self.advance()?; // `switch`
        self.expect(TokenKind::LeftParen)?;
        let disc = self.parse_expr()?;
        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::LeftBrace)?;
        let mut cases = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "case") {
                self.advance()?;
                let test = self.parse_expr()?;
                self.expect(TokenKind::Colon)?;
                let mut body = Vec::new();
                while !self.check(&TokenKind::RightBrace)
                    && !matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "case" || kw == "default")
                    && !self.check(&TokenKind::Eof)
                {
                    body.push(self.parse_stmt()?);
                }
                cases.push((Some(test), body));
            } else if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "default") {
                self.advance()?;
                self.expect(TokenKind::Colon)?;
                let mut body = Vec::new();
                while !self.check(&TokenKind::RightBrace)
                    && !matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "case" || kw == "default")
                    && !self.check(&TokenKind::Eof)
                {
                    body.push(self.parse_stmt()?);
                }
                cases.push((None, body));
            } else {
                return Err(self.err("expected `case` or `default` in switch body"));
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Stmt::Switch { disc, cases })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match &self.current.kind {
            TokenKind::Ident(kw) if kw == "let" || kw == "const" => {
                let kind = if kw == "let" {
                    DeclKind::Let
                } else {
                    DeclKind::Const
                };
                self.advance()?;
                let (pattern, value) = self.parse_binding()?;
                self.expect(TokenKind::Semicolon)?;
                // Plain `let x = v` keeps the fast path; anything else is a
                // destructuring declaration.
                match pattern {
                    Pattern::Name {
                        name,
                        default: None,
                    } => match kind {
                        DeclKind::Let => Ok(Stmt::Let { name, value }),
                        DeclKind::Const => Ok(Stmt::Const { name, value }),
                    },
                    Pattern::Name {
                        name,
                        default: Some(d),
                    } => {
                        // `let x = dflt = ...` never parses (default binds at
                        // the pattern level, consumed above) — but a bare
                        // `let x = <expr>` with an `=` INSIDE the expr is
                        // fine; this arm is unreachable. Keep it total.
                        let _ = d;
                        match kind {
                            DeclKind::Let => Ok(Stmt::Let { name, value }),
                            DeclKind::Const => Ok(Stmt::Const { name, value }),
                        }
                    }
                    pattern => Ok(Stmt::Destructure {
                        kind,
                        pattern,
                        value,
                    }),
                }
            }
            TokenKind::Ident(kw) if kw == "if" => {
                self.advance()?;
                self.expect(TokenKind::LeftParen)?;
                let cond = self.parse_expr()?;
                self.expect(TokenKind::RightParen)?;
                let then = self.parse_stmt_or_block()?;
                let else_ = if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "else") {
                    self.advance()?;
                    Some(self.parse_stmt_or_block()?)
                } else {
                    None
                };
                Ok(Stmt::If { cond, then, else_ })
            }
            TokenKind::Ident(kw) if kw == "while" => {
                self.advance()?;
                self.expect(TokenKind::LeftParen)?;
                let cond = self.parse_expr()?;
                self.expect(TokenKind::RightParen)?;
                let body = self.parse_stmt_or_block()?;
                Ok(Stmt::While { cond, body })
            }
            TokenKind::Ident(kw) if kw == "for" => self.parse_for(),
            TokenKind::Ident(kw) if kw == "switch" => self.parse_switch(),
            TokenKind::Ident(kw) if kw == "break" => {
                self.advance()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Break)
            }
            TokenKind::Ident(kw) if kw == "continue" => {
                self.advance()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Continue)
            }
            TokenKind::Ident(kw) if kw == "return" => {
                self.advance()?;
                // Bare `return;` (no value) returns `undefined` — the form
                // real code uses for early exits (`if (!x) return;`).
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                    return Ok(Stmt::Return(Expr::Literal(
                        r2n_ast::lit::Literal::Undefined,
                    )));
                }
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

    /// `target = value` / `target += value` — right-associative, lowest
    /// precedence. Target must be an identifier or a member access.
    fn parse_assign(&mut self) -> Result<Expr, ParseError> {
        let target = self.parse_nullish()?;
        // Compound assignment: `x += v` desugars to `x = x + v` (same for
        // `-=`, `*=`, `/=`, `%=`) — evaluated once per ECMA left-to-right
        // order (target evaluated first, then value).
        let compound = match &self.current.kind {
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Mod),
            _ => None,
        };
        if let Some(op) = compound {
            let is_assignable = matches!(&target, Expr::Ident { .. } | Expr::Member { .. });
            if !is_assignable {
                return Err(self.err("assignment target must be an identifier or a member access"));
            }
            self.advance()?;
            let value = self.parse_assign()?;
            return Ok(Expr::Assign {
                target: Box::new(target.clone()),
                value: Box::new(Expr::Binary {
                    op,
                    left: Box::new(target),
                    right: Box::new(value),
                }),
            });
        }
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

    /// `a ?? b` — nullish coalescing: `a` unless it is null/undefined.
    /// Binds looser than `||` (ECMA forbids mixing `??` with `&&`/`||`
    /// without parens; we allow the parse but the runtime evaluates
    /// left-to-right — documented).
    fn parse_nullish(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_ternary()?;
        while self.check(&TokenKind::QuestionQuestion) {
            self.advance()?;
            let right = self.parse_ternary()?;
            left = Expr::Binary {
                op: BinOp::Nullish,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
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
        let mut left = self.parse_bitor()?;
        while self.check(&TokenKind::PipePipe) {
            self.advance()?;
            let right = self.parse_bitor()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// `a | b` — bitwise OR (ECMA 13.11, binds tighter than `||`).
    fn parse_bitor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.check(&TokenKind::Pipe) {
            self.advance()?;
            let right = self.parse_and()?;
            left = Expr::Binary {
                op: BinOp::BitOr,
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
                // Postfix `x++` / `x--` (statement-position `i--` in `while`,
                // `i++` in `for` updates). Target must be assignable.
                TokenKind::PlusPlus | TokenKind::MinusMinus => {
                    let op = match &self.current.kind {
                        TokenKind::PlusPlus => r2n_ast::expr::UpdateOp::Inc,
                        _ => r2n_ast::expr::UpdateOp::Dec,
                    };
                    let is_assignable = matches!(&expr, Expr::Ident { .. } | Expr::Member { .. });
                    if !is_assignable {
                        return Err(
                            self.err("update target must be an identifier or a member access")
                        );
                    }
                    self.advance()?;
                    expr = Expr::Update {
                        op,
                        target: Box::new(expr),
                        prefix: false,
                    };
                }
                TokenKind::LeftParen => {
                    // An arrow `(params) => body` can appear in call position
                    // (e.g. `arr.map((x) => <li/>)`). Detect it before treating
                    // this as a function call.
                    if self.looks_like_arrow() {
                        self.advance()?; // consume '('
                        let params = self.parse_arrow_params()?;
                        self.expect(TokenKind::Arrow)?;
                        let body = self.parse_arrow_body()?;
                        // This arrow is the sole argument of the call we are
                        // currently parsing (e.g. `arr.map((x) => ...)`). Wrap it.
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args: vec![r2n_ast::expr::CallArg::Expr(Expr::Arrow {
                                params,
                                body: Box::new(body),
                                async_: false,
                            })],
                        };
                        // Consume the call's own closing `)` (the `)` of `.map(`
                        // distinct from the arrow's `(x)`).
                        self.expect(TokenKind::RightParen)?;
                    } else {
                        let args = self.parse_call_args()?;
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
                        args: vec![r2n_ast::expr::CallArg::Expr(idx)],
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// `(args...)` — parenthesized call arguments with `...spread` and
    /// trailing-comma support. Consumes both parens.
    fn parse_call_args(&mut self) -> Result<Vec<r2n_ast::expr::CallArg>, ParseError> {
        self.expect(TokenKind::LeftParen)?;
        let mut args = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            args.push(self.parse_call_arg()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                if self.check(&TokenKind::RightParen) {
                    break; // trailing comma
                }
                args.push(self.parse_call_arg()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        Ok(args)
    }

    fn parse_call_arg(&mut self) -> Result<r2n_ast::expr::CallArg, ParseError> {
        if self.check(&TokenKind::DotDotDot) {
            self.advance()?;
            Ok(r2n_ast::expr::CallArg::Spread(self.parse_expr()?))
        } else {
            Ok(r2n_ast::expr::CallArg::Expr(self.parse_expr()?))
        }
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
                let args = self.parse_call_args()?;
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
            TokenKind::TemplateStart => self.parse_template(),
            // Object literal `{a, b: expr, ...spread}`. NOTE: `{` in statement
            // position is a BLOCK, not an object — blocks are handled by the
            // statement parser; in expression position `{` always opens an
            // object literal (arrow bodies use `parse_arrow_body`, not this).
            TokenKind::LeftBrace => self.parse_object(),
            // arrow function: "(" params? ")" "=>" expr — full patterns.
            TokenKind::LeftParen => {
                if self.looks_like_arrow() {
                    self.advance()?; // consume the '('
                    let params = self.parse_arrow_params()?;
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
            // Single-ident arrow: `x => expr` (no parens). Only when `=>`
            // IMMEDIATELY follows the ident — keywords like `case`/`default`
            // that happen to precede an unrelated `=>` elsewhere must not
            // match. `arrow_after_ident` peeks exactly one token.
            TokenKind::Ident(name)
                if !matches!(
                    name.as_str(),
                    "async"
                        | "await"
                        | "yield"
                        | "throw"
                        | "try"
                        | "if"
                        | "else"
                        | "new"
                        | "import"
                        | "typeof"
                        | "true"
                        | "false"
                        | "null"
                        | "undefined"
                        | "this"
                        | "case"
                        | "default"
                        | "return"
                        | "let"
                        | "const"
                        | "while"
                        | "for"
                        | "switch"
                        | "break"
                        | "continue"
                        | "function"
                        | "class"
                        | "export"
                        | "from"
                        | "as"
                ) && self.arrow_after_ident() =>
            {
                let n = name.clone();
                self.advance()?;
                self.expect(TokenKind::Arrow)?;
                let body = self.parse_arrow_body()?;
                Ok(Expr::Arrow {
                    params: vec![Param {
                        pattern: Pattern::Name {
                            name: n,
                            default: None,
                        },
                        default: None,
                        rest: false,
                    }],
                    body: Box::new(body),
                    async_: false,
                })
            }
            TokenKind::Ident(name) if name == "async" && self.looks_like_async_arrow() => {
                // `async (params) => body` / `async x => body` (M2-T07).
                self.advance()?; // `async`
                let params = if self.check(&TokenKind::LeftParen) {
                    self.advance()?;
                    let ps = self.parse_arrow_params_inner()?;
                    self.expect(TokenKind::RightParen)?;
                    ps
                } else {
                    vec![Param {
                        pattern: Pattern::Name {
                            name: self.expect_ident()?,
                            default: None,
                        },
                        default: None,
                        rest: false,
                    }]
                };
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
            TokenKind::Ident(name) if name == "yield" => {
                // `yield` / `yield expr` — generator suspension (M2-T08); the
                // lowerer restricts it to generator statement positions.
                self.advance()?;
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
                // `throw value` — an expression form; the value raises to the
                // nearest enclosing try (eval turns it into a thrown Value).
                self.advance()?;
                let value = self.parse_expr()?;
                Ok(Expr::Throw(Box::new(value)))
            }
            TokenKind::Ident(name) if name == "try" => self.parse_try(),
            TokenKind::Ident(name) if name == "function" => {
                // `function Name?(params) { stmts }` in expression position
                // (e.g. `memo(function Item(...) { return ...; })`): a
                // first-class function value with a full statement body.
                self.advance()?; // `function`
                let name = if matches!(&self.current.kind, TokenKind::Ident(_)) {
                    // An optional name — but `(` directly means anonymous.
                    // Peek: a name is followed by `(`; anything else errors
                    // in expect_ident anyway.
                    let n = self.expect_ident()?;
                    Some(n)
                } else {
                    None
                };
                let params = self.parse_param_patterns()?;
                let body = self.parse_stmt_block()?;
                Ok(Expr::Function { name, params, body })
            }
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
            TokenKind::Ident(name) if name == "import" => {
                // `import("path")` — dynamic import (M2-T09). Lookahead: the
                // lexer (Copy) is positioned to emit the token after `import`;
                // dynamic import is recognized only when that token is `(`.
                // A bare `import` in expression position stays an identifier.
                let mut l = self.lexer.clone();
                let is_call = matches!(l.next_token(), Ok(tok)
                    if matches!(tok.kind, TokenKind::LeftParen));
                if is_call {
                    self.advance()?; // `import`
                    self.expect(TokenKind::LeftParen)?;
                    let specifier = match &self.current.kind {
                        TokenKind::String(s) => s.clone(),
                        _ => {
                            return Err(
                                self.err("dynamic import specifier must be a string literal")
                            )
                        }
                    };
                    self.advance()?;
                    self.expect(TokenKind::RightParen)?;
                    Ok(Expr::DynImport { specifier })
                } else {
                    self.advance()?;
                    Ok(Expr::Ident {
                        name: "import".to_string(),
                        is_component: false,
                    })
                }
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

    /// Arrow params after `(` was consumed: full patterns. (The `(` itself is
    /// consumed by the caller — `parse_param_patterns` expects it; this reads
    /// the inner list plus `)`.)
    fn parse_arrow_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.parse_param()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                if self.check(&TokenKind::RightParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        Ok(params)
    }

    /// Same, for the async-arrow path where `(` was already consumed.
    fn parse_arrow_params_inner(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.parse_param()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                if self.check(&TokenKind::RightParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }
        Ok(params)
    }

    /// Lookahead: is the current ident immediately followed by `=>`?
    /// (Single-ident arrow `x => ...`.)
    fn arrow_after_ident(&self) -> bool {
        let mut l = self.lexer.clone();
        matches!(l.next_token().map(|t| t.kind), Ok(TokenKind::Arrow))
    }

    /// Cheap multi-token lookahead to decide whether `(` begins an arrow
    /// function `(params) => expr` rather than a parenthesized expression.
    /// When this is called, `self.current` is `LeftParen` and `self.lexer`
    /// is positioned to emit the token *after* `(`. Accepts full patterns:
    /// idents, `{`/`}`/`[`/`]`/`,`/`...`/`=` plus nested literals; the
    /// closing `)` must be followed by `=>`. Depth-tracked so defaults like
    /// `(x = f(1)) => ...` scan correctly.
    fn looks_like_arrow(&self) -> bool {
        let mut l = self.lexer.clone(); // clone: does not disturb parser state
        let mut depth = 0usize;
        loop {
            let tok = match l.next_token() {
                Ok(t) => t,
                Err(_) => return false,
            };
            match &tok.kind {
                TokenKind::LeftParen | TokenKind::LeftBrace | TokenKind::LeftBracket => {
                    depth += 1;
                }
                TokenKind::RightParen if depth == 0 => {
                    // After `)`, the next token must be `=>`.
                    let nxt = match l.next_token() {
                        Ok(t) => t,
                        Err(_) => return false,
                    };
                    return matches!(nxt.kind, TokenKind::Arrow);
                }
                TokenKind::RightParen | TokenKind::RightBrace | TokenKind::RightBracket => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                TokenKind::Ident(_)
                | TokenKind::Comma
                | TokenKind::DotDotDot
                | TokenKind::Equals
                | TokenKind::Colon
                | TokenKind::Int(_)
                | TokenKind::Float(_)
                | TokenKind::String(_) => continue,
                _ => return false,
            }
        }
    }

    /// Cheap lookahead: does `async` here begin an async arrow —
    /// `async (params) => ` or `async ident => `? All scan errors -> false
    /// (then `async` is an ordinary identifier).
    fn looks_like_async_arrow(&self) -> bool {
        let mut l = self.lexer.clone(); // clone: does not disturb parser state
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
                        Expr::Yield { value, .. } => Expr::Yield {
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
                // Full statement grammar in block bodies (`switch`, `while`,
                // `for`, `if`, `break`...): each Stmt lowers to its block
                // expression form via stmt_to_block_expr.
                if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "if" || kw == "while" || kw == "for" || kw == "switch" || kw == "break" || kw == "continue")
                {
                    let st = self.parse_stmt()?;
                    stmts.push(self.stmt_to_block_expr(st)?);
                    continue;
                }
                // `let`/`const` (incl. destructuring) inside a block-bodied
                // arrow: scoped locals, same as full statements.
                if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "let" || kw == "const")
                {
                    self.advance()?;
                    let (pattern, value) = self.parse_binding()?;
                    if self.check(&TokenKind::Semicolon) {
                        self.advance()?;
                    }
                    stmts.push(lower_binding_to_assign(pattern, value));
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

    /// Lower a general statement to its block-expression form for arrow/block
    /// bodies: `if` -> ternary-style If, `while`/`for` -> While, `switch` ->
    /// nested If-chain on strict equality, `break`/`continue` -> control
    /// errors expressed as values (the runtime's While driver recognizes
    /// them; a stray break/continue outside a loop is a runtime error).
    /// NOTE: full break/continue-through-nesting needs runtime control-flow
    /// values; this_sig provides statement-position support inside blocks.
    fn stmt_to_block_expr(&mut self, st: Stmt) -> Result<Expr, ParseError> {
        match st {
            Stmt::Expr(e) => Ok(e),
            Stmt::Return(e) => Ok(e),
            Stmt::Let { name, value } | Stmt::Const { name, value } => Ok(lower_binding_to_assign(
                Pattern::Name {
                    name,
                    default: None,
                },
                value,
            )),
            Stmt::Destructure { pattern, value, .. } => Ok(lower_binding_to_assign(pattern, value)),
            Stmt::If { cond, then, else_ } => {
                let then_e = self.stmts_to_block_expr(then)?;
                let else_e = match else_ {
                    Some(ss) => self.stmts_to_block_expr(ss)?,
                    None => Expr::Literal(r2n_ast::lit::Literal::Null),
                };
                Ok(Expr::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then_e),
                    else_: Box::new(else_e),
                })
            }
            Stmt::While { cond, body } => {
                let body_e = self.stmts_to_block_expr(body)?;
                Ok(Expr::While {
                    cond: Box::new(cond),
                    body: Box::new(body_e),
                })
            }
            Stmt::For {
                init,
                cond,
                update,
                body,
            } => {
                // `for (init; cond; update) body` -> `init; while (cond) { body; update }`
                let mut seq = Vec::new();
                if let Some(i) = init {
                    seq.push(self.stmt_to_block_expr(*i)?);
                }
                let body_e = self.stmts_to_block_expr(body)?;
                let body_e = match update {
                    Some(u) => Expr::Block(vec![body_e, u]),
                    None => body_e,
                };
                seq.push(Expr::While {
                    cond: Box::new(
                        cond.unwrap_or(Expr::Literal(r2n_ast::lit::Literal::Bool(true))),
                    ),
                    body: Box::new(body_e),
                });
                Ok(Expr::Block(seq))
            }
            Stmt::Switch { disc, cases } => {
                // `switch (d) { case a: s...; default: s... }` -> nested
                // ternary chain on `d === a` with fall-through: each case's
                // body runs when its test matches OR an earlier case matched
                // and didn't break. Full fall-through needs runtime support;
                // this lowers the common case (each case ends in
                // return/break) precisely: match test -> body, else next.
                let mut acc = Expr::Literal(r2n_ast::lit::Literal::Null);
                for (test, body) in cases.into_iter().rev() {
                    let body_e = self.stmts_to_block_expr(body)?;
                    acc = match test {
                        Some(t) => Expr::Ternary {
                            cond: Box::new(Expr::Binary {
                                op: BinOp::StrictEq,
                                left: Box::new(disc.clone()),
                                right: Box::new(t),
                            }),
                            then: Box::new(body_e),
                            else_: Box::new(acc),
                        },
                        None => body_e,
                    };
                }
                Ok(acc)
            }
            Stmt::Break => Ok(Expr::Break),
            Stmt::Continue => Ok(Expr::Continue),
        }
    }

    fn stmts_to_block_expr(&mut self, stmts: Vec<Stmt>) -> Result<Expr, ParseError> {
        let mut out = Vec::new();
        for st in stmts {
            // A terminal `return` inside a nested block RAISES function-return
            // control flow (caught at every function-like boundary: calls,
            // callbacks, reducers, handlers, memo factories) and stops the
            // sequence. Without this, `if (c) return x;` nested in a loop or
            // a `try` would silently fall through to the following code.
            if let Stmt::Return(e) = st {
                out.push(Expr::Return(Some(Box::new(e))));
                break;
            }
            out.push(self.stmt_to_block_expr(st)?);
        }
        if out.is_empty() {
            out.push(Expr::Literal(r2n_ast::lit::Literal::Null));
        }
        if out.len() == 1 {
            Ok(out.into_iter().next().unwrap())
        } else {
            Ok(Expr::Block(out))
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
                // Bare `return;` completes with `undefined`.
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                    stmts.push(Expr::Return(None));
                    break;
                }
                let v = self.parse_expr()?;
                // `return await p` — the resolved value completes the async
                // fn (marked; a bare terminal `await p;` only suspends).
                let v = match v {
                    Expr::Await { value, .. } => Expr::Await {
                        value,
                        from_return: true,
                    },
                    Expr::Yield { value, .. } => Expr::Yield {
                        value,
                        from_return: true,
                    },
                    // Any other `return e` RAISES function-return control
                    // flow (caught at the call boundary): without this, a
                    // `return` inside `try` would silently fall through to
                    // the code after the `try`.
                    other => Expr::Return(Some(Box::new(other))),
                };
                stmts.push(v);
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                }
                break;
            }
            // Full statement grammar in block bodies (`if`/`while`/`for`/
            // `switch`/`break`/`continue`): each Stmt lowers to its block
            // expression form (same dispatch as arrow bodies).
            if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "if" || kw == "while" || kw == "for" || kw == "switch" || kw == "break" || kw == "continue")
            {
                let st = self.parse_stmt()?;
                stmts.push(self.stmt_to_block_expr(st)?);
                continue;
            }
            if matches!(&self.current.kind, TokenKind::Ident(kw) if kw == "let" || kw == "const") {
                let kind = match &self.current.kind {
                    TokenKind::Ident(kw) if kw == "let" => DeclKind::Let,
                    _ => DeclKind::Const,
                };
                self.advance()?;
                let (pattern, value) = self.parse_binding()?;
                if self.check(&TokenKind::Semicolon) {
                    self.advance()?;
                }
                let _ = kind;
                stmts.push(lower_binding_to_assign(pattern, value));
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
            items.push(self.parse_array_item()?);
            while self.check(&TokenKind::Comma) {
                self.advance()?;
                // allow trailing comma
                if self.check(&TokenKind::RightBracket) {
                    break;
                }
                items.push(self.parse_array_item()?);
            }
        }
        self.expect(TokenKind::RightBracket)?;
        Ok(Expr::Array(items))
    }

    fn parse_array_item(&mut self) -> Result<r2n_ast::expr::ArrayItem, ParseError> {
        if self.check(&TokenKind::DotDotDot) {
            self.advance()?;
            Ok(r2n_ast::expr::ArrayItem::Spread(self.parse_expr()?))
        } else {
            Ok(r2n_ast::expr::ArrayItem::Expr(self.parse_expr()?))
        }
    }

    /// `{key: value, shorthand, ...spread}` — object literal. Keys are plain
    /// identifiers or string literals; `{a}` is shorthand for `{a: a}`.
    fn parse_object(&mut self) -> Result<Expr, ParseError> {
        use r2n_ast::expr::ObjectItem;
        self.expect(TokenKind::LeftBrace)?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::DotDotDot) {
                self.advance()?;
                items.push(ObjectItem::Spread(self.parse_expr()?));
            } else {
                let key = match &self.current.kind {
                    TokenKind::String(s) => {
                        let k = s.clone();
                        self.advance()?;
                        k
                    }
                    _ => self.expect_ident()?,
                };
                if self.check(&TokenKind::Colon) {
                    self.advance()?;
                    items.push(ObjectItem::Prop(key, self.parse_expr()?));
                } else {
                    // `{a}` shorthand — value is the in-scope binding.
                    items.push(ObjectItem::Shorthand(key));
                }
            }
            if self.check(&TokenKind::Comma) {
                self.advance()?;
                if self.check(&TokenKind::RightBrace) {
                    break; // trailing comma
                }
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Expr::Object(items))
    }

    /// `` `head${expr}tail${expr}...` `` — template literal. The lexer emits
    /// TemplateStart; then each chunk is TemplateText (text up to `${` or the
    /// closing backtick) with TemplateEnd terminating. After every
    /// interpolation expression the parser calls `lex_template_chunk`
    /// directly (the `}` that closes `${` was already consumed as a normal
    /// RightBrace by the expression parser).
    fn parse_template(&mut self) -> Result<Expr, ParseError> {
        // `current` IS TemplateStart (the opening backtick was already
        // consumed by `lex_template_start`; the lexer is positioned AFTER
        // it). Do NOT advance — that would lex the chunk text as normal
        // tokens (`$` → error). Call `lex_template_chunk` directly.
        let mut parts = Vec::new();
        let mut exprs = Vec::new();
        loop {
            let chunk = self.lexer.lex_template_chunk()?;
            match chunk.kind {
                TokenKind::TemplateText(text) => {
                    parts.push(text);
                    // Middle chunk: `${` was consumed, an expression follows.
                    // But a chunk ending at the backtick ALSO comes back as
                    // TemplateText (with the End parked) — distinguish via a
                    // probe: clone the lexer and pull one token. NOTE: the
                    // probe's next_token SKIPS trivia, but a parked End is
                    // delivered BEFORE trivia-skipping, so the probe is exact:
                    // parked End -> TemplateEnd; otherwise some real token.
                    let mut probe = self.lexer.clone();
                    match probe.next_token() {
                        Ok(t) if matches!(t.kind, TokenKind::TemplateEnd) => {
                            // Chunk ended at the backtick: consume the parked
                            // End through the real lexer, then advance PAST
                            // it so the caller continues after the template
                            // (the End is synthetic — the backtick is
                            // already consumed; leaving `current` on it
                            // would make the caller misread the next token).
                            let end = self.lexer.next_token()?;
                            debug_assert!(matches!(end.kind, TokenKind::TemplateEnd));
                            self.current = end;
                            self.advance()?;
                            break;
                        }
                        _ => {
                            // Middle chunk: parse the interpolation expr.
                            // The parser's `current` is stale (it predates the
                            // template); re-anchor by pulling the first token
                            // of the expression through the real lexer.
                            // NOTE: next_token skips trivia — leading spaces
                            // inside `${ ... }` are fine.
                            self.current = self.lexer.next_token()?;
                            let e = self.parse_expr()?;
                            // The expression ends at `}`. Consume it WITHOUT
                            // going through expect()/advance: that path lexes
                            // the next token via next_token, which skips
                            // trivia and chokes on a following `${` (middle
                            // chunk — `` `${a} ${b}` ``) with "unexpected `$`".
                            // The loop's next lex_template_chunk reads `rest`
                            // directly (preserving inter-chunk spaces), so no
                            // advance is needed at all here.
                            if !matches!(self.current.kind, TokenKind::RightBrace) {
                                return Err(self.err("expected `}` to close `${...}`"));
                            }
                            self.lexer.exit_template_expr();
                            exprs.push(e);
                        }
                    }
                }
                TokenKind::TemplateEnd => {
                    // Template ends right after an interpolation (`` `a${x}` ``):
                    // the chunk lexer returned End directly with no trailing
                    // text. `current` is stale (it predates the template), so
                    // adopt the End token WITHOUT advancing — then advance
                    // PAST it so the caller continues after the template: the
                    // End token is synthetic (the backtick was already
                    // consumed by the chunk lexer), and leaving `current` ON
                    // it would make the caller re-lex the token after the
                    // template as if the End were real input.
                    // Record the empty tail part to keep parts == exprs + 1.
                    parts.push(String::new());
                    self.current = chunk;
                    self.advance()?;
                    break;
                }
                _ => return Err(self.err("expected template text or end of template")),
            }
        }
        Ok(Expr::Template { parts, exprs })
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
        // A dotted tag (`<ns.X/>`, `<Ctx.Provider>`) is always a member-access
        // element (a JSX "component" form), never a host element — regardless of
        // the base's case. React treats any dotted tag as an expression that
        // evaluates to an element type, so only a plain lowercase identifier is
        // a host element.
        let is_component = !is_fragment && (Self::is_component_name(&tag) || tag.contains('.'));
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
                    // Attribute names may contain dashes (`data-testid`,
                    // `aria-label`): consume `-ident` segments.
                    let mut pname = name.clone();
                    self.advance()?;
                    while self.check(&TokenKind::Minus) {
                        self.advance()?;
                        pname.push('-');
                        pname.push_str(&self.expect_ident()?);
                    }
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

/// Wrap a plain identifier as a `Param` (shim until full pattern parsing
/// lands with the statement-grammar work).
fn plain_param(name: String) -> Param {
    Param {
        pattern: Pattern::Name {
            name,
            default: None,
        },
        default: None,
        rest: false,
    }
}

/// Lower a `let`/`const` binding to an assignment expression for block bodies
/// (arrow bodies, try/catch/finally blocks): plain names assign directly;
/// destructuring patterns assign a `$bind` temp (the runtime expands it).
/// Full destructuring expansion in block bodies arrives with T10 lowering;
/// until then a destructuring `let` in a block body is a precise lower
/// error, not a silent miscompile — EXCEPT the parser still accepts the
/// syntax (the error surfaces at the right stage with position context).
fn lower_binding_to_assign(pattern: Pattern, value: Expr) -> Expr {
    match pattern {
        Pattern::Name { name, .. } => Expr::Assign {
            target: Box::new(Expr::Ident {
                name,
                is_component: false,
            }),
            value: Box::new(value),
        },
        // Destructuring in a block body: bind the whole value to a temp the
        // lowerer will expand. `$bind` is reserved (never a user binding).
        _ => Expr::Assign {
            target: Box::new(Expr::Ident {
                name: "$bind".to_string(),
                is_component: false,
            }),
            value: Box::new(value),
        },
    }
}
