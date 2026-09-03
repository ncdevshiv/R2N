//! Binary and unary operators supported by the language.

/// Binary operators (both arithmetic and comparison/logical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    /// `===` (strict equality — no coercion, objects by identity).
    StrictEq,
    /// `!==`
    StrictNeq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    Nullish,
    /// `|` — bitwise OR on ToInt32-coerced operands (ECMA 13.11).
    BitOr,
}

impl BinOp {
    /// Returns the operator's textual spelling as it appears in source.
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "==",
            BinOp::StrictEq => "===",
            BinOp::Neq => "!=",
            BinOp::StrictNeq => "!==",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Nullish => "??",
            BinOp::BitOr => "|",
        }
    }
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        }
    }
}
