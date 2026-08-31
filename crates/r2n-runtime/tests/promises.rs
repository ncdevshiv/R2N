//! M2-T07 acceptance: promises + async/await with scheduler-driven
//! continuations.
//!
//! ECMA-262 observable semantics through the runtime:
//! 1. `new Promise(executor)` — the executor runs synchronously; resolve /
//!    reject settle the promise (idempotent).
//! 2. `.then(onOk, onErr)` / `.catch(onErr)` — always async (jobs drain at
//!    defined scheduler points), chaining with pass-through, handler results
//!    (incl. returned promises) adopting into the chained promise.
//! 3. `async () => { ... await p; ... }` — a state machine: segment 0 runs
//!    synchronously, each await suspends, continuations are scheduler jobs.
//! 4. A rejected await rejects the async fn's result; a throw inside an
//!    async fn rejects it too (never throws synchronously).
//! 5. Continuations integrate with the React scheduler: a state setter in an
//!    await continuation re-renders and emits patches.
//!
//! Observation uses `console.log` (render-locals re-initialize per render —
//! React semantics — so log order is the honest observable).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn logs_after_flush(src: &str) -> Vec<String> {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let _ = rt.flush().expect("flush");
    rt.logs().to_vec()
}

#[test]
fn promise_executor_and_then() {
    // Executor runs synchronously; resolve settles; .then queues the
    // continuation as a job — the drain completes within one flush.
    let logs = logs_after_flush(
        r#"
        component App() {
            console.log("before");
            let p = new Promise((resolve, reject) => {
                console.log("executor");
                resolve("value-42");
            });
            p.then((v) => { console.log("then", v); });
            console.log("after-then");
            return <div/>;
        }
        export default App;
    "#,
    );
    // ECMA order: executor sync, then-callback async (after the sync body).
    assert_eq!(
        logs,
        vec!["before", "executor", "after-then", "then value-42"],
        "executor sync + then async: {logs:?}"
    );
}

#[test]
fn settle_is_idempotent() {
    // Second resolve() is a no-op (ECMA); the handler sees the FIRST value.
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => {
                resolve("first");
                resolve("second");
            });
            p.then((v) => { console.log("got", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["got first"], "first resolve wins: {logs:?}");
}

#[test]
fn then_chaining_with_transformation() {
    // p -> f -> g: each handler's return value fulfills the chained promise.
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { resolve(1); });
            p.then((v) => { return v + 1; })
             .then((v) => { return v * 10; })
             .then((v) => { console.log("chain", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["chain 20"], "chained transformation: {logs:?}");
}

#[test]
fn then_pass_through_without_handler() {
    // .then() with no callbacks passes the value through (ECMA identity).
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { resolve("plain"); });
            p.then().then((v) => { console.log("passthrough", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["passthrough plain"], "pass-through: {logs:?}");
}

#[test]
fn rejection_caught_by_catch() {
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { reject("bad-input"); });
            p.catch((e) => { console.log("caught", e); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["caught bad-input"], "catch handler: {logs:?}");
}

#[test]
fn rejection_propagates_through_then_without_handler() {
    // .then(f) on a rejected promise (no onErr) propagates the rejection.
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { reject("boom"); });
            p.then((v) => { console.log("wrong", v); })
             .catch((e) => { console.log("propagated", e); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["propagated boom"],
        "rejection skipped the then, reached catch: {logs:?}"
    );
}

#[test]
fn promise_resolve_reject_statics() {
    let logs = logs_after_flush(
        r#"
        component App() {
            Promise.resolve("static-ok").then((v) => { console.log("ok", v); });
            Promise.reject("static-err").catch((e) => { console.log("err", e); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["ok static-ok", "err static-err"],
        "statics settle: {logs:?}"
    );
}

#[test]
fn async_await_full_flow() {
    // THE core flow: async fn awaits a promise; the continuation (segment 1)
    // runs as a scheduler job; its returned value fulfills the async
    // promise; the outer .then observes it.
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { resolve(40); });
            let get = async () => {
                let v = await p;
                return v + 2;
            };
            get().then((v) => { console.log("answer", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["answer 42"], "async/await full flow: {logs:?}");
}

#[test]
fn async_multiple_awaits_sequential() {
    // Two awaits = three segments: values flow through the binds in order.
    let logs = logs_after_flush(
        r#"
        component App() {
            let p1 = new Promise((resolve, reject) => { resolve("A"); });
            let p2 = new Promise((resolve, reject) => { resolve("B"); });
            let run = async () => {
                let x = await p1;
                console.log("got-x", x);
                let y = await p2;
                return x + y;
            };
            run().then((v) => { console.log("sum", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["got-x A", "sum AB"],
        "sequential awaits: {logs:?}"
    );
}

#[test]
fn await_non_promise_value() {
    // ECMA: await of a non-promise resolves with the value itself.
    let logs = logs_after_flush(
        r#"
        component App() {
            let run = async () => {
                let v = await 7;
                return v * 3;
            };
            run().then((v) => { console.log("n", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["n 21"], "await non-promise: {logs:?}");
}

#[test]
fn async_return_await_completes_with_value() {
    // `return await p` — the resolved value completes the async fn (vs a
    // bare terminal `await p;` which would complete with undefined).
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { resolve("final"); });
            let run = async () => {
                return await p;
            };
            let run2 = async () => {
                await p;
            };
            run().then((v) => { console.log("r", v); });
            run2().then((v) => { console.log("bare", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["r final", "bare undefined"],
        "return await vs bare await: {logs:?}"
    );
}

#[test]
fn throw_inside_async_rejects() {
    // An async fn NEVER throws synchronously — the throw rejects its result.
    let logs = logs_after_flush(
        r#"
        component App() {
            let run = async () => {
                throw "async-boom";
            };
            run().catch((e) => { console.log("rejected", e); });
            console.log("caller-continued");
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["caller-continued", "rejected async-boom"],
        "async throw rejects (caller not interrupted): {logs:?}"
    );
}

#[test]
fn await_rejected_promise_rejects_async_result() {
    let logs = logs_after_flush(
        r#"
        component App() {
            let p = new Promise((resolve, reject) => { reject("denied"); });
            let run = async () => {
                let v = await p;
                return v;
            };
            run().catch((e) => { console.log("await-rejected", e); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["await-rejected denied"],
        "rejected await surfaces as async rejection: {logs:?}"
    );
}

#[test]
fn async_handler_returns_promise_adopted() {
    // A .then handler returning a promise: the chained promise adopts it.
    let logs = logs_after_flush(
        r#"
        component App() {
            let inner = new Promise((resolve, reject) => { resolve("inner-value"); });
            let outer = Promise.resolve("start");
            outer.then((v) => { return inner; }).then((v) => { console.log("adopted", v); });
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["adopted inner-value"], "adoption: {logs:?}");
}

#[test]
fn await_continuation_updates_state_and_rerenders() {
    // Scheduler integration: a setState inside an await continuation marks
    // the frame dirty; the flush re-renders and emits the patch.
    let src = r#"
        component Counter() {
            let n = useState(0);
            let setN = n[1];
            let load = async () => {
                let v = await Promise.resolve(5);
                setN(v);
            };
            useEffect(() => { load(); }, []);
            return <div><p>{n[0]}</p></div>;
        }
        export default Counter;
    "#;
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    let text = r.render_string();
    assert!(
        text.contains(">5<"),
        "state updated from await continuation: {text}"
    );
}

#[test]
fn await_outside_async_is_compile_error() {
    let src = r#"
        component App() {
            let v = await Promise.resolve(1);
            return <div><p>{v}</p></div>;
        }
        export default App;
    "#;
    let err = compile_source(src).expect_err("await outside async must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("await"),
        "precise compile error mentions await: {msg}"
    );
}

#[test]
fn await_nested_in_expression_is_compile_error() {
    // The supported surface is statement-position awaits; anything else is
    // a precise compile error, not a silent miscompile.
    let src = r#"
        component App() {
            let run = async () => {
                let v = 1 + await Promise.resolve(2);
                return v;
            };
            return <div/>;
        }
        export default App;
    "#;
    let err = compile_source(src).expect_err("nested await must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("statement value"),
        "precise compile error for nested await: {msg}"
    );
}
