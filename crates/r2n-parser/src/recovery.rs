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
    ClassComponent, Component, Decl, DeclKind, ExportDecl, ExportNamed, FuncDecl, Import, Method,
    ObjectProp, Param, Pattern, Program, Stmt,
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

/// Tokenize `src`, driving the template chunk protocol inline: after a
/// TemplateStart, pull chunks via `lex_template_chunk`; after each
/// interpolation-closing `}`, resume chunks. Brace depth tracks when a `}`
/// closes an interpolation (next is chunk text) vs ordinary code. Nested
/// templates recurse. Without this, a raw `next_token` loop would lex `${`
/// chunk text as normal tokens (`$` → error).
fn lex_all_with_templates(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut lexer = Lexer::new(src)?;
    let mut tokens: Vec<Token> = Vec::new();
    // Stack of brace depths at which each open `${` started: a `}` seen at
    // exactly that depth closes the interpolation.
    let mut interp_stack: Vec<usize> = Vec::new();
    let mut brace_depth: usize = 0;
    // Chunk-mode stack: each TemplateStart pushes a frame; TemplateEnd pops.
    // While non-empty, the next token comes from `lex_template_chunk`.
    let mut in_template = false;
    loop {
        if in_template {
            let chunk = lexer.lex_template_chunk()?;
            let is_end = matches!(chunk.kind, TokenKind::TemplateEnd);
            // A chunk ending at the backtick arrives as TemplateText with
            // the End parked — detect via probe.
            let mut text_end = false;
            if matches!(chunk.kind, TokenKind::TemplateText(_)) {
                let mut probe = lexer.clone();
                if let Ok(t) = probe.next_token() {
                    text_end = matches!(t.kind, TokenKind::TemplateEnd);
                }
            }
            tokens.push(chunk);
            if is_end {
                in_template = false;
                continue;
            }
            if text_end {
                tokens.push(lexer.next_token()?);
                in_template = false;
                continue;
            }
            // Middle chunk: `${` consumed — interpolation expression tokens
            // follow until the matching `}`.
            interp_stack.push(brace_depth);
            in_template = false;
            continue;
        }
        let tok = lexer.next_token()?;
        let eof = matches!(tok.kind, TokenKind::Eof);
        match &tok.kind {
            TokenKind::TemplateStart => {
                tokens.push(tok);
                in_template = true;
            }
            TokenKind::LeftBrace => {
                brace_depth += 1;
                tokens.push(tok);
            }
            TokenKind::RightBrace => {
                // A `}` at exactly the innermost interpolation's depth closes
                // it: the next token resumes chunk text.
                if Some(&brace_depth) == interp_stack.last() {
                    interp_stack.pop();
                    tokens.push(tok);
                    in_template = true;
                } else {
                    brace_depth = brace_depth.saturating_sub(1);
                    tokens.push(tok);
                }
            }
            _ => tokens.push(tok),
        }
        if eof {
            break;
        }
    }
    Ok(tokens)
}

/// Parse `src`, collecting every recoverable error instead of stopping at
/// the first. Lexer-level failures (unterminated string/comment) are still
/// fatal: there is no sane position to resume tokenization from.
pub fn parse_with_recovery(src: &str) -> Result<Recovered, ParseError> {
    let tokens = lex_all_with_templates(src)?;

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
            TokenKind::Ident(kw) if kw == "function" => self.parse_function_decl(),
            TokenKind::Ident(kw) if kw == "let" || kw == "const" => {
                let kind = if kw == "let" {
                    DeclKind::Let
                } else {
                    DeclKind::Const
                };
                self.bump();
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

    /// `function name(params) { stmts }` or `function* name(params) { stmts }`
    /// (mirrors parser.rs).
    fn parse_function_decl(&mut self) -> Result<Decl, ParseError> {
        self.bump(); // `function`
        if self.check(&TokenKind::Star) {
            return self.parse_generator_fn_tail();
        }
        let name = self.expect_ident()?;
        let params = self.parse_param_patterns()?;
        let body = self.parse_stmt_block()?;
        Ok(Decl::FuncDecl(FuncDecl { name, params, body }))
    }

    /// `function* name(params) { stmts }` (mirrors parser.rs).
    fn parse_generator_fn_tail(&mut self) -> Result<Decl, ParseError> {
        self.bump(); // `*`
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

    /// `(p1, p2, ...)` — full parameter patterns (mirrors parser.rs).
    fn parse_param_patterns(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(TokenKind::LeftParen)?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.parse_param()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
                if self.check(&TokenKind::RightParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        for (i, p) in params.iter().enumerate() {
            if p.rest && i + 1 != params.len() {
                return Err(self.err("rest parameter must be last"));
            }
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        if self.check(&TokenKind::DotDotDot) {
            self.bump();
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
            self.bump();
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

    /// Plain identifier parameter (kept for error-message parity in
    /// diagnostics paths).
    #[allow(dead_code)]
    fn parse_plain_param(&mut self) -> Result<Param, ParseError> {
        let name = self.expect_ident()?;
        Ok(Param {
            pattern: Pattern::Name {
                name,
                default: None,
            },
            default: None,
            rest: false,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.parse_pattern_inner(false)
    }

    fn parse_binding_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.parse_pattern_inner(true)
    }

    fn parse_pattern_inner(&mut self, in_binding: bool) -> Result<Pattern, ParseError> {
        match &self.cur().kind {
            TokenKind::LeftBrace => self.parse_object_pattern(),
            TokenKind::LeftBracket => self.parse_array_pattern(),
            _ => {
                let name = self.expect_ident()?;
                let default = if self.check(&TokenKind::Equals) && !in_binding {
                    self.bump();
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
                self.bump();
                rest = Some(self.expect_ident()?);
                if self.check(&TokenKind::Comma) {
                    self.bump();
                }
                break;
            }
            let key = self.expect_ident()?;
            let alias = if self.check(&TokenKind::Colon) {
                self.bump();
                Some(self.parse_pattern()?)
            } else if self.check(&TokenKind::Equals) {
                self.bump();
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
                self.bump();
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
                self.bump();
                items.push(None);
                continue;
            }
            if self.check(&TokenKind::DotDotDot) {
                self.bump();
                rest = Some(self.expect_ident()?);
                if self.check(&TokenKind::Comma) {
                    self.bump();
                }
                break;
            }
            items.push(Some(self.parse_pattern()?));
            if self.check(&TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBracket)?;
        Ok(Pattern::Array { items, rest })
    }

    fn parse_binding(&mut self) -> Result<(Pattern, Expr), ParseError> {
        let pattern = self.parse_binding_pattern()?;
        self.expect(TokenKind::Equals)?;
        let value = self.parse_expr()?;
        Ok((pattern, value))
    }

    /// `function* name(params) { stmts }` (mirrors parser.rs). Kept for
    /// error-message parity; `parse_function_decl` above handles dispatch.
    /// Legacy `function*` entry (kept for error-message parity in
    /// diagnostics tests; `parse_function_decl` handles dispatch).
    #[allow(dead_code)]
    fn parse_generator_fn(&mut self) -> Result<Decl, ParseError> {
        self.bump(); // `function`
        if !self.check(&TokenKind::Star) {
            return Err(self
                .err("expected `function*` (only generator function declarations are supported)"));
        }
        self.bump(); // `*`
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
                    let params = self.parse_param_patterns()?;
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
                    let exported = if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "as")
                    {
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
            // `export function name(params) { ... }` — inline-exported
            // function (mirrors parser.rs).
            TokenKind::Ident(kw) if kw == "function" => match self.parse_function_decl()? {
                Decl::FuncDecl(f) => Ok(Decl::ExportDecl(ExportDecl::Function(f))),
                Decl::GeneratorFn(g) => Ok(Decl::GeneratorFn(g)),
                _ => unreachable!("parse_function_decl returns FuncDecl/GeneratorFn"),
            },
            // `export const name = expr;` / `export let name = expr;`.
            TokenKind::Ident(kw) if kw == "const" || kw == "let" => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect(TokenKind::Equals)?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Decl::ExportDecl(ExportDecl::Const { name, value }))
            }
            _ => Err(self.err("expected `default` or `{` after `export`")),
        }
    }

    fn parse_component(&mut self) -> Result<Component, ParseError> {
        self.bump(); // "component"
        let name = self.expect_ident()?;
        let params = self.parse_param_patterns()?;
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
                TokenKind::Ident(kw)
                    if matches!(
                        kw.as_str(),
                        "let"
                            | "const"
                            | "return"
                            | "if"
                            | "while"
                            | "for"
                            | "switch"
                            | "break"
                            | "continue"
                    ) =>
                {
                    return
                }
                _ => self.bump(),
            }
        }
    }

    /// `{ stmts }` — a brace block of statements (mirrors parser.rs).
    fn parse_stmt_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(TokenKind::LeftBrace)?;
        let mut body = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            body.push(self.parse_stmt()?);
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(body)
    }

    /// A loop/branch body: either a `{ stmts }` block or a single statement.
    fn parse_stmt_or_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.check(&TokenKind::LeftBrace) {
            self.parse_stmt_block()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    /// `for (init; cond; update) body` (mirrors parser.rs).
    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `for`
        self.expect(TokenKind::LeftParen)?;
        let init = if self.check(&TokenKind::Semicolon) {
            self.bump();
            None
        } else if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "let" || kw == "const") {
            let kind = match &self.cur().kind {
                TokenKind::Ident(kw) if kw == "let" => DeclKind::Let,
                _ => DeclKind::Const,
            };
            self.bump();
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

    /// `switch (disc) { case e: stmts...; default: stmts... }` (mirrors
    /// parser.rs).
    fn parse_switch(&mut self) -> Result<Stmt, ParseError> {
        self.bump(); // `switch`
        self.expect(TokenKind::LeftParen)?;
        let disc = self.parse_expr()?;
        self.expect(TokenKind::RightParen)?;
        self.expect(TokenKind::LeftBrace)?;
        let mut cases = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "case") {
                self.bump();
                let test = self.parse_expr()?;
                self.expect(TokenKind::Colon)?;
                let mut body = Vec::new();
                while !self.check(&TokenKind::RightBrace)
                    && !matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "case" || kw == "default")
                    && !self.check(&TokenKind::Eof)
                {
                    body.push(self.parse_stmt()?);
                }
                cases.push((Some(test), body));
            } else if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "default") {
                self.bump();
                self.expect(TokenKind::Colon)?;
                let mut body = Vec::new();
                while !self.check(&TokenKind::RightBrace)
                    && !matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "case" || kw == "default")
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
        match &self.cur().kind {
            TokenKind::Ident(kw) if kw == "let" || kw == "const" => {
                let kind = if kw == "let" {
                    DeclKind::Let
                } else {
                    DeclKind::Const
                };
                self.bump();
                let (pattern, value) = self.parse_binding()?;
                self.expect(TokenKind::Semicolon)?;
                match pattern {
                    Pattern::Name {
                        name,
                        default: None,
                    } => match kind {
                        DeclKind::Let => Ok(Stmt::Let { name, value }),
                        DeclKind::Const => Ok(Stmt::Const { name, value }),
                    },
                    Pattern::Name { name, .. } => match kind {
                        DeclKind::Let => Ok(Stmt::Let { name, value }),
                        DeclKind::Const => Ok(Stmt::Const { name, value }),
                    },
                    pattern => Ok(Stmt::Destructure {
                        kind,
                        pattern,
                        value,
                    }),
                }
            }
            TokenKind::Ident(kw) if kw == "if" => {
                self.bump();
                self.expect(TokenKind::LeftParen)?;
                let cond = self.parse_expr()?;
                self.expect(TokenKind::RightParen)?;
                let then = self.parse_stmt_or_block()?;
                let else_ = if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "else") {
                    self.bump();
                    Some(self.parse_stmt_or_block()?)
                } else {
                    None
                };
                Ok(Stmt::If { cond, then, else_ })
            }
            TokenKind::Ident(kw) if kw == "while" => {
                self.bump();
                self.expect(TokenKind::LeftParen)?;
                let cond = self.parse_expr()?;
                self.expect(TokenKind::RightParen)?;
                let body = self.parse_stmt_or_block()?;
                Ok(Stmt::While { cond, body })
            }
            TokenKind::Ident(kw) if kw == "for" => self.parse_for(),
            TokenKind::Ident(kw) if kw == "switch" => self.parse_switch(),
            TokenKind::Ident(kw) if kw == "break" => {
                self.bump();
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Break)
            }
            TokenKind::Ident(kw) if kw == "continue" => {
                self.bump();
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Continue)
            }
            TokenKind::Ident(kw) if kw == "return" => {
                self.bump();
                // Bare `return;` returns `undefined` (mirrors parser.rs).
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                    return Ok(Stmt::Return(Expr::Literal(
                        r2n_ast::lit::Literal::Undefined,
                    )));
                }
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
        let target = self.parse_nullish()?;
        let compound = match &self.cur().kind {
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
            self.bump();
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
            self.bump();
            let value = self.parse_assign()?;
            return Ok(Expr::Assign {
                target: Box::new(target),
                value: Box::new(value),
            });
        }
        Ok(target)
    }

    /// `a ?? b` (mirrors parser.rs).
    fn parse_nullish(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_ternary()?;
        while self.check(&TokenKind::QuestionQuestion) {
            self.bump();
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
        let mut left = self.parse_bitor()?;
        while self.check(&TokenKind::PipePipe) {
            self.bump();
            let right = self.parse_bitor()?;
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// `a | b` — bitwise OR (mirrors parser.rs).
    fn parse_bitor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.check(&TokenKind::Pipe) {
            self.bump();
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
                // Postfix `x++` / `x--` (mirrors parser.rs).
                TokenKind::PlusPlus | TokenKind::MinusMinus => {
                    let op = match &self.cur().kind {
                        TokenKind::PlusPlus => r2n_ast::expr::UpdateOp::Inc,
                        _ => r2n_ast::expr::UpdateOp::Dec,
                    };
                    let is_assignable = matches!(&expr, Expr::Ident { .. } | Expr::Member { .. });
                    if !is_assignable {
                        return Err(
                            self.err("update target must be an identifier or a member access")
                        );
                    }
                    self.bump();
                    expr = Expr::Update {
                        op,
                        target: Box::new(expr),
                        prefix: false,
                    };
                }
                TokenKind::LeftParen => {
                    if self.arrow_follows() {
                        // `(params) => body` as the sole call argument,
                        // e.g. `items.map((x) => <li/>)`.
                        self.bump(); // '('
                        let params = self.parse_arrow_params()?;
                        self.expect(TokenKind::Arrow)?;
                        let body = self.parse_arrow_body()?;
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args: vec![r2n_ast::expr::CallArg::Expr(Expr::Arrow {
                                params,
                                body: Box::new(body),
                                async_: false,
                            })],
                        };
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
                    self.bump();
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
        // Depth-tracked scan accepting full patterns (mirrors parser.rs's
        // looks_like_arrow): idents, braces/brackets, `...`, `=`, `:` and
        // nested literals; the `)` at depth 0 must be followed by `=>`.
        let mut j = self.pos + 1;
        let mut depth = 0usize;
        loop {
            match self.tokens.get(j).map(|t| &t.kind) {
                Some(TokenKind::LeftParen)
                | Some(TokenKind::LeftBrace)
                | Some(TokenKind::LeftBracket) => {
                    depth += 1;
                    j += 1;
                }
                Some(TokenKind::RightParen) if depth == 0 => {
                    return matches!(
                        self.tokens.get(j + 1).map(|t| &t.kind),
                        Some(TokenKind::Arrow)
                    );
                }
                Some(TokenKind::RightParen)
                | Some(TokenKind::RightBrace)
                | Some(TokenKind::RightBracket) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                    j += 1;
                }
                Some(TokenKind::Ident(_))
                | Some(TokenKind::Comma)
                | Some(TokenKind::DotDotDot)
                | Some(TokenKind::Equals)
                | Some(TokenKind::Colon)
                | Some(TokenKind::Int(_))
                | Some(TokenKind::Float(_))
                | Some(TokenKind::String(_)) => j += 1,
                _ => return false,
            }
        }
    }

    /// Lookahead: is the current ident immediately followed by `=>`?
    fn ident_arrow_follows(&self) -> bool {
        matches!(
            self.tokens.get(self.pos + 1).map(|t| &t.kind),
            Some(TokenKind::Arrow)
        )
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
                self.bump();
                Ok(Expr::Literal(Literal::Null))
            }
            TokenKind::LeftParen => {
                if self.arrow_follows() {
                    self.bump(); // '('
                    let params = self.parse_arrow_params()?;
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
            // Single-ident arrow: `x => expr` (mirrors parser.rs).
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
                ) && self.ident_arrow_follows() =>
            {
                let n = name.clone();
                self.bump();
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
            TokenKind::Ident(name) if name == "async" && self.async_arrow_follows() => {
                // `async (params) => body` / `async x => body` (mirrors parser.rs).
                self.bump(); // `async`
                let params = if self.check(&TokenKind::LeftParen) {
                    self.bump();
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
                // `await expr` (mirrors parser.rs).
                self.bump();
                let value = self.parse_expr()?;
                Ok(Expr::Await {
                    value: Box::new(value),
                    from_return: false,
                })
            }
            TokenKind::Ident(name) if name == "function" => {
                // `function Name?(params) { stmts }` in expression position
                // (mirrors parser.rs).
                self.bump(); // `function`
                let name = if matches!(&self.cur().kind, TokenKind::Ident(_)) {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                let params = self.parse_param_patterns()?;
                let body = self.parse_stmt_block()?;
                Ok(Expr::Function { name, params, body })
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
            TokenKind::LeftBrace => self.parse_object(),
            TokenKind::TemplateStart => self.parse_template(),
            TokenKind::Lt => self.parse_element(),
            other => Err(self.err(&format!("unexpected {} in expression", other.describe()))),
        }
    }

    /// `{key: value, shorthand, ...spread}` (mirrors parser.rs).
    fn parse_object(&mut self) -> Result<Expr, ParseError> {
        use r2n_ast::expr::ObjectItem;
        self.expect(TokenKind::LeftBrace)?;
        let mut items = Vec::new();
        while !self.check(&TokenKind::RightBrace) {
            if self.check(&TokenKind::DotDotDot) {
                self.bump();
                items.push(ObjectItem::Spread(self.parse_expr()?));
            } else {
                let key = match &self.cur().kind {
                    TokenKind::String(s) => {
                        let k = s.clone();
                        self.bump();
                        k
                    }
                    _ => self.expect_ident()?,
                };
                if self.check(&TokenKind::Colon) {
                    self.bump();
                    items.push(ObjectItem::Prop(key, self.parse_expr()?));
                } else {
                    items.push(ObjectItem::Shorthand(key));
                }
            }
            if self.check(&TokenKind::Comma) {
                self.bump();
                if self.check(&TokenKind::RightBrace) {
                    break;
                }
            } else {
                break;
            }
        }
        self.expect(TokenKind::RightBrace)?;
        Ok(Expr::Object(items))
    }

    /// `` `head${expr}tail` `` (mirrors parser.rs). The recovery twin works
    /// over a flat token list, so template chunks arrive pre-lexed as
    /// TemplateText/TemplateEnd tokens — no `lex_template_chunk` calls.
    fn parse_template(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // TemplateStart
        let mut parts = Vec::new();
        let mut exprs = Vec::new();
        loop {
            match &self.cur().kind {
                TokenKind::TemplateText(text) => {
                    let text = text.clone();
                    self.bump();
                    parts.push(text);
                    // Middle chunk: an expression follows (it ends at `}`).
                    // Trailing chunk: TemplateEnd follows directly.
                    if self.check(&TokenKind::TemplateEnd) {
                        continue;
                    }
                    let e = self.parse_expr()?;
                    self.expect(TokenKind::RightBrace)?;
                    exprs.push(e);
                }
                TokenKind::TemplateEnd => {
                    self.bump();
                    // A template ending in an interpolation has no trailing
                    // text: the last pushed part belongs to the previous
                    // chunk... reconcile counts: parts must be exprs+1.
                    while parts.len() <= exprs.len() {
                        parts.push(String::new());
                    }
                    break;
                }
                _ => return Err(self.err("expected template text or end of template")),
            }
        }
        Ok(Expr::Template { parts, exprs })
    }

    /// Arrow params after `(` was consumed: full patterns (mirrors parser.rs).
    fn parse_arrow_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            params.push(self.parse_param()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
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
                self.bump();
                if self.check(&TokenKind::RightParen) {
                    break;
                }
                params.push(self.parse_param()?);
            }
        }
        Ok(params)
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
                // `let`/`const` (incl. destructuring) inside a block-bodied
                // arrow: scoped locals (mirrors parser.rs).
                if matches!(&self.cur().kind, TokenKind::Ident(kw) if kw == "let" || kw == "const")
                {
                    self.bump();
                    let (pattern, value) = self.parse_binding()?;
                    if self.check(&TokenKind::Semicolon) {
                        self.bump();
                    }
                    stmts.push(lower_binding_to_assign(pattern, value));
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
                let (pattern, value) = self.parse_binding()?;
                if self.check(&TokenKind::Semicolon) {
                    self.bump();
                }
                stmts.push(lower_binding_to_assign(pattern, value));
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
            items.push(self.parse_array_item()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
                if self.check(&TokenKind::RightBracket) {
                    break; // trailing comma
                }
                items.push(self.parse_array_item()?);
            }
        }
        self.expect(TokenKind::RightBracket)?;
        Ok(Expr::Array(items))
    }

    fn parse_array_item(&mut self) -> Result<r2n_ast::expr::ArrayItem, ParseError> {
        if self.check(&TokenKind::DotDotDot) {
            self.bump();
            Ok(r2n_ast::expr::ArrayItem::Spread(self.parse_expr()?))
        } else {
            Ok(r2n_ast::expr::ArrayItem::Expr(self.parse_expr()?))
        }
    }

    /// `(args...)` — parenthesized call arguments with `...spread` and
    /// trailing-comma support. Consumes both parens.
    fn parse_call_args(&mut self) -> Result<Vec<r2n_ast::expr::CallArg>, ParseError> {
        self.expect(TokenKind::LeftParen)?;
        let mut args = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            args.push(self.parse_call_arg()?);
            while self.check(&TokenKind::Comma) {
                self.bump();
                if self.check(&TokenKind::RightParen) {
                    break;
                }
                args.push(self.parse_call_arg()?);
            }
        }
        self.expect(TokenKind::RightParen)?;
        Ok(args)
    }

    fn parse_call_arg(&mut self) -> Result<r2n_ast::expr::CallArg, ParseError> {
        if self.check(&TokenKind::DotDotDot) {
            self.bump();
            Ok(r2n_ast::expr::CallArg::Spread(self.parse_expr()?))
        } else {
            Ok(r2n_ast::expr::CallArg::Expr(self.parse_expr()?))
        }
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
                    // Attribute names may contain dashes (`data-testid`,
                    // `aria-label`): consume `-ident` segments (mirrors
                    // parser.rs).
                    let mut pname = name.clone();
                    self.bump();
                    while self.check(&TokenKind::Minus) {
                        self.bump();
                        pname.push('-');
                        pname.push_str(&self.expect_ident()?);
                    }
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

/// Wrap a plain identifier as a `Param` (kept for error-message parity in
/// diagnostics paths).
#[allow(dead_code)]
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
/// (mirrors parser.rs).
fn lower_binding_to_assign(pattern: Pattern, value: Expr) -> Expr {
    match pattern {
        Pattern::Name { name, .. } => Expr::Assign {
            target: Box::new(Expr::Ident {
                name,
                is_component: false,
            }),
            value: Box::new(value),
        },
        _ => Expr::Assign {
            target: Box::new(Expr::Ident {
                name: "$bind".to_string(),
                is_component: false,
            }),
            value: Box::new(value),
        },
    }
}
