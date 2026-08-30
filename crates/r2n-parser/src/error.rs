//! Error type for lexing and parsing, plus rendered diagnostics.

use std::fmt;

/// A parse error with a position (line, column) and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub fn new(line: usize, column: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            column,
            message: message.into(),
        }
    }

    /// Render this error against the original source: the offending line
    /// with a caret pointing at the error column. `source` must be the text
    /// the error was produced from, or the render is meaningless.
    pub fn render(&self, source: &str) -> String {
        let line_text = source
            .lines()
            .nth(self.line.saturating_sub(1))
            .unwrap_or("<source line not found>");
        let gutter = " ".repeat(self.line.to_string().len());
        // Columns are 1-based; tabs expand to spaces so the caret still
        // lands under the right character.
        let prefix: String = line_text
            .chars()
            .take(self.column.saturating_sub(1))
            .map(|c| {
                if c == '\t' {
                    "    ".to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();
        let width = line_text
            .chars()
            .nth(self.column.saturating_sub(1))
            .map(|c| if c == '\t' { 4 } else { 1 })
            .unwrap_or(1);
        format!(
            "{gutter} |\n{} | {line_text}\n{gutter} | {prefix}{}",
            self.line,
            "^".repeat(width)
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "parse error at {}:{}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}
