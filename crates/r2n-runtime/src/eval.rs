//! Evaluator for JS IR expressions (free-function style).
//!
//! Evaluates `r2n_ir::JsExpr` against an environment, the active hook frame,
//! the host, and the component table. Implemented as free functions (rather
//! than capturing everything in one `EvalContext`) so the engine can control
//! borrows precisely while it holds hook frames per component instance.
//!
//! This is the "zero-JS runtime": no `eval`, no JS engine — just a closed
//! evaluator over the ABI value set, with the frame-protocol callbacks for
//! `useState`/`useEffect` (ADR-002/ADR-003).

use crate::hooks::{EffectBody, HookFrame};
use crate::value::{RuntimeError, Symbol, Value};
use r2n_ir::js::{JsBinOp, JsExpr, JsUnOp};
use r2n_ir::runtime::RuntimeComponent;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The evaluation environment: a chain of named binding frames.
/// Frames are SHARED (`Rc<RefCell<...>>`): a closure captures its lexical
/// environment by cloning the `Env` (the frame vector), so later writes to
/// captured names are visible to the closure — JS lexical capture
/// semantics, not a snapshot.
#[derive(Debug, Clone, Default)]
pub struct Env {
    frames: Vec<std::rc::Rc<std::cell::RefCell<BTreeMap<String, Value>>>>,
    /// Shared render-pass context stack: providers push, useContext reads
    /// the nearest value for a context id. Cloned envs share the stack
    /// (Rc<RefCell>) because context is scoped by TREE POSITION, not by
    /// component env; a fresh stack is created per render pass.
    ctx: std::rc::Rc<std::cell::RefCell<Vec<(u64, Value)>>>,
}

impl Env {
    pub fn new() -> Self {
        // Start with one top-level scope so `define` always has somewhere to
        // write. `push_scope`/`pop_scope` add/remove nested scopes.
        Self {
            frames: vec![std::rc::Rc::new(std::cell::RefCell::new(BTreeMap::new()))],
            ctx: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        }
    }

    /// The context stack (shared across env clones of this pass).
    pub fn ctx(&self) -> std::rc::Rc<std::cell::RefCell<Vec<(u64, Value)>>> {
        self.ctx.clone()
    }

    /// A fresh env whose context stack is the SHARED stack of `parent` —
    /// children of a provider must see the provider's values.
    pub fn child_of(parent: &Env) -> Self {
        Self {
            frames: vec![std::rc::Rc::new(std::cell::RefCell::new(BTreeMap::new()))],
            ctx: parent.ctx.clone(),
        }
    }
    pub fn push_scope(&mut self) {
        self.frames
            .push(std::rc::Rc::new(std::cell::RefCell::new(BTreeMap::new())));
    }
    pub fn pop_scope(&mut self) {
        self.frames.pop();
    }
    pub fn define(&mut self, name: &str, value: Value) {
        if let Some(top) = self.frames.last() {
            top.borrow_mut().insert(name.to_string(), value);
        }
    }
    pub fn get(&self, name: &str) -> Result<Value, RuntimeError> {
        for frame in self.frames.iter().rev() {
            if let Some(v) = frame.borrow().get(name) {
                return Ok(v.clone());
            }
        }
        // Strict-mode `this` outside a member call is `undefined`, not an
        // unbound-variable error (ES semantics).
        if name == "this" {
            return Ok(Value::Undefined);
        }
        Err(RuntimeError::new(format!("unbound variable '{name}'")))
    }

    /// The shared frame vector (the lexical environment reference).
    pub fn frames(&self) -> &[std::rc::Rc<std::cell::RefCell<BTreeMap<String, Value>>>] {
        &self.frames
    }
}

/// Callbacks the evaluator needs from the host (logging side effects).
pub trait Host {
    fn log(&mut self, line: &str);
}

/// Evaluate a JS expression in the given environment/frame/host.
#[allow(clippy::too_many_arguments)]
pub fn eval(
    expr: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<Value, RuntimeError> {
    match expr {
        JsExpr::Lit(l) => Ok(lit_to_value(l)),
        JsExpr::Var(name) => env.get(name),
        JsExpr::Get { base, prop } => {
            let v = eval(base, env, frame, host, components, effects)?;
            // A ref's `.current` reads the frame slot (the box's live value).
            if let Value::Ref { slot } = &v {
                if prop == "current" {
                    return frame
                        .read_ref(*slot)
                        .ok_or_else(|| RuntimeError::new("ref slot not found"));
                }
            }
            get_prop(&v, prop)
        }
        JsExpr::Index { base, key } => {
            let v = eval(base, env, frame, host, components, effects)?;
            let k = eval(key, env, frame, host, components, effects)?;
            index_prop(&v, &k)
        }
        JsExpr::New { callee, args } => {
            // `new P(args)`: P must be a non-React class in the component
            // table. Creates an instance whose prototype carries the class
            // methods (as Functions; `this` is bound at method calls).
            let name = match &**callee {
                JsExpr::Var(n) => n.clone(),
                other => {
                    return Err(RuntimeError::new(format!(
                        "new expects a class name, got {other:?}"
                    )))
                }
            };
            let class_info = components
                .iter()
                .find(|c| c.name == name)
                .and_then(|c| c.class.clone())
                .ok_or_else(|| RuntimeError::new(format!("class '{name}' not found")))?;
            // Prototype with methods (Functions; `this` bound per call).
            let mut proto_data = crate::value::ObjData::new();
            for (mname, m) in &class_info.methods {
                if mname == "constructor" {
                    continue;
                }
                proto_data.props.insert(
                    mname.clone(),
                    Value::Function {
                        params: m.params.clone(),
                        body: Box::new(m.body.clone()),
                        captured: Env::new(),
                    },
                );
            }
            let proto = std::rc::Rc::new(std::cell::RefCell::new(proto_data));
            let inst = Value::Object(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::ObjData {
                    props: BTreeMap::new(),
                    proto: Some(proto),
                },
            )));
            // Run the constructor with `this` = instance and params bound.
            if let Some((_, ctor)) = class_info.methods.iter().find(|(n, _)| n == "constructor") {
                let mut cenv = Env::new();
                cenv.define("this", inst.clone());
                for (i, p) in ctor.params.iter().enumerate() {
                    let v = if let Some(a) = args.get(i) {
                        eval(a, env, frame, host, components, effects)?
                    } else {
                        Value::Undefined
                    };
                    cenv.define(p, v);
                }
                let _ = eval(&ctor.body, &mut cenv, frame, host, components, effects)?;
            }
            Ok(inst)
        }
        JsExpr::Assign { target, value } => {
            let v = eval(value, env, frame, host, components, effects)?;
            match &**target {
                // `x = v` updates the current scope binding.
                JsExpr::Var(name) => {
                    env.define(name, v.clone());
                    Ok(v)
                }
                JsExpr::Get { base, prop } => {
                    let b = eval(base, env, frame, host, components, effects)?;
                    match (&b, prop.as_str()) {
                        // `ref.current = v` writes the frame slot (persists
                        // across renders without re-render).
                        (Value::Ref { slot }, "current") => {
                            frame.write_ref(*slot, v.clone());
                            Ok(v)
                        }
                        // Object member writes set an OWN property (ECMA
                        // data-prop creation). `__proto__ = proto` sets the
                        // chain link (null clears it).
                        (Value::Object(o), "__proto__") => {
                            let mut b = o.borrow_mut();
                            b.proto = match &v {
                                Value::Object(p) => Some(p.clone()),
                                Value::Null => None,
                                _ => {
                                    return Err(RuntimeError::new(
                                        "__proto__ must be an object or null",
                                    ))
                                }
                            };
                            Ok(v)
                        }
                        (Value::Object(o), p) => {
                            o.borrow_mut().set_own(p.to_string(), v.clone());
                            Ok(v)
                        }
                        _ => Err(RuntimeError::new(format!("cannot assign to {prop} on {b}"))),
                    }
                }
                other => Err(RuntimeError::new(format!(
                    "cannot assign to unsupported target {other:?}"
                ))),
            }
        }
        JsExpr::Bin { op, left, right } => {
            eval_bin(*op, left, right, env, frame, host, components, effects)
        }
        JsExpr::Un { op, expr } => {
            let v = eval(expr, env, frame, host, components, effects)?;
            eval_un(*op, &v)
        }
        JsExpr::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(eval(it, env, frame, host, components, effects)?);
            }
            Ok(Value::Array(out))
        }
        JsExpr::Block(stmts) => {
            // Evaluate in order; the block's value is the last expression
            // (null when empty).
            let mut last = Value::Null;
            for s in stmts {
                last = eval(s, env, frame, host, components, effects)?;
            }
            Ok(last)
        }
        JsExpr::If { cond, then, else_ } => {
            if eval(cond, env, frame, host, components, effects)?.is_truthy() {
                eval(then, env, frame, host, components, effects)
            } else {
                eval(else_, env, frame, host, components, effects)
            }
        }
        JsExpr::Closure { params, body, .. } => {
            // First-class function value with LEXICAL capture: the closure
            // holds the env it was evaluated in (shared frames — later
            // writes to captured names are visible).
            Ok(Value::Function {
                params: params.clone(),
                body: body.clone(),
                captured: env.clone(),
            })
        }
        JsExpr::Call { callee, args } => {
            if let JsExpr::Get { base, prop } = &**callee {
                // Object.create(proto) / Object.getPrototypeOf(obj).
                if let JsExpr::Var(o) = &**base {
                    if o == "Object" && prop == "create" {
                        let p = if let Some(a) = args.first() {
                            eval(a, env, frame, host, components, effects)?
                        } else {
                            Value::Undefined
                        };
                        return match p {
                            Value::Object(proto) => Ok(Value::Object(std::rc::Rc::new(
                                std::cell::RefCell::new(crate::value::ObjData {
                                    props: BTreeMap::new(),
                                    proto: Some(proto),
                                }),
                            ))),
                            Value::Null => Ok(Value::Object(std::rc::Rc::new(
                                std::cell::RefCell::new(crate::value::ObjData::new()),
                            ))),
                            other => Err(RuntimeError::new(format!(
                                "Object.create: {other} is not an object"
                            ))),
                        };
                    }
                    if o == "Object" && prop == "getPrototypeOf" {
                        let v = if let Some(a) = args.first() {
                            eval(a, env, frame, host, components, effects)?
                        } else {
                            Value::Undefined
                        };
                        return match &v {
                            Value::Object(obj) => match &obj.borrow().proto {
                                Some(p) => Ok(Value::Object(p.clone())),
                                None => Ok(Value::Null),
                            },
                            other => Err(RuntimeError::new(format!(
                                "getPrototypeOf: {other} is not an object"
                            ))),
                        };
                    }
                }
                if prop == "map" && args.len() == 1 {
                    return call_map(base, &args[0], env, frame, host, components, effects);
                }
                if prop == "filter" && args.len() == 1 {
                    return call_filter(base, &args[0], env, frame, host, components, effects);
                }
                if prop == "log" {
                    let parts: Result<Vec<String>, RuntimeError> = args
                        .iter()
                        .map(|a| {
                            eval(a, env, frame, host, components, effects).map(|v| v.display())
                        })
                        .collect();
                    host.log(&parts?.join(" "));
                    return Ok(Value::Null);
                }
            }
            if let JsExpr::Var(name) = &**callee {
                return call_var(name, args, env, frame, host, components, effects);
            }
            // Method call `o.m(...)`: the member-call receiver is bound to
            // the function's `this` (JS semantics) — and the callee value is
            // resolved via get_prop (prototype chain walk) so inherited
            // methods work.
            if let JsExpr::Get { base, prop } = &**callee {
                let b = eval(base, env, frame, host, components, effects)?;
                // `this` is bound for member calls on object receivers;
                // for others the receiver is still the value (primitives
                // have methods too — e.g. array/string length).
                let this_arg = Some(&b);
                let callee_val = get_prop(&b, prop);
                let arg_vals: Result<Vec<Value>, RuntimeError> = args
                    .iter()
                    .map(|a| eval(a, env, frame, host, components, effects))
                    .collect();
                return call_value(
                    &callee_val?,
                    &arg_vals?,
                    env,
                    frame,
                    host,
                    components,
                    effects,
                    this_arg,
                );
            }
            let callee_val = eval(callee, env, frame, host, components, effects)?;
            let arg_vals: Result<Vec<Value>, RuntimeError> = args
                .iter()
                .map(|a| eval(a, env, frame, host, components, effects))
                .collect();
            let arg_vals = arg_vals?;
            call_value(
                &callee_val,
                &arg_vals,
                env,
                frame,
                host,
                components,
                effects,
                None,
            )
        }
        JsExpr::Builtin(_) => Err(RuntimeError::new(
            "builtin references are not supported in the AOT-only subset",
        )),
        // The pre-lowered children of a component call: carried verbatim as
        // a `Value::Children`. The nodes still belong to the parent scope;
        // the component that received them splices them (engine.rs).
        JsExpr::Children(nodes) => Ok(Value::Children(nodes.clone())),
    }
}

/// ECMA-262 ToNumber for the supported value set: undefined -> NaN,
/// null -> 0, bool -> 0|1, strings parse ("" -> 0, invalid -> NaN),
/// numbers pass through, BigInt/Numeric-like convert (BigInt via f64,
/// bounded), everything else NaN.
fn ecma_to_number(v: &Value) -> f64 {
    match v {
        Value::Undefined => f64::NAN,
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => *n,
        Value::BigInt(n) => *n as f64,
        Value::Str(s) => {
            let s = String::from_utf16_lossy(s);
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        _ => f64::NAN,
    }
}

fn lit_to_value(l: &r2n_ir::value::Literal) -> Value {
    use r2n_ir::value::Literal as L;
    match l {
        L::Int(i) => Value::Number(*i as f64),
        L::Float(f) => Value::Number(*f),
        L::String(s) => Value::from_str_utf8(s),
        L::Bool(b) => Value::Bool(*b),
        L::Null => Value::Null,
        L::Undefined => Value::Undefined,
    }
}

fn get_prop(base: &Value, prop: &str) -> Result<Value, RuntimeError> {
    match base {
        Value::Map(m) => Ok(m.get(prop).cloned().unwrap_or(Value::Null)),
        Value::Object(o) if prop == "__proto__" => match &o.borrow().proto {
            Some(p) => Ok(Value::Object(p.clone())),
            None => Ok(Value::Null),
        },
        Value::Object(o) => {
            // Walk the prototype chain (own prop first); missing -> undefined.
            let mut cur: Option<std::rc::Rc<std::cell::RefCell<crate::value::ObjData>>> =
                Some(o.clone());
            while let Some(c) = cur {
                let b = c.borrow();
                if let Some(v) = b.props.get(prop) {
                    return Ok(v.clone());
                }
                cur = b.proto.clone();
            }
            Ok(Value::Undefined)
        }
        Value::Str(u) if prop == "length" => Ok(Value::Number(u.len() as f64)),
        Value::Array(a) if prop == "length" => Ok(Value::Number(a.len() as f64)),
        other => Err(RuntimeError::new(format!("cannot read .{prop} on {other}"))),
    }
}

fn index_prop(base: &Value, key: &Value) -> Result<Value, RuntimeError> {
    match base {
        Value::Array(a) => {
            let i = key
                .as_number()
                .ok_or_else(|| RuntimeError::new("non-number index"))? as usize;
            Ok(a.get(i).cloned().unwrap_or(Value::Null))
        }
        Value::Str(u) => {
            let i = key
                .as_number()
                .ok_or_else(|| RuntimeError::new("non-number index"))? as usize;
            Ok(u.get(i)
                .map(|c| Value::Str(vec![*c]))
                .unwrap_or(Value::Null))
        }
        Value::Map(m) => {
            let k = key.as_str_utf8().unwrap_or_else(|| key.display());
            Ok(m.get(&k).cloned().unwrap_or(Value::Null))
        }
        Value::Object(o) => {
            let k = key.as_str_utf8().unwrap_or_else(|| key.display());
            let mut cur: Option<std::rc::Rc<std::cell::RefCell<crate::value::ObjData>>> =
                Some(o.clone());
            while let Some(c) = cur {
                let b = c.borrow();
                if let Some(v) = b.props.get(&k) {
                    return Ok(v.clone());
                }
                cur = b.proto.clone();
            }
            Ok(Value::Undefined)
        }
        other => Err(RuntimeError::new(format!("cannot index {other}"))),
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_bin(
    op: JsBinOp,
    left: &JsExpr,
    right: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<Value, RuntimeError> {
    use JsBinOp::*;
    match op {
        And => {
            let l = eval(left, env, frame, host, components, effects)?;
            if !l.is_truthy() {
                return Ok(l);
            }
            return eval(right, env, frame, host, components, effects);
        }
        Or => {
            let l = eval(left, env, frame, host, components, effects)?;
            if l.is_truthy() {
                return Ok(l);
            }
            return eval(right, env, frame, host, components, effects);
        }
        _ => {}
    }
    let l = eval(left, env, frame, host, components, effects)?;
    let r = eval(right, env, frame, host, components, effects)?;
    let res = match op {
        Add => {
            if matches!(l, Value::Str(_)) || matches!(r, Value::Str(_)) {
                return Ok(Value::from_str_utf8(&format!(
                    "{}{}",
                    l.display(),
                    r.display()
                )));
            }
            let ln = ecma_to_number(&l);
            let rn = ecma_to_number(&r);
            Value::Number(ln + rn)
        }
        Sub => num_op(&l, &r, |a, b| a - b)?,
        Mul => num_op(&l, &r, |a, b| a * b)?,
        Div => num_op(&l, &r, |a, b| a / b)?,
        Mod => num_op(&l, &r, |a, b| a % b)?,
        Eq => {
            return Ok(Value::Bool(loosely_equal(
                &l, &r, env, frame, host, components, effects,
            )?))
        }
        Neq => {
            return Ok(Value::Bool(!loosely_equal(
                &l, &r, env, frame, host, components, effects,
            )?))
        }
        StrictEq => return Ok(Value::Bool(strictly_equal(&l, &r))),
        StrictNeq => return Ok(Value::Bool(!strictly_equal(&l, &r))),
        Lt => return Ok(Value::Bool(ord(&l, &r)? < std::cmp::Ordering::Equal)),
        Gt => return Ok(Value::Bool(ord(&l, &r)? > std::cmp::Ordering::Equal)),
        Le => return Ok(Value::Bool(ord(&l, &r)? != std::cmp::Ordering::Greater)),
        Ge => return Ok(Value::Bool(ord(&l, &r)? != std::cmp::Ordering::Less)),
        And | Or => unreachable!(),
    };
    Ok(res)
}

/// Compute an arithmetic result, requiring both operands to be numbers.
fn num_op(l: &Value, r: &Value, f: impl Fn(f64, f64) -> f64) -> Result<Value, RuntimeError> {
    let ln = l
        .as_number()
        .ok_or_else(|| RuntimeError::new("non-number operand"))?;
    let rn = r
        .as_number()
        .ok_or_else(|| RuntimeError::new("non-number operand"))?;
    Ok(Value::Number(f(ln, rn)))
}

/// Total order over values for comparison operators (numbers, then by kind,
/// then lexicographically). Allows `<`/`>` between any two values without a
/// type error (ECMAScript would coerce; we compare structurally, which is
/// sufficient for the supported subset where comparisons are number/number or
/// string/string).
fn ord(l: &Value, r: &Value) -> Result<std::cmp::Ordering, RuntimeError> {
    use std::cmp::Ordering::*;
    match (l, r) {
        (Value::Number(a), Value::Number(b)) => Ok(a.partial_cmp(b).unwrap_or(Equal)),
        (Value::Str(a), Value::Str(b)) => {
            Ok(String::from_utf16_lossy(a).cmp(&String::from_utf16_lossy(b)))
        }
        (Value::Null, Value::Null) => Ok(Equal),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        _ => Err(RuntimeError::new("incomparable operands")),
    }
}

/// ECMA-262 section 7.2.15 IsStrictlyEqual (`===`). No coercion: operands of
/// different types are never equal, `NaN` is not equal to itself, and
/// reference types (objects, functions) compare by identity. `Array`/`Map`
/// are copied on read in this runtime (plain `Vec`/`BTreeMap`), so they
/// compare structurally — the one deliberate divergence from ECMA identity,
/// documented in M2-T05.
fn strictly_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        (Value::Symbol(x), Value::Symbol(y)) => x.id == y.id,
        (Value::Object(x), Value::Object(y)) => std::rc::Rc::ptr_eq(x, y),
        (Value::Function { captured: x, .. }, Value::Function { captured: y, .. }) => {
            std::ptr::eq(x, y)
        }
        (Value::Array(x), Value::Array(y)) => x == y,
        (Value::Map(x), Value::Map(y)) => x == y,
        _ => a == b,
    }
}

/// ECMA-262 section 7.2.14 IsLooselyEqual (`==`). Coercion ladder: same-type
/// comparison; `null == undefined`; booleans coerce to numbers first;
/// number/string coerce toward each other; BigInt compares against string and
/// number mathematically; objects go through OrdinaryToPrimitive (valueOf,
/// then toString). A methodless object raises TypeError exactly as ECMA does
/// for `Object.create(null) == x` (our model has no built-in
/// Object.prototype methods yet, so every plain object is null-prototype).
#[allow(clippy::too_many_arguments)]
fn loosely_equal(
    a: &Value,
    b: &Value,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<bool, RuntimeError> {
    match (a, b) {
        // Same-type operands compare strictly (steps 1-2).
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => Ok(true),
        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => Ok(true),
        (Value::Number(x), Value::Number(y)) => Ok(x == y),
        (Value::Str(x), Value::Str(y)) => Ok(x == y),
        (Value::Bool(x), Value::Bool(y)) => Ok(x == y),
        (Value::BigInt(x), Value::BigInt(y)) => Ok(x == y),
        (Value::Symbol(x), Value::Symbol(y)) => Ok(x.id == y.id),
        (Value::Object(_), Value::Object(_)) => Ok(strictly_equal(a, b)),
        // Number <-> String: coerce the string side (steps 3-4).
        (Value::Number(x), Value::Str(_)) => Ok(*x == ecma_to_number(b)),
        (Value::Str(_), Value::Number(y)) => Ok(ecma_to_number(a) == *y),
        // BigInt <-> String: StringToBigInt; unparseable strings are not equal
        // (steps 5-6).
        (Value::BigInt(x), Value::Str(_)) | (Value::Str(_), Value::BigInt(x)) => {
            let s = if matches!(a, Value::Str(_)) { a } else { b };
            Ok(string_to_bigint(s).map(|n| n == *x).unwrap_or(false))
        }
        // Booleans coerce to numbers FIRST, then re-compare (steps 7-8) — so
        // `true == 1` but `true == "2"` is false ("2" -> 2 != 1).
        (Value::Bool(_), _) => loosely_equal(
            &Value::Number(ecma_to_number(a)),
            b,
            env,
            frame,
            host,
            components,
            effects,
        ),
        (_, Value::Bool(_)) => loosely_equal(
            a,
            &Value::Number(ecma_to_number(b)),
            env,
            frame,
            host,
            components,
            effects,
        ),
        // BigInt <-> Number: mathematical equality; NaN/Inf/integral check
        // (steps 9-10).
        (Value::BigInt(x), Value::Number(y)) => Ok(number_bigint_equal(*y, *x)),
        (Value::Number(x), Value::BigInt(y)) => Ok(number_bigint_equal(*x, *y)),
        // Object vs primitive: OrdinaryToPrimitive (valueOf, then toString),
        // then re-compare (steps 12-13). A methodless object (null
        // prototype — our plain objects have no Object.prototype yet) raises
        // TypeError, exactly as `Object.create(null) == 1` does in ECMA.
        (Value::Object(_), _) => {
            let pa = to_primitive(a, env, frame, host, components, effects)?;
            loosely_equal(&pa, b, env, frame, host, components, effects)
        }
        (_, Value::Object(_)) => {
            let pb = to_primitive(b, env, frame, host, components, effects)?;
            loosely_equal(a, &pb, env, frame, host, components, effects)
        }
        // Remaining pairs (Symbol vs string/number, function vs primitive,
        // runtime-internal variants): no ECMA coercion step applies, so fall
        // back to same-variant comparison — different types are not equal.
        _ => Ok(a == b),
    }
}

/// OrdinaryToPrimitive for the default hint: call the object's `valueOf`,
/// then `toString`; the first non-object result wins. Objects with neither
/// callable method are a TypeError.
#[allow(clippy::too_many_arguments)]
fn to_primitive(
    v: &Value,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<Value, RuntimeError> {
    // Arrays convert like Array.prototype.toString (join with ","):
    // null/undefined elements become empty strings.
    if let Value::Array(items) = v {
        let parts: Vec<String> = items
            .iter()
            .map(|e| match e {
                Value::Null | Value::Undefined => String::new(),
                other => other.display(),
            })
            .collect();
        return Ok(Value::from_str_utf8(&parts.join(",")));
    }
    if let Value::Object(_) = v {
        for name in ["valueOf", "toString"] {
            if let Value::Function { .. } = get_prop(v, name)? {
                let f = get_prop(v, name)?;
                let r = call_value(&f, &[], env, frame, host, components, effects, Some(v))?;
                if !matches!(r, Value::Object(_)) {
                    return Ok(r);
                }
            }
        }
        return Err(RuntimeError::new(
            "Cannot convert object to primitive value (no callable valueOf/toString)",
        ));
    }
    Ok(v.clone())
}

/// StringToBigInt: trim, parse as i64; any failure yields None (the caller
/// treats it as "not equal", per ECMA).
fn string_to_bigint(v: &Value) -> Option<i64> {
    let s = v.as_str_utf8()?;
    s.trim().parse::<i64>().ok()
}

/// BigInt/number mathematical equality: the number must be finite, integral,
/// and equal to the BigInt (i64 bounds the BigInt, so numbers beyond that
/// range simply never compare equal to it).
fn number_bigint_equal(n: f64, b: i64) -> bool {
    n.is_finite() && n.fract() == 0.0 && n == b as f64
}

fn eval_un(op: JsUnOp, v: &Value) -> Result<Value, RuntimeError> {
    match op {
        JsUnOp::Neg => {
            let n = v
                .as_number()
                .ok_or_else(|| RuntimeError::new("non-number operand"))?;
            Ok(Value::Number(-n))
        }
        JsUnOp::Not => Ok(Value::Bool(!v.is_truthy())),
    }
}

/// Invoke a value as a function (handles `Setter` and `Dispatcher` calls;
/// handlers are invoked only via event dispatch). The extra evaluator
/// context is needed for reducers: `Dispatcher(slot)` evaluates the stored
/// reducer body against a fresh env of its params.
#[allow(clippy::too_many_arguments)]
fn call_value(
    callee: &Value,
    args: &[Value],
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
    this_arg: Option<&Value>,
) -> Result<Value, RuntimeError> {
    match callee {
        Value::Setter(s) => {
            let _ = this_arg;
            let next = args
                .first()
                .cloned()
                .ok_or_else(|| RuntimeError::new("setter expects one argument"))?;
            frame.apply_setter(s, next);
            Ok(Value::Null)
        }
        Value::Dispatcher { slot } => {
            let _ = this_arg;
            let action = args
                .first()
                .cloned()
                .ok_or_else(|| RuntimeError::new("dispatch expects one argument"))?;
            let (params, body, state) = frame
                .reducer_state(*slot)
                .ok_or_else(|| RuntimeError::new("dispatch: reducer slot not found"))?;
            let mut denv = Env::new();
            if let Some(p) = params.first() {
                denv.define(p, state);
            }
            if let Some(p) = params.get(1) {
                denv.define(p, action);
            }
            let new_state = eval(&body, &mut denv, frame, host, components, effects)?;
            frame.write_state(*slot, new_state);
            Ok(Value::Null)
        }
        Value::Handler { body, .. } => {
            // Handler values are plain closures; invoking one evaluates its
            // body in the CURRENT env/frame (the caller's render scope —
            // e.g. `this.method()` inside a class render/handler).
            // Event dispatch still exists for the ABI's on* path.
            eval(body, env, frame, host, components, effects)
        }
        Value::Function {
            params,
            body,
            captured,
        } => {
            // Call against the CAPTURED lexical env (params in a child
            // scope; shared-frame writes in the captured scope are seen).
            let mut fenv = captured.clone();
            fenv.push_scope();
            // Method calls bind the receiver as `this`.
            if let Some(t) = this_arg {
                fenv.define("this", t.clone());
            }
            for (i, p) in params.iter().enumerate() {
                fenv.define(p, args.get(i).cloned().unwrap_or(Value::Undefined));
            }
            eval(body, &mut fenv, frame, host, components, effects)
        }
        other => Err(RuntimeError::new(format!(
            "cannot call {other} as function"
        ))),
    }
}

/// Resolve a `Var` call: `useState`, `useEffect`, `console.log`, or a user fn.
#[allow(clippy::too_many_arguments)]
fn call_var(
    name: &str,
    args: &[JsExpr],
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<Value, RuntimeError> {
    match name {
        "useState" => {
            let initial = if let Some(a) = args.first() {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Null
            };
            let (value, setter) = frame.use_state(initial);
            Ok(Value::Array(vec![value, Value::Setter(setter)]))
        }
        "useEffect" | "useLayoutEffect" => {
            let layout = name == "useLayoutEffect";
            let deps = if args.len() >= 2 {
                let d = eval(&args[1], env, frame, host, components, effects)?;
                Some(deps_from_value(&d))
            } else {
                None
            };
            // React cleanup form: the effect arrow's VALUE is a cleanup
            // arrow (`() => { s(); return () => c(); }` — or the shorthand
            // `() => () => c()`). The cleanup body + current env are stored
            // with the slot; when the deps change or the component unmounts,
            // the cleanup runs BEFORE the next setup (React ordering).
            let cleanup = match args.first() {
                Some(JsExpr::Closure { body, .. }) => {
                    cleanup_of(body, env, layout, frame.path().map(|p| p.to_vec()))
                }
                _ => None,
            };
            let (should_run, old_cleanup) = frame.use_effect(deps, cleanup);
            if should_run {
                // The previous cleanup (deps changed) runs first, then the
                // new setup — both in hook order. Layout effects drain
                // synchronously (pre-commit); regular effects after the diff.
                if let Some(old) = old_cleanup {
                    effects.push(old);
                }
                if let JsExpr::Closure { body, .. } = &args[0] {
                    effects.push(EffectBody {
                        body: (**body).clone(),
                        env: env.clone(),
                        layout,
                        frame_path: frame.path().map(|p| p.to_vec()),
                    });
                }
            }
            Ok(Value::Null)
        }
        "useReducer" => {
            // The reducer is the FIRST arg's closure (params + body); it is
            // stored as IR data (never a function pointer) and evaluated at
            // dispatch time: `reducer(state, action)`.
            let (rparams, rbody) = match args.first() {
                Some(JsExpr::Closure { params, body, .. }) => (params.clone(), (**body).clone()),
                _ => return Err(RuntimeError::new("useReducer expects a reducer arrow")),
            };
            let initial = if let Some(a) = args.get(1) {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Null
            };
            let (state, dispatcher) = frame.use_reducer(rparams, rbody, initial);
            Ok(Value::Array(vec![state, dispatcher]))
        }
        "createContext" => {
            // A context handle: unique id + the default value (React:
            // createContext(defaultValue)).
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let default = match args.first() {
                Some(a) => eval(a, env, frame, host, components, effects)?,
                None => Value::Null,
            };
            Ok(Value::Context {
                id,
                default: Box::new(default),
            })
        }
        "useContext" => {
            let ctx_val = match args.first() {
                Some(a) => eval(a, env, frame, host, components, effects)?,
                None => return Err(RuntimeError::new("useContext expects a context")),
            };
            let Value::Context { id, default } = ctx_val else {
                return Err(RuntimeError::new(
                    "useContext expects a createContext value",
                ));
            };
            // Nearest provider value on the render-pass stack, else the
            // handle's default.
            let stack = env.ctx();
            let found = stack
                .borrow()
                .iter()
                .rev()
                .find(|(cid, _)| *cid == id)
                .map(|(_, v)| v.clone());
            Ok(found.unwrap_or(*default))
        }
        "useResource" => {
            // (value, resolve): reads Value::Pending until resolve() is
            // called — a real, state-driven source for Suspense.
            let key = if let Some(a) = args.first() {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Null
            };
            let (p, r) = frame.use_pending(key);
            Ok(Value::Array(vec![p, r]))
        }
        "useId" => {
            if frame.path().is_none() {
                return Err(RuntimeError::new("useId outside a component"));
            }
            Ok(frame.use_id())
        }
        "BigInt" => {
            let v = if let Some(a) = args.first() {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Undefined
            };
            match v {
                Value::Number(n) if n.fract() == 0.0 && n.abs() < 9.2e18 => {
                    Ok(Value::BigInt(n as i64))
                }
                Value::BigInt(n) => Ok(Value::BigInt(n)),
                Value::Bool(b) => Ok(Value::BigInt(b as i64)),
                Value::Undefined | Value::Null => {
                    Err(RuntimeError::new("cannot convert to BigInt"))
                }
                other => Err(RuntimeError::new(format!(
                    "cannot convert {other} to BigInt"
                ))),
            }
        }
        "Symbol" => {
            // Symbol() -> fresh anonymous; Symbol.for? -> registered by key.
            let key = if args.is_empty() {
                None
            } else {
                let v = eval(&args[0], env, frame, host, components, effects)?;
                Some(v.display())
            };
            Ok(Value::Symbol(new_symbol(key)))
        }
        "Object" => {
            // Object() constructor: an empty object (proto None).
            Ok(Value::Object(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::ObjData::new(),
            ))))
        }
        "typeof" => {
            let v = if let Some(a) = args.first() {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Undefined
            };
            Ok(Value::from_str_utf8(match &v {
                Value::Undefined => "undefined",
                Value::Null => "object",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::BigInt(_) => "bigint",
                Value::Str(_) => "string",
                Value::Symbol(_) => "symbol",
                Value::Object(_) | Value::Map(_) | Value::Array(_) | Value::Children(_) => "object",
                Value::Function { .. } | Value::Handler { .. } => "function",
                Value::External(_) => "external",
                Value::Setter(_) | Value::Dispatcher { .. } | Value::Ref { .. } => "function",
                Value::Context { .. } => "object",
                Value::Pending => "object",
            }))
        }
        "useRef" => {
            let initial = if let Some(a) = args.first() {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Null
            };
            Ok(frame.use_ref(initial))
        }
        "useMemo" => {
            let deps = if args.len() >= 2 {
                let d = eval(&args[1], env, frame, host, components, effects)?;
                Some(deps_from_value(&d))
            } else {
                None
            };
            match frame.use_memo(deps) {
                Some(cached) => Ok(cached),
                None => {
                    // Compute lazily: run the factory (an arrow's body) in
                    // the CURRENT render env (it closes over the scope).
                    let value = match &args[0] {
                        JsExpr::Closure { body, .. } => {
                            eval(body, env, frame, host, components, effects)?
                        }
                        _ => return Err(RuntimeError::new("useMemo expects a factory arrow")),
                    };
                    frame.record_memo(value.clone());
                    Ok(value)
                }
            }
        }
        "useCallback" => {
            let deps = if args.len() >= 2 {
                let d = eval(&args[1], env, frame, host, components, effects)?;
                Some(deps_from_value(&d))
            } else {
                None
            };
            // The cached identity is the ABI function value: a Handler
            // (owning instance path + closure body). Its body executes via
            // the event-dispatch path when used as an on* prop.
            let inst_path = frame
                .path()
                .ok_or_else(|| RuntimeError::new("useCallback outside a component"))?
                .to_vec();
            let body = match &args[0] {
                JsExpr::Closure { body, .. } => Box::new((**body).clone()),
                _ => return Err(RuntimeError::new("useCallback expects an arrow")),
            };
            let ident = frame.next_callback_ident();
            let value = Value::Handler {
                inst_path,
                body,
                ident,
            };
            Ok(frame.use_callback(deps, value))
        }
        "console" => Ok(Value::Null),
        "log" => {
            let parts: Result<Vec<String>, RuntimeError> = args
                .iter()
                .map(|a| eval(a, env, frame, host, components, effects).map(|v| v.display()))
                .collect();
            host.log(&parts?.join(" "));
            Ok(Value::Null)
        }
        _ => {
            let callee_val = env.get(name)?;
            let arg_vals: Result<Vec<Value>, RuntimeError> = args
                .iter()
                .map(|a| eval(a, env, frame, host, components, effects))
                .collect();
            call_value(
                &callee_val,
                &arg_vals?,
                env,
                frame,
                host,
                components,
                effects,
                None,
            )
        }
    }
}

/// `base.map(fn)`: apply the arrow `fn` to each element of an array, binding its
/// first param to the element and second (optional) to the index.
#[allow(clippy::too_many_arguments)]
fn call_map(
    base: &JsExpr,
    fn_expr: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<Value, RuntimeError> {
    let arr = eval(base, env, frame, host, components, effects)?;
    let arr = match arr {
        Value::Array(a) => a,
        other => return Err(RuntimeError::new(format!("cannot map over {other}"))),
    };
    let (params, body) = match fn_expr {
        JsExpr::Closure { params, body, .. } => (params, body),
        _ => return Err(RuntimeError::new("map expects an arrow function")),
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, elem) in arr.into_iter().enumerate() {
        env.push_scope();
        if let Some(p) = params.first() {
            env.define(p, elem);
        }
        if let Some(p) = params.get(1) {
            env.define(p, Value::Number(i as f64));
        }
        let r = eval(body, env, frame, host, components, effects);
        env.pop_scope();
        out.push(r?);
    }
    Ok(Value::Array(out))
}

/// `arr.filter(predicate)` — keeps elements whose predicate result is truthy.
/// Same per-item evaluation protocol as `call_map` (param 0 = element,
/// param 1 = index).
fn call_filter(
    base: &JsExpr,
    fn_expr: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectBody>,
) -> Result<Value, RuntimeError> {
    let arr = eval(base, env, frame, host, components, effects)?;
    let arr = match arr {
        Value::Array(a) => a,
        other => return Err(RuntimeError::new(format!("cannot filter over {other}"))),
    };
    let (params, body) = match fn_expr {
        JsExpr::Closure { params, body, .. } => (params, body),
        _ => return Err(RuntimeError::new("filter expects an arrow function")),
    };
    let mut out = Vec::new();
    for (i, elem) in arr.into_iter().enumerate() {
        let keep = {
            env.push_scope();
            if let Some(p) = params.first() {
                env.define(p, elem.clone());
            }
            if let Some(p) = params.get(1) {
                env.define(p, Value::Number(i as f64));
            }
            let r = eval(body, env, frame, host, components, effects);
            env.pop_scope();
            r?.is_truthy()
        };
        if keep {
            out.push(elem);
        }
    }
    Ok(Value::Array(out))
}

/// The cleanup closure of an effect arrow: an arrow in the effect body's
/// VALUE position (the last statement of a block body, per the `return`
/// spelling, or the whole body for `() => () => cleanup()`). Returns the
/// cleanup's own body + the env captured at effect registration.
/// `layout` is the effect's own phase: the cleanup belongs to the SAME
/// hook slot, so a useLayoutEffect cleanup is a layout cleanup (it must
/// run in the same queue as its setup, immediately before it).
pub fn cleanup_of(
    body: &JsExpr,
    env: &Env,
    layout: bool,
    frame_path: Option<Vec<String>>,
) -> Option<EffectBody> {
    let cleanup_body = match body {
        // `() => () => cleanup()` — the effect body IS the cleanup arrow.
        JsExpr::Closure { body, .. } => Some((**body).clone()),
        // `{ setup(); return <arrow>; }` — the block's VALUE (last expr)
        // is the cleanup arrow; earlier statements are setup side effects.
        JsExpr::Block(stmts) => match stmts.last() {
            Some(JsExpr::Closure { body, .. }) => Some((**body).clone()),
            _ => None,
        },
        _ => None,
    }?;
    Some(EffectBody {
        body: cleanup_body,
        env: env.clone(),
        layout,
        frame_path,
    })
}

/// Symbol identity source: fresh ids; `Symbol.for(key)` symbols share id
/// by key (registered symbols).
fn new_symbol(key: Option<String>) -> Symbol {
    use std::sync::atomic::{AtomicU64, Ordering as O};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, O::Relaxed);
    Symbol { id, key }
}

fn deps_from_value(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    }
}

/// Run a captured effect body (used after commit) against the frame that
/// owns any hook handles the body references.
pub fn run_effect_body(
    body: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
) -> Result<(), RuntimeError> {
    let mut effects = Vec::new();
    eval(body, env, frame, host, components, &mut effects)?;
    Ok(())
}
