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

use crate::hooks::{EffectBody, EffectJob, HookFrame};
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
        // CHAINED (M2-T08): the child shares the parent's frames (Rc) and
        // gets a fresh top scope of its own. Reads walk into the parent
        // (this is how the GLOBAL env — top-level `function*` declarations —
        // reaches every component); defines land in the child's own top.
        let mut frames = parent.frames.clone();
        frames.push(std::rc::Rc::new(std::cell::RefCell::new(BTreeMap::new())));
        Self {
            frames,
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

    /// JS ASSIGNMENT semantics: update the NEAREST existing binding of
    /// `name`, walking outer scopes; only define into the current scope when
    /// no frame binds it (sloppy-mode implicit binding). `define` stays for
    /// declarations (let/const lowering, catch params, component bindings) —
    /// an assignment inside a nested scope (`catch`, arrow block) must reach
    /// the outer binding, not shadow it.
    pub fn assign(&mut self, name: &str, value: Value) {
        for frame in self.frames.iter().rev() {
            if frame.borrow().contains_key(name) {
                frame.borrow_mut().insert(name.to_string(), value);
                return;
            }
        }
        self.define(name, value);
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

/// Evaluate a function-like body to its completion value: a `return v`
/// raised anywhere inside (plain functions, closures, map/filter/every
/// callbacks, reducers, handlers, memo factories, effect bodies) completes
/// the CURRENT unit with v (JS `return` semantics). Any other error
/// propagates unchanged.
#[allow(clippy::too_many_arguments)]
pub fn eval_function_body(
    body: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectJob>,
) -> Result<Value, RuntimeError> {
    match eval(body, env, frame, host, components, effects) {
        Ok(v) => Ok(v),
        Err(e) => match e.return_value() {
            Some(v) => Ok(v),
            None => Err(e),
        },
    }
}

/// Evaluate a JS expression in the given environment/frame/host.
#[allow(clippy::too_many_arguments)]
pub fn eval(
    expr: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectJob>,
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
            if name == "Promise" {
                // `new Promise(executor)` (M2-T07): the executor runs
                // synchronously with (resolve, reject) settlers; a throw in
                // it rejects the promise (ECMA).
                let Some(JsExpr::Closure { params, body, .. }) = args.first() else {
                    return Err(RuntimeError::new("new Promise requires an executor arrow"));
                };
                let handle = crate::value::PromiseData::new();
                let fval = Value::Function {
                    params: params.clone(),
                    body: body.clone(),
                    captured: env.clone(),
                    ident: std::rc::Rc::new(()),
                };
                let res = Value::Settler {
                    promise: handle.clone(),
                    fulfill: true,
                };
                let rej = Value::Settler {
                    promise: handle.clone(),
                    fulfill: false,
                };
                if let Err(e) = call_value(
                    &fval,
                    &[res, rej],
                    env,
                    frame,
                    host,
                    components,
                    effects,
                    None,
                ) {
                    let reason = e.caught_value();
                    settle_promise(&handle, reason, false, effects);
                }
                return Ok(Value::Promise(handle));
            }
            if name == "Error" {
                // `new Error(msg)` — same object as a plain Error(msg) call.
                let msg = if let Some(JsExpr::Lit(r2n_ir::value::Literal::String(s))) = args.first()
                {
                    Value::from_str_utf8(s)
                } else if let Some(a) = args.first() {
                    eval(a, env, frame, host, components, effects)?
                } else {
                    Value::Undefined
                };
                return Ok(make_error_value(msg));
            }
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
                        ident: std::rc::Rc::new(()),
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
                // `x = v` updates the nearest existing binding (JS
                // assignment — outer scopes are reached, not shadowed).
                JsExpr::Var(name) => {
                    env.assign(name, v.clone());
                    Ok(v)
                }
                JsExpr::Get { base, prop } => {
                    let b = eval(base, env, frame, host, components, effects)?;
                    write_prop(&b, prop, v.clone(), frame)?;
                    Ok(v)
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
                match it {
                    r2n_ir::js::JsArrayItem::Expr(e) => {
                        out.push(eval(e, env, frame, host, components, effects)?);
                    }
                    r2n_ir::js::JsArrayItem::Spread(e) => {
                        let v = eval(e, env, frame, host, components, effects)?;
                        match v {
                            Value::Array(items) => out.extend(items),
                            other => {
                                return Err(RuntimeError::new(format!(
                                    "spread of non-array in array literal: {other}"
                                )))
                            }
                        }
                    }
                }
            }
            Ok(Value::Array(out))
        }
        JsExpr::Object(items) => {
            use std::collections::BTreeMap;
            let mut props = BTreeMap::new();
            for it in items {
                match it {
                    r2n_ir::js::JsObjectItem::Shorthand(name) => {
                        let v = env.get(name)?;
                        props.insert(name.clone(), v);
                    }
                    r2n_ir::js::JsObjectItem::Prop(k, v) => {
                        let val = eval(v, env, frame, host, components, effects)?;
                        props.insert(k.clone(), val);
                    }
                    r2n_ir::js::JsObjectItem::Spread(e) => {
                        let v = eval(e, env, frame, host, components, effects)?;
                        match v {
                            Value::Object(o) => {
                                for (k, val) in o.borrow().props.iter() {
                                    props.insert(k.clone(), val.clone());
                                }
                            }
                            Value::Map(m) => {
                                for (k, val) in m.iter() {
                                    props.insert(k.clone(), val.clone());
                                }
                            }
                            other => {
                                return Err(RuntimeError::new(format!(
                                    "spread of non-object in object literal: {other}"
                                )))
                            }
                        }
                    }
                }
            }
            Ok(Value::Object(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::ObjData { props, proto: None },
            ))))
        }
        JsExpr::While { cond, body, step } => {
            loop {
                let c = eval(cond, env, frame, host, components, effects)?;
                if !c.is_truthy() {
                    break;
                }
                // `step` (a `for` update) runs after every iteration
                // INCLUDING `continue`, but NOT after `break` (ECMA).
                let run_step = |env: &mut Env,
                                frame: &mut HookFrame,
                                host: &mut dyn Host,
                                effects: &mut Vec<EffectJob>|
                 -> Result<(), RuntimeError> {
                    if let Some(s) = step {
                        eval(s, env, frame, host, components, effects)?;
                    }
                    Ok(())
                };
                match eval(body, env, frame, host, components, effects) {
                    Ok(_) => run_step(env, frame, host, effects)?,
                    Err(e) if e.is_break() => break,
                    Err(e) if e.is_continue() => {
                        run_step(env, frame, host, effects)?;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(Value::Null)
        }
        JsExpr::Switch {
            disc,
            cases,
            default,
        } => {
            // ECMA fall-through: find the first case whose test strictly
            // equals the discriminant (or the default when none matches),
            // then run that case and every case after it until `break`.
            let d = eval(disc, env, frame, host, components, effects)?;
            let mut start = cases.len();
            for (i, c) in cases.iter().enumerate() {
                let t = eval(&c.test, env, frame, host, components, effects)?;
                if strictly_equal(&d, &t) {
                    start = i;
                    break;
                }
            }
            if start == cases.len() {
                if let Some(def) = default {
                    let body = JsExpr::Block(def.clone());
                    match eval(&body, env, frame, host, components, effects) {
                        Ok(_) => {}
                        Err(e) if e.is_break() => {}
                        Err(e) => return Err(e),
                    }
                }
                return Ok(Value::Null);
            }
            for c in &cases[start..] {
                let body = JsExpr::Block(c.body.clone());
                match eval(&body, env, frame, host, components, effects) {
                    Ok(_) => {}
                    Err(e) if e.is_break() => break,
                    Err(e) => return Err(e),
                }
            }
            Ok(Value::Null)
        }
        JsExpr::Break => Err(RuntimeError::break_()),
        JsExpr::Continue => Err(RuntimeError::continue_()),
        JsExpr::Return(v) => {
            let value = match v {
                Some(e) => eval(e, env, frame, host, components, effects)?,
                None => Value::Undefined,
            };
            Err(RuntimeError::return_(value))
        }
        JsExpr::Update {
            inc,
            target,
            prefix,
        } => {
            // `x++` / `++x`: read, ToNumber-coerce, write back, yield old/new.
            let old = eval(target, env, frame, host, components, effects)?;
            let n = ecma_to_number(&old);
            let new = Value::Number(if *inc { n + 1.0 } else { n - 1.0 });
            match &**target {
                JsExpr::Var(name) => {
                    env.assign(name, new.clone());
                }
                JsExpr::Get { base, prop } => {
                    let b = eval(base, env, frame, host, components, effects)?;
                    write_prop(&b, prop, new.clone(), frame)?;
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "update target must be a variable or member, got {other:?}"
                    )))
                }
            }
            Ok(if *prefix { new } else { old })
        }
        JsExpr::SpreadArg(e) => {
            // Spread outside a call arg list is a precise error (the lowerer
            // only emits SpreadArg inside Call args).
            let _ = eval(e, env, frame, host, components, effects)?;
            Err(RuntimeError::new("spread element outside a call"))
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
        JsExpr::AsyncFn { params, segments } => Ok(Value::AsyncFn(std::rc::Rc::new(
            crate::value::AsyncFnData {
                params: params.clone(),
                segments: segments.clone(),
                captured: env.clone(),
            },
        ))),
        JsExpr::Throw { value } => {
            let v = eval(value, env, frame, host, components, effects)?;
            Err(RuntimeError::thrown(v))
        }
        JsExpr::Try {
            block,
            catch_param,
            catch,
            finally,
        } => {
            let try_block = JsExpr::Block(block.clone());
            let mut outcome = eval(&try_block, env, frame, host, components, effects);
            // Control flow (`break`/`continue`/`return`) is NOT catchable —
            // it propagates straight through to its driver (ECMA abrupt
            // completions bypass `catch`; only `finally` still runs).
            if let Err(e) = &outcome {
                if e.is_break() || e.is_continue() || e.return_value().is_some() {
                    if let Some(fb) = finally {
                        let finally_block = JsExpr::Block(fb.clone());
                        eval(&finally_block, env, frame, host, components, effects)?;
                    }
                    return outcome;
                }
            }
            if outcome.is_err() {
                if let Some(cb) = catch {
                    let caught = outcome.err().unwrap().caught_value();
                    // The catch param binds in a fresh scope (ECMA catch
                    // block scoping); outer reads/writes still work.
                    env.push_scope();
                    if let Some(p) = catch_param {
                        env.define(p, caught);
                    }
                    let catch_block = JsExpr::Block(cb.clone());
                    outcome = eval(&catch_block, env, frame, host, components, effects);
                    env.pop_scope();
                }
            }
            // Finally runs on EVERY path; an error raised in it replaces the
            // pending outcome (ECMA completion semantics — the expression IR
            // has no return-completion, so no return-override case exists).
            if let Some(fb) = finally {
                let finally_block = JsExpr::Block(fb.clone());
                match eval(&finally_block, env, frame, host, components, effects) {
                    Ok(_) => outcome,
                    Err(fe) => Err(fe),
                }
            } else {
                outcome
            }
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
                // One token per closure VALUE: `f === f` must hold (the same
                // binding read twice yields the same ident Rc), while two
                // evaluations of the same closure expression are distinct.
                ident: std::rc::Rc::new(()),
            })
        }
        JsExpr::Call { callee, args } => {
            if let JsExpr::Get { base, prop } = &**callee {
                // Promise.prototype.then: queue continuations on the
                // referenced promise (M2-T07). The callbacks run as
                // scheduler jobs when the promise settles (or immediately
                // queue if already settled — ECMA: then is always async).
                if prop == "then" {
                    let b = eval(base, env, frame, host, components, effects)?;
                    let Value::Promise(p) = &b else {
                        return Err(RuntimeError::new(format!(".then on non-promise {b}")));
                    };
                    let (on_ok, on_err) = then_callbacks(args)?;
                    let result = crate::value::PromiseData::new();
                    let job_env = env.clone();
                    let fp = frame.path().map(|p| p.to_vec());
                    let already = {
                        let mut pd = p.borrow_mut();
                        match &pd.state {
                            crate::value::PromiseState::Pending => {
                                pd.handlers.push(crate::value::Continuation::Then {
                                    on_ok: on_ok.clone(),
                                    on_err: on_err.clone(),
                                    env: job_env.clone(),
                                    result: result.clone(),
                                    frame_path: fp.clone(),
                                });
                                None
                            }
                            crate::value::PromiseState::Fulfilled(v) => Some((v.clone(), false)),
                            crate::value::PromiseState::Rejected(v) => Some((v.clone(), true)),
                        }
                    };
                    if let Some((value, rejected)) = already {
                        effects.push(EffectJob::Then {
                            on_ok,
                            on_err,
                            env: job_env,
                            value,
                            rejected,
                            result: result.clone(),
                            frame_path: fp,
                        });
                    }
                    return Ok(Value::Promise(result));
                }
                // .catch(f) is sugar for .then(undefined, f).
                if prop == "catch" {
                    let b = eval(base, env, frame, host, components, effects)?;
                    let Value::Promise(p) = &b else {
                        return Err(RuntimeError::new(format!(".catch on non-promise {b}")));
                    };
                    let on_err = match args.first() {
                        Some(a) => match a {
                            JsExpr::Closure { params, body, .. } => {
                                let param =
                                    params.first().cloned().unwrap_or_else(|| "$_".to_string());
                                Some(((**body).clone(), param))
                            }
                            other => {
                                return Err(RuntimeError::new(format!(
                                    ".catch expects an arrow handler, got {other:?}"
                                )))
                            }
                        },
                        None => None,
                    };
                    let result = crate::value::PromiseData::new();
                    let job_env = env.clone();
                    let fp = frame.path().map(|p| p.to_vec());
                    let already = {
                        let mut pd = p.borrow_mut();
                        match &pd.state {
                            crate::value::PromiseState::Pending => {
                                pd.handlers.push(crate::value::Continuation::Then {
                                    on_ok: None,
                                    on_err: on_err.clone(),
                                    env: job_env.clone(),
                                    result: result.clone(),
                                    frame_path: fp.clone(),
                                });
                                None
                            }
                            crate::value::PromiseState::Fulfilled(v) => Some((v.clone(), false)),
                            crate::value::PromiseState::Rejected(v) => Some((v.clone(), true)),
                        }
                    };
                    if let Some((value, rejected)) = already {
                        effects.push(EffectJob::Then {
                            on_ok: None,
                            on_err,
                            env: job_env,
                            value,
                            rejected,
                            result: result.clone(),
                            frame_path: fp,
                        });
                    }
                    return Ok(Value::Promise(result));
                }
                // Generator protocol (M2-T08): next(arg) / return(v) /
                // throw(e) — and the iterator protocol on arrays
                // (.values()/.entries()/.keys() -> a snapshot iterator).
                if matches!(prop.as_str(), "next" | "return" | "throw") {
                    let b = eval(base, env, frame, host, components, effects)?;
                    if let Value::Generator(g) = &b {
                        return generator_call(
                            g, prop, args, env, frame, host, components, effects,
                        );
                    }
                    // Iterator protocol on arrays: {value, done} results.
                    if prop == "next" {
                        if let Value::ArrayIter(it) = &b {
                            return array_iter_next(it);
                        }
                    }
                }
                if matches!(prop.as_str(), "values" | "entries" | "keys") {
                    let b = eval(base, env, frame, host, components, effects)?;
                    if let Value::Array(items) = &b {
                        let kind = match prop.as_str() {
                            "values" => crate::value::ArrayIterKind::Values,
                            "entries" => crate::value::ArrayIterKind::Entries,
                            _ => crate::value::ArrayIterKind::Keys,
                        };
                        return Ok(Value::ArrayIter(std::rc::Rc::new(std::cell::RefCell::new(
                            crate::value::ArrayIterState {
                                items: items.clone(),
                                idx: 0,
                                kind,
                            },
                        ))));
                    }
                }
                // Promise.resolve(v) / Promise.reject(e) statics.
                if let JsExpr::Var(pr) = &**base {
                    if pr == "Promise" && (prop == "resolve" || prop == "reject") {
                        let v = if let Some(a) = args.first() {
                            eval(a, env, frame, host, components, effects)?
                        } else {
                            Value::Undefined
                        };
                        let h = crate::value::PromiseData::new();
                        settle_promise(&h, v, prop == "resolve", effects);
                        return Ok(Value::Promise(h));
                    }
                }
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
                // `arr.concat(x, y, ...)` — ECMA Array.prototype.concat: each
                // argument appends element-wise when it is an array, as a
                // single element otherwise. Returns a NEW array.
                if prop == "concat" {
                    let b = eval(base, env, frame, host, components, effects)?;
                    let Value::Array(items) = &b else {
                        return Err(RuntimeError::new(format!("concat on non-array {b}")));
                    };
                    let mut out = items.clone();
                    for a in args {
                        let v = eval(a, env, frame, host, components, effects)?;
                        match v {
                            Value::Array(vs) => out.extend(vs),
                            other => out.push(other),
                        }
                    }
                    return Ok(Value::Array(out));
                }
                // `Math.random()` — backed by a xorshift64* PRNG (deterministic
                // seed: rendering stays reproducible across runs; randomness
                // quality matches the id-generation use case).
                if let JsExpr::Var(m) = &**base {
                    if m == "Math" && prop == "random" && args.is_empty() {
                        static SEED: std::sync::atomic::AtomicU64 =
                            std::sync::atomic::AtomicU64::new(0x243F_6A88_85A3_08D3);
                        let mut s = SEED.load(std::sync::atomic::Ordering::Relaxed);
                        s ^= s >> 12;
                        s ^= s << 25;
                        s ^= s >> 27;
                        SEED.store(s, std::sync::atomic::Ordering::Relaxed);
                        let r = (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64
                            / (1u64 << 53) as f64;
                        return Ok(Value::Number(r));
                    }
                }
                // `obj.$rest("a", "b")` / `arr.$restFrom(n)` — destructuring
                // rest lowering: own-enumerable leftovers minus listed keys
                // (object), or elements from index n (array).
                if prop == "$rest" {
                    let b = eval(base, env, frame, host, components, effects)?;
                    let mut skip = std::collections::BTreeSet::new();
                    for a in args {
                        let k = eval(a, env, frame, host, components, effects)?;
                        skip.insert(k.as_str_utf8().unwrap_or_else(|| k.display()));
                    }
                    match &b {
                        Value::Object(o) => {
                            let mut props = BTreeMap::new();
                            for (k, v) in o.borrow().props.iter() {
                                if !skip.contains(k) {
                                    props.insert(k.clone(), v.clone());
                                }
                            }
                            return Ok(Value::Object(std::rc::Rc::new(std::cell::RefCell::new(
                                crate::value::ObjData { props, proto: None },
                            ))));
                        }
                        Value::Map(m) => {
                            let rest: BTreeMap<String, Value> = m
                                .iter()
                                .filter(|(k, _)| !skip.contains(*k))
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            return Ok(Value::Map(rest.into_iter().collect()));
                        }
                        other => {
                            return Err(RuntimeError::new(format!(
                                "object rest of non-object {other}"
                            )))
                        }
                    }
                }
                if prop == "$restFrom" && args.len() == 1 {
                    let b = eval(base, env, frame, host, components, effects)?;
                    let n = eval(&args[0], env, frame, host, components, effects)?;
                    let Value::Array(items) = &b else {
                        return Err(RuntimeError::new(format!("array rest of non-array {b}")));
                    };
                    let Some(i) = n.as_number() else {
                        return Err(RuntimeError::new("array rest index must be a number"));
                    };
                    let from = (i as usize).min(items.len());
                    return Ok(Value::Array(items[from..].to_vec()));
                }
                if prop == "filter" && args.len() == 1 {
                    return call_filter(base, &args[0], env, frame, host, components, effects);
                }
                // `arr.every(pred)` — true when the predicate is truthy for
                // every element (vacuously true for `[]`). Same per-item
                // protocol as map/filter.
                if prop == "every" && args.len() == 1 {
                    let arr = eval(base, env, frame, host, components, effects)?;
                    let Value::Array(items) = arr else {
                        return Err(RuntimeError::new("cannot every over non-array"));
                    };
                    let (params, body) = match &args[0] {
                        JsExpr::Closure { params, body, .. } => (params, body),
                        _ => return Err(RuntimeError::new("every expects an arrow function")),
                    };
                    let mut ok = true;
                    for (i, elem) in items.into_iter().enumerate() {
                        env.push_scope();
                        if let Some(p) = params.first() {
                            env.define(p, elem);
                        }
                        if let Some(p) = params.get(1) {
                            env.define(p, Value::Number(i as f64));
                        }
                        // A `return` inside the predicate completes THAT
                        // invocation.
                        let r = eval_function_body(body, env, frame, host, components, effects);
                        env.pop_scope();
                        if !r?.is_truthy() {
                            ok = false;
                            break;
                        }
                    }
                    return Ok(Value::Bool(ok));
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

/// Shared member-write path for `Assign` and `Update` (`x.y = v`, `x.y++`):
/// `ref.current` writes the frame slot; `__proto__` sets the chain link;
/// object members set own props; anything else is a precise error.
fn write_prop(
    base: &Value,
    prop: &str,
    v: Value,
    frame: &mut HookFrame,
) -> Result<(), RuntimeError> {
    match (base, prop) {
        (Value::Ref { slot }, "current") => {
            frame.write_ref(*slot, v);
            Ok(())
        }
        (Value::Object(o), "__proto__") => {
            let mut b = o.borrow_mut();
            b.proto = match &v {
                Value::Object(p) => Some(p.clone()),
                Value::Null => None,
                _ => return Err(RuntimeError::new("__proto__ must be an object or null")),
            };
            Ok(())
        }
        (Value::Object(o), p) => {
            o.borrow_mut().set_own(p.to_string(), v);
            Ok(())
        }
        _ => Err(RuntimeError::new(format!(
            "cannot assign to {prop} on {base}"
        ))),
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
    effects: &mut Vec<EffectJob>,
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
        // `a ?? b`: `a` unless it is null/undefined. Short-circuit: `b`
        // was already evaluated above (strict arg evaluation) — the value
        // choice is what matters; laziness arrives with full lazy-arg
        // evaluation (documented limitation).
        Nullish => {
            return Ok(match &l {
                Value::Null | Value::Undefined => r,
                _ => l,
            })
        }
        // `a | b`: bitwise OR on ToInt32 operands (ECMA 13.11). Used by
        // real-world code (`(x * 64) | 0` as Math.floor).
        BitOr => {
            let li = ecma_to_int32(&l);
            let ri = ecma_to_int32(&r);
            Value::Number(f64::from(li | ri))
        }
    };
    Ok(res)
}

/// ECMA-262 ToInt32 (7.1.6): ToNumber, then modulo 2^32 into signed range.
/// NaN/±Infinity/±0 map to +0.
fn ecma_to_int32(v: &Value) -> i32 {
    let n = ecma_to_number(v);
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    // Truncate toward zero, then take modulo 2^32 as UNSIGNED and reinterpret
    // the bits as signed — exactly ECMA's "modulo 2^32, then if >= 2^31
    // subtract 2^32".
    let truncated = n.trunc();
    // f64 -> u64 via Euclidean remainder on the (huge but finite) value.
    let moduled = truncated.rem_euclid(4294967296.0) as u64;
    moduled as i32
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
        (Value::Function { ident: x, .. }, Value::Function { ident: y, .. }) => {
            std::rc::Rc::ptr_eq(x, y)
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
    effects: &mut Vec<EffectJob>,
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
    effects: &mut Vec<EffectJob>,
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
    effects: &mut Vec<EffectJob>,
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
            let new_state = eval_function_body(&body, &mut denv, frame, host, components, effects)?;
            frame.write_state(*slot, new_state);
            Ok(Value::Null)
        }
        Value::Handler { body, .. } => {
            // Handler values are plain closures; invoking one evaluates its
            // body in the CURRENT env/frame (the caller's render scope —
            // e.g. `this.method()` inside a class render/handler). A
            // `return` inside completes the handler (closure semantics).
            // Event dispatch still exists for the ABI's on* path.
            eval_function_body(body, env, frame, host, components, effects)
        }
        Value::AsyncFn(af) => {
            // An async call returns a promise; segment 0 runs synchronously
            // (ECMA: an async fn body runs to its first await inline).
            // Params bind in a fresh per-call env so concurrent calls of the
            // same fn have isolated locals.
            let result = crate::value::PromiseData::new();
            let mut cenv = af.captured.clone();
            cenv.push_scope();
            for (i, p) in af.params.iter().enumerate() {
                cenv.define(p, args.get(i).cloned().unwrap_or(Value::Undefined));
            }
            let fp = frame.path().map(|p| p.to_vec());
            run_async_step(
                af.clone(),
                0,
                &mut cenv,
                frame,
                host,
                components,
                effects,
                result.clone(),
                fp,
            );
            Ok(Value::Promise(result))
        }
        Value::GeneratorFn(f) => {
            // Calling a generator function creates an instance: params bind
            // in a fresh env over the fn's captured (global) env; nothing
            // runs until the first next() (ECMA laziness).
            let mut cenv = f.captured.clone();
            cenv.push_scope();
            for (i, p) in f.params.iter().enumerate() {
                cenv.define(p, args.get(i).cloned().unwrap_or(Value::Undefined));
            }
            Ok(Value::Generator(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::GeneratorInst {
                    f: f.clone(),
                    env: cenv,
                    seg: 0,
                    done: false,
                    pending_bind: None,
                },
            ))))
        }
        Value::Settler { promise, fulfill } => {
            // `resolve(v)` / `reject(v)` inside a Promise executor: settle
            // the promise, queueing its handlers as jobs. Settling an
            // already-settled promise is a no-op (ECMA).
            let v = args.first().cloned().unwrap_or(Value::Undefined);
            settle_promise(promise, v, *fulfill, effects);
            Ok(Value::Undefined)
        }
        Value::Function {
            params,
            body,
            captured,
            ..
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
            // `return v` inside the body raises through the error channel;
            // catch it here — the carried value is the call's result. A body
            // with no `return` yields the block's value (or null).
            eval_function_body(body, &mut fenv, frame, host, components, effects)
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
    effects: &mut Vec<EffectJob>,
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
                    effects.push(EffectJob::Effect(old));
                }
                if let JsExpr::Closure { body, .. } = &args[0] {
                    effects.push(EffectJob::Effect(EffectBody {
                        body: (**body).clone(),
                        env: env.clone(),
                        layout,
                        frame_path: frame.path().map(|p| p.to_vec()),
                    }));
                }
            }
            Ok(Value::Null)
        }
        "useReducer" => {
            // The reducer is the FIRST arg (params + body); it is stored as
            // IR data (never a function pointer) and evaluated at dispatch
            // time: `reducer(state, action)`. Accepts an inline arrow OR a
            // reference to a module-level reducer (e.g. an imported
            // `todoReducer`, which evaluates to a `Value::Function`).
            let (rparams, rbody) = match args.first() {
                Some(JsExpr::Closure { params, body, .. }) => (params.clone(), (**body).clone()),
                Some(e) => match eval(e, env, frame, host, components, effects)? {
                    Value::Function { params, body, .. } => (params, (*body).clone()),
                    other => {
                        return Err(RuntimeError::new(format!(
                            "useReducer expects a reducer function, got {other}"
                        )))
                    }
                },
                None => return Err(RuntimeError::new("useReducer expects a reducer arrow")),
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
        "Error" => {
            // Error(message) — an Error-shaped object ({name, message}); the
            // form real code throws (`throw new Error("x")`).
            let msg = if let Some(a) = args.first() {
                eval(a, env, frame, host, components, effects)?
            } else {
                Value::Undefined
            };
            Ok(make_error_value(msg))
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
                // ECMA: promises are objects; async fns are functions; the
                // settler is an internal callable.
                Value::Promise(_) => "object",
                Value::AsyncFn(_) => "function",
                Value::GeneratorFn(_) => "function",
                Value::Generator(_) | Value::ArrayIter(_) => "object",
                Value::Settler { .. } => "function",
                Value::ComponentRefVal(_) => "object",
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
                    // the CURRENT render env (it closes over the scope). A
                    // `return` inside completes the factory (closure
                    // semantics — e.g. early exits in a memo computation).
                    let value = match &args[0] {
                        JsExpr::Closure { body, .. } => {
                            eval_function_body(body, env, frame, host, components, effects)?
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
        "memo" => {
            // `memo(fn)` — React's memo HOF. Semantics here are identity:
            // the wrapped function is returned unchanged (no re-render
            // bailout optimization yet — correctness first). Accepts a
            // closure OR an already-evaluated function value.
            match args.first() {
                Some(JsExpr::Closure { params, body, .. }) => Ok(Value::Function {
                    params: params.clone(),
                    body: body.clone(),
                    captured: env.clone(),
                    ident: std::rc::Rc::new(()),
                }),
                Some(e) => {
                    let v = eval(e, env, frame, host, components, effects)?;
                    match &v {
                        Value::Function { .. } => Ok(v),
                        other => Err(RuntimeError::new(format!(
                            "memo expects a function, got {other}"
                        ))),
                    }
                }
                None => Err(RuntimeError::new("memo expects a function")),
            }
        }
        "classnames" => {
            // The `classnames` package (subset): strings pass through,
            // `{key: truthy}` objects contribute keys with truthy values,
            // arrays flatten recursively; falsy values contribute nothing.
            // Called as `classnames(...)` (external import — no module).
            let mut classes = Vec::new();
            for a in args {
                let v = eval(a, env, frame, host, components, effects)?;
                collect_classnames(&v, &mut classes);
            }
            Ok(Value::from_str_utf8(&classes.join(" ")))
        }
        "useLocation" => {
            // `react-router-dom` stub: the static renderer has no router, so
            // the location is always `/` (the "All" view). Real routing
            // arrives with the browser host.
            let mut data = crate::value::ObjData::new();
            data.set_own("pathname".to_string(), Value::from_str_utf8("/"));
            Ok(Value::Object(std::rc::Rc::new(std::cell::RefCell::new(
                data,
            ))))
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
    effects: &mut Vec<EffectJob>,
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
        // A `return` inside the callback completes THAT invocation (JS
        // callback semantics — e.g. early exits in a map/filter body).
        let r = eval_function_body(body, env, frame, host, components, effects);
        env.pop_scope();
        out.push(r?);
    }
    Ok(Value::Array(out))
}

/// Collect `classnames` contributions: strings verbatim, objects by truthy
/// keys, arrays flattened; numbers contribute when nonzero (matching the
/// package for the supported value set).
fn collect_classnames(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Str(s) => {
            let text = String::from_utf16_lossy(s);
            if !text.is_empty() {
                out.push(text);
            }
        }
        Value::Number(n) if *n != 0.0 => out.push(v.display()),
        Value::Bool(true) => {}
        Value::Array(items) => {
            for it in items {
                collect_classnames(it, out);
            }
        }
        Value::Object(o) => {
            for (k, val) in o.borrow().props.iter() {
                if val.is_truthy() {
                    out.push(k.clone());
                }
            }
        }
        Value::Map(m) => {
            for (k, val) in m.iter() {
                if val.is_truthy() {
                    out.push(k.clone());
                }
            }
        }
        _ => {}
    }
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
    effects: &mut Vec<EffectJob>,
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
            // A `return` inside the predicate completes THAT invocation.
            let r = eval_function_body(body, env, frame, host, components, effects);
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
/// An Error-shaped object: `{name: "Error", message}`. `throw new Error(m)`
/// produces one; catches read `e.message`, and the boundary/error path uses
/// the message as the error text.
fn make_error_value(msg: Value) -> Value {
    let mut data = crate::value::ObjData::new();
    data.set_own("name".to_string(), Value::from_str_utf8("Error"));
    data.set_own("message".to_string(), msg);
    Value::Object(std::rc::Rc::new(std::cell::RefCell::new(data)))
}

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
/// A generator's pull-based segment step (M2-T08): run segment `seg` in the
/// instance env; a terminal yield SUSPENDS (Yield outcome), the final
/// segment completes (Done). Unlike async, no job queue — the caller of
/// next() drives.
#[allow(clippy::too_many_arguments)]
fn run_generator_step(
    f: &std::rc::Rc<crate::value::AsyncFnData>,
    seg: usize,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectJob>,
) -> Result<(Value, Option<String>, bool), RuntimeError> {
    // (yield value | completion value, pending bind of the yield, done)
    let step = f.segments[seg].clone();
    let mut outcome: Result<Value, RuntimeError> = Ok(Value::Undefined);
    for s in &step.stmts {
        outcome = eval(s, env, frame, host, components, effects);
        if outcome.is_err() {
            break;
        }
    }
    // `return v` inside the segment COMPLETES the generator with v
    // (`{value: v, done: true}` at the caller).
    let v = match outcome {
        Err(e) => match e.return_value() {
            Some(v) => return Ok((v, None, true)),
            None => return Err(e),
        },
        Ok(v) => v,
    };
    match &step.await_expr {
        Some(yexpr) => {
            let yv = eval(yexpr, env, frame, host, components, effects)?;
            if step.await_completes {
                // `return yield v`: the NEXT next(arg) completes the
                // generator with arg — treat like a yield with a
                // completion flag (handled by the caller via pending_bind
                // + a sentinel done-on-resume; the segment AFTER this one
                // is the empty terminal).
                Ok((yv, step.await_bind.clone(), false))
            } else {
                Ok((yv, step.await_bind.clone(), false))
            }
        }
        None => Ok((v, None, true)),
    }
}

/// `g.next(arg)` / `g.return(v)` / `g.throw(e)` (M2-T08).
#[allow(clippy::too_many_arguments)]
fn generator_call(
    g: &std::rc::Rc<std::cell::RefCell<crate::value::GeneratorInst>>,
    prop: &str,
    args: &[JsExpr],
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectJob>,
) -> Result<Value, RuntimeError> {
    let arg = if let Some(a) = args.first() {
        eval(a, env, frame, host, components, effects)?
    } else {
        Value::Undefined
    };
    match prop {
        "next" => {
            let mut inst = g.borrow_mut();
            if inst.done {
                return Ok(crate::value::iter_result(Value::Undefined, true));
            }
            // Bind the previous yield's target (ECMA: the yield EXPRESSION's
            // value is the next() argument).
            if let Some(b) = inst.pending_bind.take() {
                inst.env.define(&b, arg.clone());
            }
            let f = inst.f.clone();
            let mut ienv = inst.env.clone();
            let seg = inst.seg;
            drop(inst);
            let (value, bind, done) =
                run_generator_step(&f, seg, &mut ienv, frame, host, components, effects)?;
            let mut inst = g.borrow_mut();
            if done {
                inst.done = true;
                Ok(crate::value::iter_result(value, true))
            } else {
                inst.seg = seg + 1;
                inst.pending_bind = bind;
                inst.env = ienv;
                Ok(crate::value::iter_result(value, false))
            }
        }
        "return" => {
            let mut inst = g.borrow_mut();
            inst.done = true;
            Ok(crate::value::iter_result(arg, true))
        }
        "throw" => {
            let mut inst = g.borrow_mut();
            inst.done = true;
            Err(RuntimeError::thrown(arg))
        }
        _ => unreachable!("prop checked by caller"),
    }
}

/// `it.next()` for array iterators (M2-T08): {value, done} over a snapshot.
fn array_iter_next(
    it: &std::rc::Rc<std::cell::RefCell<crate::value::ArrayIterState>>,
) -> Result<Value, RuntimeError> {
    let mut st = it.borrow_mut();
    if st.idx >= st.items.len() {
        return Ok(crate::value::iter_result(Value::Undefined, true));
    }
    let i = st.idx;
    st.idx += 1;
    let v = match st.kind {
        crate::value::ArrayIterKind::Values => st.items[i].clone(),
        crate::value::ArrayIterKind::Keys => Value::Number(i as f64),
        crate::value::ArrayIterKind::Entries => {
            Value::Array(vec![Value::Number(i as f64), st.items[i].clone()])
        }
    };
    Ok(crate::value::iter_result(v, false))
}

/// One state-machine step of an async fn (M2-T07). Runs segment `seg` in
/// `call_env`; a terminal await suspends — the resolved value arrives later
/// as a `EffectJob::Resume` job (scheduler-driven). An error in a segment
/// rejects the result promise (ECMA: an async fn never throws synchronously).
#[allow(clippy::too_many_arguments)]
pub fn run_async_step(
    af: std::rc::Rc<crate::value::AsyncFnData>,
    seg: usize,
    call_env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
    effects: &mut Vec<EffectJob>,
    result: std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
    frame_path: Option<Vec<String>>,
) {
    let step = af.segments[seg].clone();
    let mut outcome: Result<Value, RuntimeError> = Ok(Value::Null);
    for s in &step.stmts {
        outcome = eval(s, call_env, frame, host, components, effects);
        if outcome.is_err() {
            break;
        }
    }
    match outcome {
        Err(e) => {
            // `return v` inside the segment COMPLETES the async fn with v
            // (ECMA: return in an async function resolves its promise);
            // any other failure rejects it with the error value.
            if let Some(v) = e.return_value() {
                settle_promise(&result, v, true, effects);
            } else {
                let v = e.caught_value();
                settle_promise(&result, v, false, effects);
            }
        }
        Ok(v) => {
            match &step.await_expr {
                Some(aexpr) => {
                    // Suspend: the await's promise drives the next step.
                    let av = eval(aexpr, call_env, frame, host, components, effects);
                    match av {
                        Err(e) => {
                            let v = e.caught_value();
                            settle_promise(&result, v, false, effects);
                        }
                        Ok(Value::Promise(p)) => {
                            let mut pd = p.borrow_mut();
                            match &pd.state {
                                crate::value::PromiseState::Pending => {
                                    pd.handlers.push(crate::value::Continuation::Resume {
                                        af,
                                        seg: seg + 1,
                                        bind: step.await_bind.clone(),
                                        completes: step.await_completes,
                                        call_env: call_env.clone(),
                                        result,
                                        frame_path,
                                    });
                                }
                                crate::value::PromiseState::Fulfilled(fv) => {
                                    let fv = fv.clone();
                                    drop(pd);
                                    effects.push(EffectJob::Resume {
                                        af,
                                        seg: seg + 1,
                                        bind: step.await_bind.clone(),
                                        completes: step.await_completes,
                                        call_env: call_env.clone(),
                                        result,
                                        frame_path,
                                        incoming: Some(fv),
                                    });
                                }
                                crate::value::PromiseState::Rejected(rv) => {
                                    let rv = rv.clone();
                                    drop(pd);
                                    // A rejected await rejects the async fn's
                                    // result promise (no local catch of await
                                    // rejections in the supported surface).
                                    settle_promise(&result, rv, false, effects);
                                }
                            }
                        }
                        Ok(other) => {
                            // `await non-promise` resolves with the value
                            // itself (ECMA Await wraps via PromiseResolve).
                            effects.push(EffectJob::Resume {
                                af,
                                seg: seg + 1,
                                bind: step.await_bind.clone(),
                                completes: step.await_completes,
                                call_env: call_env.clone(),
                                result,
                                frame_path,
                                incoming: Some(other),
                            });
                        }
                    }
                }
                None => {
                    // Terminal segment: the fn completes with its last
                    // statement's value — undefined for an empty body
                    // (ECMA: no return -> undefined).
                    let done = if step.stmts.is_empty() {
                        Value::Undefined
                    } else {
                        v
                    };
                    settle_promise(&result, done, true, effects);
                }
            }
        }
    }
}

/// Settle a promise and queue its handlers as jobs (M2-T07). Idempotent
/// (ECMA: first settle wins). Fulfilling with ANOTHER promise adopts it: the
/// result stays pending until the source settles (pass-through), and only
/// that pass-through may complete it (force_settle). Self-adoption rejects
/// with a TypeError (ECMA).
pub fn settle_promise(
    p: &std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
    value: Value,
    fulfilled: bool,
    effects: &mut Vec<EffectJob>,
) {
    let mut pd = p.borrow_mut();
    if pd.settled {
        return;
    }
    if fulfilled {
        if let Value::Promise(other) = &value {
            pd.settled = true; // first resolve wins, even during adoption
            if std::rc::Rc::ptr_eq(p, other) {
                pd.state = crate::value::PromiseState::Rejected(Value::from_str_utf8(
                    "Chaining cycle detected for promise",
                ));
                let handlers = std::mem::take(&mut pd.handlers);
                drop(pd);
                for h in handlers {
                    push_handler_job(h, &value, true, effects);
                }
            } else {
                let already = {
                    let mut od = other.borrow_mut();
                    match &od.state {
                        crate::value::PromiseState::Pending => {
                            od.handlers.push(crate::value::Continuation::Then {
                                on_ok: None,
                                on_err: None,
                                env: Env::new(),
                                result: p.clone(),
                                frame_path: None,
                            });
                            None
                        }
                        crate::value::PromiseState::Fulfilled(v) => Some((v.clone(), false)),
                        crate::value::PromiseState::Rejected(v) => Some((v.clone(), true)),
                    }
                };
                drop(pd);
                if let Some((v, rej)) = already {
                    effects.push(EffectJob::Then {
                        on_ok: None,
                        on_err: None,
                        env: Env::new(),
                        value: v,
                        rejected: rej,
                        result: p.clone(),
                        frame_path: None,
                    });
                }
                // Handlers stay queued on p; the pass-through completes it.
            }
            return;
        }
    }
    pd.settled = true;
    pd.state = if fulfilled {
        crate::value::PromiseState::Fulfilled(value.clone())
    } else {
        crate::value::PromiseState::Rejected(value.clone())
    };
    let handlers = std::mem::take(&mut pd.handlers);
    drop(pd);
    for h in handlers {
        push_handler_job(h, &value, !fulfilled, effects);
    }
}

/// Complete an ADOPTING promise with its source's settled value. Only the
/// pass-through continuation reaches this (settle_promise is blocked by the
/// settled flag the adoption set).
fn force_settle(
    p: &std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
    value: Value,
    fulfilled: bool,
    effects: &mut Vec<EffectJob>,
) {
    let handlers = {
        let mut pd = p.borrow_mut();
        pd.state = if fulfilled {
            crate::value::PromiseState::Fulfilled(value.clone())
        } else {
            crate::value::PromiseState::Rejected(value.clone())
        };
        std::mem::take(&mut pd.handlers)
    };
    for h in handlers {
        push_handler_job(h, &value, !fulfilled, effects);
    }
}

fn push_handler_job(
    h: crate::value::Continuation,
    value: &Value,
    rejected: bool,
    effects: &mut Vec<EffectJob>,
) {
    match h {
        crate::value::Continuation::Then {
            on_ok,
            on_err,
            env,
            result,
            frame_path,
        } => effects.push(EffectJob::Then {
            on_ok,
            on_err,
            env,
            value: value.clone(),
            rejected,
            result,
            frame_path,
        }),
        crate::value::Continuation::Resume {
            af,
            seg,
            bind,
            completes,
            call_env,
            result,
            frame_path,
        } => effects.push(EffectJob::Resume {
            af,
            seg,
            bind,
            completes,
            call_env,
            result,
            frame_path,
            incoming: Some(value.clone()),
        }),
    }
}

/// Complete a promise from a pass-through continuation: an ADOPTING promise
/// (settled but state Pending) is force-completed; a normal pass-through
/// settles normally.
pub fn force_settle_pub(
    p: &std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
    value: Value,
    fulfilled: bool,
    effects: &mut Vec<EffectJob>,
) {
    let adopting = {
        let pd = p.borrow();
        pd.settled && matches!(pd.state, crate::value::PromiseState::Pending)
    };
    if adopting {
        force_settle(p, value, fulfilled, effects);
    } else {
        settle_promise(p, value, fulfilled, effects);
    }
}

/// Extract `(onOk, onErr)` from a `.then(a, b)` argument list. `.then(f)`
/// is a fulfillment handler only; `.then(f, g)` both. Non-arrow args are a
/// runtime error (the supported surface passes arrows).
/// A .then handler: (body, param-name) — None = no handler (pass-through).
type ThenHandler = Option<(JsExpr, String)>;

fn then_callbacks(args: &[JsExpr]) -> Result<(ThenHandler, ThenHandler), RuntimeError> {
    let pick = |a: &JsExpr| -> Result<(JsExpr, String), RuntimeError> {
        match a {
            JsExpr::Closure { params, body, .. } => {
                let p = params.first().cloned().unwrap_or_else(|| "$_".to_string());
                Ok(((**body).clone(), p))
            }
            other => Err(RuntimeError::new(format!(
                ".then expects arrow handlers, got {other:?}"
            ))),
        }
    };
    let on_ok = match args.first() {
        Some(a) => Some(pick(a)?),
        None => None,
    };
    let on_err = match args.get(1) {
        Some(a) => Some(pick(a)?),
        None => None,
    };
    Ok((on_ok, on_err))
}

pub fn run_effect_body(
    body: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
) -> Result<Vec<EffectJob>, RuntimeError> {
    let mut effects = Vec::new();
    // A `return` inside completes the setup early (its value is discarded;
    // cleanup arrows are discovered statically by `cleanup_of`).
    eval_function_body(body, env, frame, host, components, &mut effects)?;
    Ok(effects)
}
