//! Tokenizer for the R2N source subset.
//!
//! Hand-written lexer (not a regex hack) producing a precise token stream with
//! source positions so the parser can emit good errors. It is based on a
//! remaining-source slice (`rest`) rather than an iterator, which makes it
//! `Copy` — so the parser can cheaply clone it to perform multi-token
//! lookahead (needed to disambiguate `(x) => arrow` from `(expr)` grouping).

use crate::error::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // literals
    Int(i64),
    Float(f64),
    String(String),
    // identifiers / keywords
    Ident(String),
    // punctuation
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Semicolon,
    Equals,
    Arrow, // =>
    // JSX-ish
    Lt,      // <
    Gt,      // >
    Slash,   // /
    LtSlash, // </
    // operators
    Plus,
    Minus,
    Star,
    Percent,
    Bang,
    EqEq,
    EqEqEq,
    BangEq,
    BangEqEq,
    LtEq,
    GtEq,
    AmpAmp,
    PipePipe,
    Question,
    // meta
    Eof,
}

impl TokenKind {
    /// Human-readable name for diagnostics: `` `;` `` rather than `Semicolon`.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(i) => format!("number `{i}`"),
            TokenKind::Float(f) => format!("number `{f}`"),
            TokenKind::String(s) => format!("string \"{s}\""),
            TokenKind::Ident(n) => format!("`{n}`"),
            TokenKind::LeftParen => "`(`".into(),
            TokenKind::RightParen => "`)`".into(),
            TokenKind::LeftBrace => "`{`".into(),
            TokenKind::RightBrace => "`}`".into(),
            TokenKind::LeftBracket => "`[`".into(),
            TokenKind::RightBracket => "`]`".into(),
            TokenKind::Comma => "`,`".into(),
            TokenKind::Dot => "`.`".into(),
            TokenKind::Colon => "`:`".into(),
            TokenKind::Semicolon => "`;`".into(),
            TokenKind::Equals => "`=`".into(),
            TokenKind::Arrow => "`=>`".into(),
            TokenKind::Lt => "`<`".into(),
            TokenKind::Gt => "`>`".into(),
            TokenKind::Slash => "`/`".into(),
            TokenKind::LtSlash => "`</`".into(),
            TokenKind::Plus => "`+`".into(),
            TokenKind::Minus => "`-`".into(),
            TokenKind::Star => "`*`".into(),
            TokenKind::Percent => "`%`".into(),
            TokenKind::Bang => "`!`".into(),
            TokenKind::EqEq => "`==`".into(),
            TokenKind::EqEqEq => "`===`".into(),
            TokenKind::BangEq => "`!=`".into(),
            TokenKind::BangEqEq => "`!==`".into(),
            TokenKind::LtEq => "`<=`".into(),
            TokenKind::GtEq => "`>=`".into(),
            TokenKind::AmpAmp => "`&&`".into(),
            TokenKind::PipePipe => "`||`".into(),
            TokenKind::Question => "`?`".into(),
            TokenKind::Eof => "end of file".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
    /// Byte offset in the source where this token starts. Lets the parser
    /// rescan raw JSX text starting at the pending token.
    pub offset: usize,
}

pub struct Lexer<'a> {
    rest: &'a str,
    line: usize,
    col: usize,
    /// The full source (kept for JSX text rescanning).
    src: &'a str,
    /// Byte offset in `src` where `rest` begins.
    offset: usize,
    /// Byte offset where the token currently being lexed started (set by
    /// `next_token` before any characters are consumed).
    token_start: usize,
    /// Line and 1-based column of the current token's FIRST character,
    /// recorded by `next_token` before any characters are consumed. `col`
    /// alone cannot express this: by the time `tok()` runs, multi-character
    /// tokens have already advanced it past the token.
    token_line: usize,
    token_col: usize,
}

impl<'a> Copy for Lexer<'a> {}
impl<'a> Clone for Lexer<'a> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Result<Self, ParseError> {
        let mut l = Self {
            rest: src,
            line: 1,
            col: 0,
            src,
            offset: 0,
            token_start: 0,
            token_line: 1,
            token_col: 1,
        };
        l.skip_trivia()?;
        Ok(l)
    }

    /// Rescan the pending token (and everything up to the next `{` or `<`)
    /// as raw JSX text, starting at byte offset `start`. Used for JSX
    /// children like `<button>+1</button>`: inside an element, everything up
    /// to the next expression or tag boundary is literal text.
    pub fn rescan_jsx_text(&mut self, start: usize) -> String {
        let bytes = self.src.as_bytes();
        let mut end = start;
        // Consume raw characters until `{`, `<`, or EOF.
        while end < bytes.len() {
            match bytes[end] {
                b'{' | b'<' => break,
                _ => end += 1,
            }
        }
        let text = self.src[start..end].to_string();
        // Advance the lexer state over the consumed text (line/col tracking
        // for multi-line children).
        for ch in text.chars() {
            if ch == '\n' {
                self.line += 1;
                self.col = 0;
            } else {
                self.col += 1;
            }
        }
        self.rest = &self.src[end..];
        self.offset = end;
        text
    }

    pub fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    pub fn peek2(&self) -> Option<char> {
        let mut it = self.rest.chars();
        it.next();
        it.next()
    }

    fn err<T>(&self, msg: &str) -> Result<T, ParseError> {
        Err(ParseError::new(self.line, self.col, msg.to_string()))
    }

    fn tok(&self, kind: TokenKind) -> Token {
        Token {
            kind,
            line: self.token_line,
            column: self.token_col,
            offset: self.token_start,
        }
    }

    /// Advance past whitespace and comments.
    fn skip_trivia(&mut self) -> Result<(), ParseError> {
        loop {
            match self.rest.chars().next() {
                Some(c) if c.is_whitespace() => {
                    self.consume_char();
                }
                Some('/') => {
                    // Peek the char AFTER this '/' to decide comment type.
                    // `rest.chars().next()` would re-read '/' every time, so use
                    // the second char of the remaining source directly.
                    let after = self.rest.chars().nth(1);
                    match after {
                        Some('/') => {
                            // line comment: consume '//' and until newline
                            for _ in 0..2 {
                                self.consume_char();
                            }
                            while let Some(c) = self.rest.chars().next() {
                                if c == '\n' {
                                    break;
                                }
                                self.consume_char();
                            }
                        }
                        Some('*') => {
                            for _ in 0..2 {
                                self.consume_char();
                            }
                            let mut depth = 1usize;
                            while depth > 0 {
                                match self.rest.chars().next() {
                                    None => return self.err("unterminated block comment"),
                                    Some('*') => {
                                        self.consume_char();
                                        if self.rest.starts_with('/') {
                                            self.consume_char();
                                            depth -= 1;
                                        }
                                    }
                                    Some('/') => {
                                        self.consume_char();
                                        if self.rest.starts_with('*') {
                                            self.consume_char();
                                            depth += 1;
                                        }
                                    }
                                    Some(_) => {
                                        self.consume_char();
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Consume one `char`, updating `rest`, `line`, `col`, `offset`.
    fn consume_char(&mut self) {
        let c = self.rest.chars().next().expect("consume_char on empty");
        let len = c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += 1;
        }
        self.rest = &self.rest[len..];
        self.offset += len;
    }

    /// Produce the next token, advancing `rest` past it.
    pub fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_trivia()?;
        self.token_start = self.offset;
        self.token_line = self.line;
        // `col` is 0-based until the first char of a line is consumed;
        // token columns are 1-based like editor positions.
        self.token_col = self.col + 1;
        let c = match self.rest.chars().next() {
            None => return Ok(self.tok(TokenKind::Eof)),
            Some(c) => c,
        };

        if c.is_ascii_digit()
            || (c == '.'
                && self
                    .rest
                    .chars()
                    .nth(1)
                    .map(|d| d.is_ascii_digit())
                    .unwrap_or(false))
        {
            return self.lex_number();
        }
        if c == '"' {
            return self.lex_string();
        }
        if c.is_ascii_alphabetic() || c == '_' {
            return self.lex_ident();
        }

        // two-char operators first
        if let Some(tok) = self.try_two_char()? {
            return Ok(tok);
        }

        let kind = match c {
            '(' => TokenKind::LeftParen,
            ')' => TokenKind::RightParen,
            '{' => TokenKind::LeftBrace,
            '}' => TokenKind::RightBrace,
            '[' => TokenKind::LeftBracket,
            ']' => TokenKind::RightBracket,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semicolon,
            '=' => TokenKind::Equals,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '%' => TokenKind::Percent,
            '!' => TokenKind::Bang,
            '?' => TokenKind::Question,
            '<' => TokenKind::Lt,
            '>' => TokenKind::Gt,
            '/' => TokenKind::Slash,
            _ => return self.err(&format!("unexpected character '{c}'")),
        };
        let tok = self.tok(kind);
        self.consume_char();
        Ok(tok)
    }

    fn lex_number(&mut self) -> Result<Token, ParseError> {
        let start = self.rest;
        let mut is_float = false;
        while let Some(c) = self.rest.chars().next() {
            if c.is_ascii_digit() {
                self.consume_char();
            } else if c == '.' && !is_float {
                if self
                    .rest
                    .chars()
                    .nth(1)
                    .map(|d| d.is_ascii_digit())
                    .unwrap_or(false)
                {
                    is_float = true;
                    self.consume_char();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        let s = &start[..start.len() - self.rest.len()];
        let tok = if is_float {
            match s.parse::<f64>() {
                Ok(v) => self.tok(TokenKind::Float(v)),
                Err(_) => return self.err(&format!("invalid float literal '{s}'")),
            }
        } else {
            match s.parse::<i64>() {
                Ok(v) => self.tok(TokenKind::Int(v)),
                Err(_) => return self.err(&format!("invalid integer literal '{s}'")),
            }
        };
        Ok(tok)
    }

    fn lex_string(&mut self) -> Result<Token, ParseError> {
        self.consume_char(); // opening quote
        let mut buf = String::new();
        loop {
            match self.rest.chars().next() {
                None => return self.err("unterminated string literal"),
                Some('"') => {
                    self.consume_char();
                    break;
                }
                Some('\\') => {
                    self.consume_char();
                    match self.rest.chars().next() {
                        None => return self.err("unterminated string escape"),
                        Some('n') => {
                            self.consume_char();
                            buf.push('\n');
                        }
                        Some('t') => {
                            self.consume_char();
                            buf.push('\t');
                        }
                        Some('r') => {
                            self.consume_char();
                            buf.push('\r');
                        }
                        Some('"') => {
                            self.consume_char();
                            buf.push('"');
                        }
                        Some('\\') => {
                            self.consume_char();
                            buf.push('\\');
                        }
                        Some(other) => {
                            self.consume_char();
                            buf.push('\\');
                            buf.push(other);
                        }
                    }
                }
                Some(c) => {
                    self.consume_char();
                    buf.push(c);
                }
            }
        }
        Ok(self.tok(TokenKind::String(buf)))
    }

    fn lex_ident(&mut self) -> Result<Token, ParseError> {
        let start = self.rest;
        while let Some(c) = self.rest.chars().next() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.consume_char();
            } else {
                break;
            }
        }
        let s = &start[..start.len() - self.rest.len()];
        Ok(self.tok(TokenKind::Ident(s.to_string())))
    }

    fn try_two_char(&mut self) -> Result<Option<Token>, ParseError> {
        let c = self.rest.chars().next().unwrap();
        let kind = match c {
            '=' if self.rest.chars().nth(1) == Some('=') => {
                if self.rest.chars().nth(2) == Some('=') {
                    Some(TokenKind::EqEqEq)
                } else {
                    Some(TokenKind::EqEq)
                }
            }
            '!' if self.rest.chars().nth(1) == Some('=') => {
                if self.rest.chars().nth(2) == Some('=') {
                    Some(TokenKind::BangEqEq)
                } else {
                    Some(TokenKind::BangEq)
                }
            }
            '<' if self.rest.chars().nth(1) == Some('=') => Some(TokenKind::LtEq),
            '>' if self.rest.chars().nth(1) == Some('=') => Some(TokenKind::GtEq),
            '&' if self.rest.chars().nth(1) == Some('&') => Some(TokenKind::AmpAmp),
            '|' if self.rest.chars().nth(1) == Some('|') => Some(TokenKind::PipePipe),
            '=' if self.rest.chars().nth(1) == Some('>') => Some(TokenKind::Arrow),
            '<' if self.rest.chars().nth(1) == Some('/') => Some(TokenKind::LtSlash),
            _ => None,
        };
        if let Some(kind) = kind {
            let tok = self.tok(kind);
            // `===` / `!==` are three chars; every other match here is two.
            let width = if matches!(tok.kind, TokenKind::EqEqEq | TokenKind::BangEqEq) {
                3
            } else {
                2
            };
            for _ in 0..width {
                self.consume_char();
            }
            Ok(Some(tok))
        } else {
            Ok(None)
        }
    }
}
