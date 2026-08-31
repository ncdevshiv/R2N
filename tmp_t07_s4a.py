def patch(path, old, new, count=1):
    s = open(path, encoding='utf-8').read()
    assert s.count(old) == count, f"{path}: count={s.count(old)} for {old[:60]!r}"
    s = s.replace(old, new, count)
    open(path, 'w', encoding='utf-8', newline='\n').write(s)
    print(f"patched {path}")

# ---- value.rs: extend Continuation::Resume with the full resume state ----
patch('crates/r2n-runtime/src/value.rs', '''    /// An async fn suspended at an await: on settle, resume at `seg` with the
    /// resolved value (M2-T07).
    Resume {
        af: std::rc::Rc<AsyncFnData>,
        seg: usize,
        result: std::rc::Rc<std::cell::RefCell<PromiseData>>,
    },''', '''    /// An async fn suspended at an await: on settle, resume at `seg` —
    /// bind the resolved value to `bind` (the awaited segment's `x = await p`
    /// target), or complete the result promise when `completes`
    /// (`return await p`). `call_env` is the per-call env (its top frame is
    /// fresh per call, so concurrent calls of the same async fn do not
    /// clobber each other's locals).
    Resume {
        af: std::rc::Rc<AsyncFnData>,
        seg: usize,
        bind: Option<String>,
        completes: bool,
        call_env: crate::eval::Env,
        result: std::rc::Rc<std::cell::RefCell<PromiseData>>,
        frame_path: Option<Vec<String>>,
    },''')

# ---- hooks.rs: EffectJob enum ----
patch('crates/r2n-runtime/src/hooks.rs', '''pub struct EffectBody {''', '''/// A unit of deferred runtime work (M2-T07): a React effect OR a promise
/// continuation. Both drain through the same scheduler loop — promise
/// continuations are scheduler effects, which is what makes async/await
/// deterministic in a zero-JS runtime.
#[derive(Debug, Clone)]
pub enum EffectJob {
    /// A plain effect body (useEffect/useLayoutEffect setup, cleanup).
    Effect(EffectBody),
    /// A `.then` handler whose source promise has settled: run the chosen
    /// body with `value` bound to its param; the outcome settles `result`
    /// (fulfilled with the value, rejected on a throw) — ECMA chaining.
    Then {
        on_ok: Option<(r2n_ir::js::JsExpr, String)>,
        on_err: Option<(r2n_ir::js::JsExpr, String)>,
        env: crate::eval::Env,
        value: Value,
        rejected: bool,
        result: std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
        frame_path: Option<Vec<String>>,
    },
    /// An async-fn state-machine step: bind `incoming` to `bind` (or complete
    /// the promise when `completes`), run segment `seg`, and suspend at its
    /// terminal await if it has one.
    Resume {
        af: std::rc::Rc<crate::value::AsyncFnData>,
        seg: usize,
        bind: Option<String>,
        completes: bool,
        call_env: crate::eval::Env,
        result: std::rc::Rc<std::cell::RefCell<crate::value::PromiseData>>,
        frame_path: Option<Vec<String>>,
        incoming: Option<Value>,
    },
}

pub struct EffectBody {''')
