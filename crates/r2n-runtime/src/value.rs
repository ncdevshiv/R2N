//! Runtime values — the ABI-level value set the zero-JS runtime manipulates.
//!
//! Per ADR-009, strings are UTF-16 (stored as `Vec<u16>`); numbers use f64 with
//! ECMA-exact formatting on output (see `format_number`). There is no JS object
//! graph with prototype chains; instead we have a small closed set: null,
//! bool, number, utf16 string, array, map (for props/objects), component
//! reference (a closure handle), and a "node handle" placeholder used by
//! renderers. This is the *language-neutral* value vocabulary the ABI permits.

use std::collections::BTreeMap;
use std::fmt;

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Numbers are always f64 (ECMAScript `Number`). Integers up to 2^53 are
    /// representable exactly; we keep f64 for simplicity and ECMA semantics.
    Number(f64),
    /// A UTF-16 string (ADR-009). `Vec<u16>` so `.length` is code-unit exact.
    Str(Vec<u16>),
    /// A homogeneous-indexed array (ECMAScript `Array`).
    Array(Vec<Value>),
    /// A string-keyed map (stands in for JS objects / props bags).
    Map(BTreeMap<String, Value>),
    /// An event handler: a closure paired with the component instance path
    /// whose hook frame (and env) it must run against. This is the ABI form of
    /// `onClick={() => setCount(n + 1)}` — handlers are values, so they ride
    /// through `SetProp` patches like any other prop and stay serializable
    /// (path + closure, no Rust function pointers).
    Handler {
        /// Instance path of the component whose scope the closure runs in.
        inst_path: Vec<String>,
        /// The handler body (a `JsExpr::Closure`).
        body: Box<r2n_ir::js::JsExpr>,
    },
    /// A state setter handle produced by `useState`. Carries the hook-frame
    /// slot index (the frame-protocol callback channel). It is `Copy` and
    /// serializable, so it crosses the ABI boundary as a plain value.
    Setter(crate::hooks::Setter),
    /// A `useReducer` dispatch handle: carries the hook-frame slot index of
    /// the stored reducer. Calling it evaluates `reducer(state, action)` in
    /// the frame and writes the result — the reducer body is IR data, never
    /// a Rust function pointer.
    Dispatcher {
        slot: usize,
    },
    /// The `children` prop: pre-lowered React-IR nodes passed through a
    /// component call. The nodes are pure data (they may reference the
    /// PARENT's scope; the runtime evaluates them against the parent's
    /// saved scope at splice time — see engine's `ReactNode::Children`).
    /// Serializable like every other ABI value; no Rust function pointers.
    Children(Vec<r2n_ir::react::ReactNode>),
}

impl Value {
    pub fn null() -> Self {
        Value::Null
    }

    pub fn from_str_utf8(s: &str) -> Self {
        Value::Str(s.encode_utf16().collect())
    }

    pub fn as_str_utf8(&self) -> Option<String> {
        match self {
            Value::Str(u) => Some(String::from_utf16_lossy(u)),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Map(m) => !m.is_empty(),
            Value::Handler { .. } => true,
            Value::Setter(_) => true,
            Value::Dispatcher { .. } => true,
            Value::Children(_) => true,
        }
    }

    /// Truthiness per ECMAScript ToBoolean.
    pub fn is_truthy(&self) -> bool {
        self.as_bool()
    }

    /// A stable display of the value, used by tests and the debug renderer.
    pub fn display(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => format_number(*n),
            Value::Str(u) => String::from_utf16_lossy(u),
            Value::Array(items) => {
                let parts: Vec<String> = items.iter().map(|i| i.display()).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Map(m) => {
                let parts: Vec<String> = m
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.display()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::Handler { .. } => "<handler>".to_string(),
            Value::Setter(s) => format!("<setter#{}>", s.frame_index),
            Value::Dispatcher { slot } => format!("<dispatch#{slot}>"),
            Value::Children(_) => "<children>".to_string(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

/// ECMA-262 Number formatting: integers print without a decimal point;
/// non-integers print their shortest round-trippable representation.
pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n < 0.0 {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        };
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        // Print as integer without trailing ".0".
        format!("{}", n as i64)
    } else {
        // Use Rust's default which is shortest round-trippable for f64.
        format!("{}", n)
    }
}

/// Type errors raised by evaluation (e.g. calling a non-function).
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError(String);

impl RuntimeError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime error: {}", self.0)
    }
}

impl std::error::Error for RuntimeError {}
