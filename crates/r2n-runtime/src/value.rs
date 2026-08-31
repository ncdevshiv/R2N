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

/// An ECMAScript symbol: identity by id; `Symbol.for(key)` symbols share
/// the id of their key (registered). Empty `key` = anonymous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub id: u64,
    /// The `Symbol.for` key, if registered.
    pub key: Option<String>,
}

impl Symbol {
    pub fn display(&self) -> String {
        match &self.key {
            Some(k) => format!("Symbol({k})"),
            None => format!("Symbol({})", self.id),
        }
    }
}

/// Object data: own properties and the prototype link (None = no
/// prototype — `Object.create(null)` semantics).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjData {
    pub props: BTreeMap<String, Value>,
    pub proto: Option<std::rc::Rc<std::cell::RefCell<ObjData>>>,
}

impl ObjData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Own property lookup.
    pub fn get_own(&self, prop: &str) -> Option<Value> {
        self.props.get(prop).cloned()
    }

    /// Set an own property (ECMA: writes create own data props).
    pub fn set_own(&mut self, prop: String, value: Value) {
        self.props.insert(prop, value);
    }
}

/// A runtime value. `Function` compares by reference (its captured env is
/// not a value-comparable type — two closures are the same function only
/// when they ARE the same value, JS identity semantics).
#[derive(Debug, Clone)]
pub enum Value {
    /// ECMAScript `undefined` (`typeof undefined`).
    Undefined,
    Null,
    Bool(bool),
    /// ECMAScript BigInt. i64 suffices for the supported subset; the boxed
    /// semantics (arbitrary precision) arrive with the full value model
    /// tests (M2-T01 documents the bounded representation).
    BigInt(i64),
    /// ECMAScript Symbol: identity is the runtime-unique id (same-name
    /// symbols are distinct, like `Symbol()`); registered symbols via
    /// `Symbol.for` share by key.
    Symbol(Symbol),
    /// General JS object: dynamic string-keyed properties plus a prototype
    /// chain (M2-T02). Reads walk the chain; writes create own properties.
    Object(std::rc::Rc<std::cell::RefCell<ObjData>>),
    /// A first-class function value: params + body (a `JsExpr`) invoked
    /// against its CAPTURED lexical environment (the env where the
    /// arrow was evaluated; M2-T03).
    Function {
        params: Vec<String>,
        body: Box<r2n_ir::js::JsExpr>,
        captured: crate::eval::Env,
    },
    /// An opaque external handle (host resource; renderer-bound objects).
    External(u64),
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
        /// Identity number. Plain `onX={() => ...}` handlers get 0; a
        /// `useCallback` registration gets a fresh number so its identity
        /// changes exactly when its deps change (React function identity),
        /// even though the body stays structurally identical.
        ident: u64,
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
    /// A `useRef` handle: a mutable box whose `.current` reads/writes the
    /// hook-frame slot. The value is identical across renders (same slot),
    /// and writes persist without re-render — React ref semantics.
    Ref {
        slot: usize,
    },
    /// The suspension sentinel: a component that READ a pending resource
    /// value suspends; the nearest `<Suspense fallback>` renders its
    /// fallback until the resource resolves (state flip -> re-render).
    Pending,
    /// A `createContext` handle: a runtime-unique id plus the default
    /// value passed to `createContext` (the React contract — the default
    /// lives on the handle, `useContext(Ctx)` needs no extra argument).
    /// Providers push values for this id onto the render context stack;
    /// `useContext` reads the nearest one, else the default.
    Context {
        id: u64,
        default: Box<Value>,
    },
    /// The `children` prop: pre-lowered React-IR nodes passed through a
    /// component call. The nodes are pure data (they may reference the
    /// PARENT's scope; the runtime evaluates them against the parent's
    /// saved scope at splice time — see engine's `ReactNode::Children`).
    /// Serializable like every other ABI value; no Rust function pointers.
    Children(Vec<r2n_ir::react::ReactNode>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Function { captured: a, .. }, Function { captured: b, .. }) => std::ptr::eq(a, b),
            (
                Handler {
                    inst_path: ai,
                    body: ab,
                    ident: ai_,
                },
                Handler {
                    inst_path: bi,
                    body: bb,
                    ident: bi_,
                },
            ) => ai == bi && ab == bb && ai_ == bi_,
            (Object(a), Object(b)) => std::rc::Rc::ptr_eq(a, b),
            (a, b) => {
                let _ = b;
                a.same_variant(b)
            }
        }
    }
}

impl Eq for Value {}

impl Value {
    /// Variant-by-variant equality used by the derived-style comparison
    /// (for the value types without identity semantics).
    fn same_variant(&self, other: &Value) -> bool {
        use Value::*;
        match (self, other) {
            (Undefined, Undefined) => true,
            (Null, Null) => true,
            (Bool(a), Bool(b)) => a == b,
            (Number(a), Number(b)) => a == b,
            (BigInt(a), BigInt(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            (Map(a), Map(b)) => a == b,
            (Symbol(a), Symbol(b)) => a == b,
            (External(a), External(b)) => a == b,
            (Setter(a), Setter(b)) => a == b,
            (Dispatcher { slot: a }, Dispatcher { slot: b }) => a == b,
            (Ref { slot: a }, Ref { slot: b }) => a == b,
            (Context { id: a, default: ad }, Context { id: b, default: bd }) => a == b && ad == bd,
            (Object(_), Object(_)) => false, // handled by ptr_eq above
            (Function { .. }, Function { .. }) => false, // handled above
            (Handler { .. }, Handler { .. }) => false, // handled above
            (Pending, Pending) => true,
            (Children(a), Children(b)) => a == b,
            _ => false,
        }
    }

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
        // ECMA-262 ToBoolean: falsy = undefined, null, false, ±0, NaN, ""
        // and BigInt 0n.
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::BigInt(n) => *n != 0,
            Value::Str(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Map(m) => !m.is_empty(),
            Value::Object(_) => true,
            Value::Function { .. } => true,
            Value::Symbol(_) => true,
            Value::External(_) => true,
            Value::Handler { .. } => true,
            Value::Setter(_) => true,
            Value::Dispatcher { .. } => true,
            Value::Ref { .. } => true,
            Value::Context { .. } => true,
            Value::Pending => true,
            Value::Children(_) => true,
        }
    }

    /// Truthiness per ECMAScript ToBoolean.
    pub fn is_truthy(&self) -> bool {
        self.as_bool()
    }

    /// A stable display of the value, used by tests and the debug renderer.
    pub fn display(&self) -> String {
        // ECMA-262 ToString for the supported types (BigInt/Symbol are
        // rendered distinctly; object/functions/externals use placeholders,
        // their own ToString behavior arrives with the type tasks).
        match self {
            Value::Undefined => "undefined".to_string(),
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => format_number(*n),
            Value::BigInt(n) => format!("{n}n"),
            Value::Str(u) => String::from_utf16_lossy(u),
            Value::Symbol(s) => s.display(),
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
            Value::Object(_) => "[object Object]".to_string(),
            Value::Function { .. } => "[function]".to_string(),
            Value::External(_) => "[external]".to_string(),
            Value::Handler { .. } => "<handler>".to_string(),
            Value::Setter(s) => format!("<setter#{}>", s.frame_index),
            Value::Dispatcher { slot } => format!("<dispatch#{slot}>"),
            Value::Ref { slot } => format!("<ref#{slot}>"),
            Value::Context { id, .. } => format!("<ctx#{id}>"),
            Value::Pending => "<pending>".to_string(),
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

/// Type errors raised by evaluation (e.g. calling a non-function), and the
/// carrier for JS `throw` — any value can be thrown, so the error carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    message: String,
    /// The JS value in flight (`throw x`). `None` for internal type errors,
    /// which a catch binds as their message string (ECMA maps internal errors
    /// to Error instances; ours surface as strings until a full Error class
    /// lands — `throw new Error(msg)` already produces a real Error object).
    thrown: Option<Value>,
}

impl RuntimeError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            thrown: None,
        }
    }

    /// Raise `value` (JS `throw value`). The message mirrors String(value):
    /// thrown strings bind as themselves; Error-shaped objects use their
    /// `message` prop.
    pub fn thrown(value: Value) -> Self {
        let message = match &value {
            Value::Str(u) => String::from_utf16_lossy(u),
            Value::Object(o) => match o.borrow().props.get("message") {
                Some(Value::Str(m)) => String::from_utf16_lossy(m),
                _ => value.display(),
            },
            other => other.display(),
        };
        Self {
            message,
            thrown: Some(value),
        }
    }

    /// The value a `catch` clause binds: the thrown value verbatim, or the
    /// message string for internal (Rust-level) errors.
    pub fn caught_value(&self) -> Value {
        self.thrown
            .clone()
            .unwrap_or_else(|| Value::from_str_utf8(&self.message))
    }

    /// The error message (for error boundaries: `componentDidCatch(err)`).
    pub fn error_text(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime error: {}", self.message)
    }
}

impl std::error::Error for RuntimeError {}
