//! M2-T08 acceptance: generators & the iterator protocol.
//!
//! ECMA-262 observable semantics:
//! 1. `function*` declarations are top-level; calling one creates a
//!    generator instance lazily (nothing runs until the first next()).
//! 2. `next()` drives pull-based segments: `{value, done:false}` per yield,
//!    `{value: <return>, done:true}` at completion; next-after-done yields
//!    `{undefined, true}` forever.
//! 3. `next(arg)` injects arg as the yield EXPRESSION's value (`let x =
//!    yield v` / `x = yield v`).
//! 4. `return(v)` completes with `{v, true}`; `throw(e)` raises at the
//!    caller (no catch segments in the supported surface) and kills the
//!    generator.
//! 5. Array iterators: `.values()/.entries()/.keys()` produce the same
//!    `{value, done}` protocol over a snapshot.
//! 6. Generators are visible in EVERY component (global env).

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn logs_after_flush(src: &str) -> Vec<String> {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let _ = rt.flush().expect("flush");
    rt.logs().to_vec()
}

fn tree_after_flush(src: &str) -> String {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    r.render_string()
}

#[test]
fn generator_multi_yield_pull_sequence() {
    // ECMA order: NOTHING runs until the first next() (laziness), then one
    // segment per next().
    let logs = logs_after_flush(
        r#"
        function* counter() {
            console.log("start");
            yield 1;
            yield 2;
            yield 3;
        }
        component App() {
            let g = counter();
            console.log("created");
            let a = g.next();
            console.log("a", a.value, a.done);
            let b = g.next();
            console.log("b", b.value, b.done);
            let c = g.next();
            console.log("c", c.value, c.done);
            let d = g.next();
            console.log("d", d.value, d.done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec![
            "created",
            "start",
            "a 1 false",
            "b 2 false",
            "c 3 false",
            "d undefined true",
        ],
        "pull-based sequence with laziness: {logs:?}"
    );
}

#[test]
fn generator_completion_value_and_done() {
    // `return v` completes with {value: v, done: true}.
    let logs = logs_after_flush(
        r#"
        function* seq() {
            yield "a";
            return "fin";
        }
        component App() {
            let g = seq();
            let r1 = g.next();
            console.log("r1", r1.value, r1.done);
            let r2 = g.next();
            console.log("r2", r2.value, r2.done);
            let r3 = g.next();
            console.log("r3", r3.value, r3.done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["r1 a false", "r2 fin true", "r3 undefined true"],
        "completion + post-done: {logs:?}"
    );
}

#[test]
fn next_arg_injects_yield_value() {
    // `let x = yield v` / `x = yield v`: the next next(arg) binds arg.
    let logs = logs_after_flush(
        r#"
        function* talk() {
            console.log("q1");
            let answer = yield "what?";
            console.log("a1", answer);
            let reply = yield "again";
            console.log("a2", reply);
            return answer + reply;
        }
        component App() {
            let g = talk();
            console.log("r1", g.next().value);
            console.log("r2", g.next("yes").value);
            let r3 = g.next("no");
            console.log("r3", r3.value, r3.done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec![
            "q1",
            "r1 what?",
            "a1 yes",
            "r2 again",
            "a2 no",
            "r3 yesno true",
        ],
        "next(arg) injection: {logs:?}"
    );
}

#[test]
fn generator_state_accumulates_across_nexts() {
    // The instance env persists: locals mutated between yields keep their
    // values (a classic generator counter).
    let logs = logs_after_flush(
        r#"
        function* tally() {
            let n = 0;
            n = n + 2;
            yield n;
            n = n + 2;
            yield n;
            n = n + 2;
            yield n;
        }
        component App() {
            let g = tally();
            console.log(g.next().value);
            console.log(g.next().value);
            console.log(g.next().value);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["2", "4", "6"], "state across nexts: {logs:?}");
}

#[test]
fn generator_return_kills_instance() {
    // g.return(v) completes with {v, true}; further next()s are done.
    let logs = logs_after_flush(
        r#"
        function* gen() {
            yield 1;
            yield 2;
        }
        component App() {
            let g = gen();
            g.next();
            let r = g.return("early");
            console.log("returned", r.value, r.done);
            let n = g.next();
            console.log("after", n.value, n.done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["returned early true", "after undefined true"],
        "early return: {logs:?}"
    );
}

#[test]
fn generator_throw_raises_and_kills() {
    // g.throw(e) raises at the CALLER (no catch segments in the surface)
    // and the generator is done.
    let logs = logs_after_flush(
        r#"
        function* gen() {
            yield 1;
        }
        component App() {
            let g = gen();
            g.next();
            let out = "ok";
            try {
                g.throw("kaboom");
            } catch (e) {
                out = "caught:" + e;
            }
            console.log(out);
            let n = g.next();
            console.log("after", n.done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["caught:kaboom", "after true"],
        "throw kills the generator: {logs:?}"
    );
}

#[test]
fn generators_visible_in_every_component() {
    // Top-level declarations are GLOBAL: a CHILD component consumes the
    // generator (global env chains into every component env).
    let logs = logs_after_flush(
        r#"
        function* ids() {
            yield "x1";
            yield "x2";
        }
        component Consumer() {
            let g = ids();
            console.log("child", g.next().value);
            return <p>child</p>;
        }
        component App() {
            return <div><Consumer/></div>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["child x1"], "global visibility: {logs:?}");
}

#[test]
fn generator_yield_bare_and_rendered() {
    // Bare `yield;` produces undefined; a generator's results render.
    let tree = tree_after_flush(
        r#"
        function* beats() {
            yield 1;
            yield;
            yield 3;
        }
        component App() {
            let g = beats();
            let a = g.next().value;
            let b = g.next().value;
            let c = g.next().value;
            return <div><p>{a}</p><p>{b}</p><p>{c}</p></div>;
        }
        export default App;
    "#,
    );
    assert!(
        tree.contains(">1<") && tree.contains(">3<"),
        "rendered generator values: {tree}"
    );
}

#[test]
fn array_values_iterator() {
    // Iterator protocol over a snapshot: {value, done} until exhaustion.
    let logs = logs_after_flush(
        r#"
        component App() {
            let items = [10, 20, 30];
            let it = items.values();
            let a = it.next();
            let b = it.next();
            let c = it.next();
            let d = it.next();
            console.log("a", a.value, a.done);
            console.log("b", b.value, b.done);
            console.log("c", c.value, c.done);
            console.log("d", d.value, d.done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(
        logs,
        vec!["a 10 false", "b 20 false", "c 30 false", "d undefined true",],
        "array values(): {logs:?}"
    );
}

#[test]
fn array_entries_iterator_pairs() {
    let logs = logs_after_flush(
        r#"
        component App() {
            let items = ["a", "b"];
            let it = items.entries();
            let e0 = it.next();
            let e1 = it.next();
            console.log("e0", e0.value[0], e0.value[1]);
            console.log("e1", e1.value[0], e1.value[1]);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["e0 0 a", "e1 1 b"], "array entries(): {logs:?}");
}

#[test]
fn array_keys_iterator() {
    let logs = logs_after_flush(
        r#"
        component App() {
            let items = ["a", "b", "c"];
            let it = items.keys();
            console.log(it.next().value);
            console.log(it.next().value);
            console.log(it.next().value);
            console.log(it.next().done);
            return <div/>;
        }
        export default App;
    "#,
    );
    assert_eq!(logs, vec!["0", "1", "2", "true"], "array keys(): {logs:?}");
}

#[test]
fn yield_outside_generator_is_compile_error() {
    let src = r#"
        component App() {
            let v = yield 1;
            return <div/>;
        }
        export default App;
    "#;
    let err = compile_source(src).expect_err("yield outside generator must fail");
    let msg = format!("{err}");
    assert!(msg.contains("yield"), "precise error mentions yield: {msg}");
}

#[test]
fn yield_nested_in_expression_is_compile_error() {
    let src = r#"
        function* gen() {
            let v = 1 + yield 2;
            return v;
        }
        component App() {
            return <div/>;
        }
        export default App;
    "#;
    let err = compile_source(src).expect_err("nested yield must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("statement value"),
        "precise compile error for nested yield: {msg}"
    );
}
