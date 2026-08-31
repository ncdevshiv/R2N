def patch(path, old, new, count=1):
    s = open(path, encoding='utf-8').read()
    assert s.count(old) == count, f"{path}: count={s.count(old)} for {old[:60]!r}"
    s = s.replace(old, new, count)
    open(path, 'w', encoding='utf-8', newline='\n').write(s)
    print(f"patched {path}")

# ============ eval.rs remaining ============

# adoption-aware settle: a promise fulfilled with ANOTHER promise adopts it
# (pass-through continuation) — ECMA PromiseResolve/[[Adopt]].
patch('crates/r2n-runtime/src/eval.rs', '''pub fn settle_promise(
    p: &std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
    value: Value,
    fulfilled: bool,
    effects: &mut Vec<EffectJob>,
) {
    let handlers = {''', '''pub fn settle_promise(
    p: &std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
    value: Value,
    fulfilled: bool,
    effects: &mut Vec<EffectJob>,
) {
    // Adoption (ECMA): fulfilling with a promise P makes the result track P
    // (a pass-through continuation). Self-adoption is a TypeError.
    if fulfilled {
        if let Value::Promise(other) = &value {
            if !std::rc::Rc::ptr_eq(p, other) {
                let mut pd = p.borrow_mut();
                if matches!(pd.state, crate::value::PromiseState::Pending) {
                    pd.state = crate::value::PromiseState::Fulfilled(Value::Undefined);
                    pd.handlers.push(crate::value::Continuation::Then {
                        on_ok: None,
                        on_err: None,
                        env: Env::new(),
                        result: p.clone(),
                        frame_path: None,
                    });
                    drop(pd);
                    // Settle `other`'s record so its handlers queue: mark it
                    // fulfilled with the promise? No — register our
                    // pass-through on `other` via a settle of OUR marker...
                    // Simplest: push a Then job that resolves when other
                    // settles is not possible without registering. Instead:
                    // attach the continuation to `other`.
                    drop(pd);
                    other.borrow_mut().handlers.push(crate::value::Continuation::Then {
                        on_ok: None,
                        on_err: None,
                        env: Env::new(),
                        result: p.clone(),
                        frame_path: None,
                    });
                    if !matches!(
                        other.borrow().state,
                        crate::value::PromiseState::Pending
                    ) {
                        // already settled: queue the pass-through now
                        let (sv, srej) = match &other.borrow().state {
                            crate::value::PromiseState::Fulfilled(v) => (v.clone(), false),
                            crate::value::PromiseState::Rejected(v) => (v.clone(), true),
                            crate::value::PromiseState::Pending => unreachable!(),
                        };
                        effects.push(EffectJob::Then {
                            on_ok: None,
                            on_err: None,
                            env: Env::new(),
                            value: sv,
                            rejected: srej,
                            result: p.clone(),
                            frame_path: None,
                        });
                    }
                    return;
                }
            }
        }
    }
    let handlers = {''')

# Promise.resolve / Promise.reject statics (next to the Object statics)
patch('crates/r2n-runtime/src/eval.rs', '''                if prop == "map" && args.len() == 1 {
                    return call_map(base, &args[0], env, frame, host, components, effects);
                }''', '''                // Promise.resolve(v) / Promise.reject(e) statics.
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
                if prop == "map" && args.len() == 1 {
                    return call_map(base, &args[0], env, frame, host, components, effects);
                }''')

# .catch(f) sugar = .then(undefined, f)
patch('crates/r2n-runtime/src/eval.rs', '''                    return Ok(Value::Promise(result));
                }
                // Object.create(proto) / Object.getPrototypeOf(obj).''', '''                    return Ok(Value::Promise(result));
                }
                // .catch(f) is sugar for .then(undefined, f).
                if prop == "catch" {
                    let b = eval(base, env, frame, host, components, effects)?;
                    let Value::Promise(p) = &b else {
                        return Err(RuntimeError::new(format!(
                            ".catch on non-promise {b}"
                        )));
                    };
                    let on_err = match args.first() {
                        Some(a) => Some(
                            match a {
                                JsExpr::Closure { params, body, .. } => {
                                    let param = params
                                        .first()
                                        .cloned()
                                        .unwrap_or_else(|| "$_".to_string());
                                    ((**body).clone(), param)
                                }
                                other => {
                                    return Err(RuntimeError::new(format!(
                                        ".catch expects an arrow handler, got {other:?}"
                                    )))
                                }
                            },
                        ),
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
                            result,
                            frame_path: fp,
                        });
                    }
                    return Ok(Value::Promise(result));
                }
                // Object.create(proto) / Object.getPrototypeOf(obj).''')

# new Promise(executor)
patch('crates/r2n-runtime/src/eval.rs', '''            if name == "Error" {
                // `new Error(msg)` — same object as a plain Error(msg) call.''', '''            if name == "Promise" {
                // `new Promise(executor)` (M2-T07): the executor runs
                // synchronously with (resolve, reject) settlers; a throw in
                // it rejects the promise (ECMA).
                let Some(JsExpr::Closure { params, body, .. }) = args.first() else {
                    return Err(RuntimeError::new(
                        "new Promise requires an executor arrow",
                    ));
                };
                let handle = crate::value::PromiseData::new();
                let fval = Value::Function {
                    params: params.clone(),
                    body: body.clone(),
                    captured: env.clone(),
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
                // `new Error(msg)` — same object as a plain Error(msg) call.''')

# run_effect_body returns spawned jobs so nested async work re-enters the queue
patch('crates/r2n-runtime/src/eval.rs', '''pub fn run_effect_body(
    body: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
) -> Result<(), RuntimeError> {
    let mut effects = Vec::new();
    eval(body, env, frame, host, components, &mut effects)?;
    Ok(())
}''', '''pub fn run_effect_body(
    body: &JsExpr,
    env: &mut Env,
    frame: &mut HookFrame,
    host: &mut dyn Host,
    components: &[RuntimeComponent],
) -> Result<Vec<EffectJob>, RuntimeError> {
    let mut effects = Vec::new();
    eval(body, env, frame, host, components, &mut effects)?;
    Ok(effects)
}''')

# typeof AsyncFn = "function"
patch('crates/r2n-runtime/src/eval.rs', '''                Value::Function { .. } | Value::Handler { .. } => "function",''', '''                Value::Function { .. } | Value::Handler { .. } | Value::AsyncFn(_) => "function",''')

# JsExpr::AsyncFn eval arm: the VALUE of an async arrow
patch('crates/r2n-runtime/src/eval.rs', '''        JsExpr::Throw { value } => {
            let v = eval(value, env, frame, host, components, effects)?;
            Err(RuntimeError::thrown(v))
        }''', '''        JsExpr::AsyncFn { params, segments } => Ok(Value::AsyncFn(std::rc::Rc::new(
            crate::value::AsyncFnData {
                params: params.clone(),
                segments: segments.clone(),
                captured: env.clone(),
            },
        ))),
        JsExpr::Throw { value } => {
            let v = eval(value, env, frame, host, components, effects)?;
            Err(RuntimeError::thrown(v))
        }''')
