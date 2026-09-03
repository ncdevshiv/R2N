//! M2 statements: general statement grammar in plain functions — `while` /
//! `for` / `switch` (with fall-through + `break`), early `return`, bare
//! `return;`, destructuring (object/array/rest/defaults), function
//! expressions, param defaults, and the builtins real code needs
//! (`concat`, `Math.random`, `every`, `memo`, `classnames`, `useLocation`).
//!
//! Behavioral tests only: compile a source, flush, and assert on the
//! rendered tree / console logs.

use r2n_compiler::compile_source;
use r2n_renderer_memory::MemoryRenderer;
use r2n_runtime::{Renderer, Runtime};

fn setup(src: &str) -> (Runtime, MemoryRenderer) {
    let template = compile_source(src).expect("compile");
    let mut rt = Runtime::new(template);
    let patches = rt.flush().expect("flush");
    let mut r = MemoryRenderer::new();
    r.apply(&patches);
    (rt, r)
}

fn logs(src: &str) -> Vec<String> {
    let (rt, _) = setup(src);
    rt.logs().to_vec()
}

#[test]
fn while_with_early_return_in_plain_function() {
    let out2 = logs(
        r#"
        function find_first(n) {
            let i = 0;
            while (i < 10) {
                if (i == n) {
                    return i;
                }
                i = i + 1;
            }
            return 0 - 1;
        }
        component App() {
            console.log(find_first(3));
            console.log(find_first(99));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out2, vec!["3", "-1"]);
}

#[test]
fn switch_matches_and_falls_through_until_break() {
    let out = logs(
        r#"
        function grade(n) {
            let out = "";
            switch (n) {
                case 1:
                    out = "one";
                    break;
                case 2:
                case 3:
                    out = "two-three";
                    break;
                default:
                    out = "other";
            }
            return out;
        }
        component App() {
            console.log(grade(1));
            console.log(grade(2));
            console.log(grade(3));
            console.log(grade(9));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["one", "two-three", "two-three", "other"]);
}

#[test]
fn switch_case_early_return_skips_trailing_throw() {
    // The reducer shape: each case returns; the trailing throw only fires
    // when nothing matched.
    let out = logs(
        r#"
        function pick(t) {
            switch (t) {
                case 1:
                    return "one";
                case 2:
                    return "two";
            }
            throw "nope";
        }
        component App() {
            console.log(pick(2));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["two"]);
}

#[test]
fn switch_no_match_runs_default() {
    let out = logs(
        r#"
        function pick(t) {
            switch (t) {
                case 1:
                    return "one";
                default:
                    return "dflt";
            }
        }
        component App() {
            console.log(pick(7));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["dflt"]);
}

#[test]
fn for_loop_accumulates() {
    let out = logs(
        r#"
        function sum(n) {
            let total = 0;
            for (let i = 0; i < n; i = i + 1) {
                total = total + i;
            }
            return total;
        }
        component App() {
            console.log(sum(5));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["10"]);
}

#[test]
fn for_loop_continue_skips() {
    let out = logs(
        r#"
        function sum_odd(n) {
            let total = 0;
            for (let i = 0; i < n; i = i + 1) {
                if (i % 2 == 0) {
                    continue;
                }
                total = total + i;
            }
            return total;
        }
        component App() {
            console.log(sum_odd(6));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["9"]);
}

#[test]
fn object_and_array_destructuring_in_function() {
    let out = logs(
        r#"
        function f(o) {
            const { a, b: renamed } = o;
            const [x, y] = o.pair;
            return a + renamed + x + y;
        }
        component App() {
            console.log(f({ a: 1, b: 2, pair: [10, 20] }));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["33"]);
}

#[test]
fn destructure_rest_collects_leftovers() {
    let out = logs(
        r#"
        function f(o) {
            const { a, ...rest } = o;
            return rest.b + rest.c;
        }
        function g(arr) {
            const [h, ...tail] = arr;
            return h + tail[0];
        }
        component App() {
            console.log(f({ a: 1, b: 2, c: 3 }));
            console.log(g([10, 20, 30]));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["5", "30"]);
}

#[test]
fn bare_return_exits_early_with_undefined() {
    let out = logs(
        r#"
        function f(x) {
            if (x < 0) {
                return;
            }
            return x * 2;
        }
        component App() {
            console.log(f(0 - 5));
            console.log(f(21));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["undefined", "42"]);
}

#[test]
fn param_defaults_apply_to_undefined() {
    let out = logs(
        r#"
        function sized(size = 21) {
            return size;
        }
        component App() {
            console.log(sized());
            console.log(sized(7));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["21", "7"]);
}

#[test]
fn function_expression_value_calls() {
    let out = logs(
        r#"
        component App() {
            const double = function (x) {
                return x * 2;
            };
            console.log(double(21));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["42"]);
}

#[test]
fn array_concat_appends_values_and_arrays() {
    let out = logs(
        r#"
        component App() {
            console.log([1, 2].concat(3, [4, 5]));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["[1, 2, 3, 4, 5]"]);
}

#[test]
fn math_random_returns_unit_range_number() {
    let out = logs(
        r#"
        component App() {
            const r = Math.random();
            console.log(r >= 0 && r < 1);
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["true"]);
}

#[test]
fn array_every_checks_all() {
    let out = logs(
        r#"
        component App() {
            console.log([2, 4, 6].every((x) => x % 2 == 0));
            console.log([2, 3, 6].every((x) => x % 2 == 0));
            console.log([].every((x) => x));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["true", "false", "true"]);
}

#[test]
fn memo_returns_the_function_unchanged() {
    let out = logs(
        r#"
        function Item(x) {
            return x * 2;
        }
        component App() {
            const M = memo(Item);
            console.log(M(21));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["42"]);
}

#[test]
fn classnames_joins_truthy_keys() {
    let out = logs(
        r#"
        component App() {
            console.log(classnames({ a: true, b: false, c: 1 }));
            console.log(classnames("x", ["y", { z: true }]));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["a c", "x y z"]);
}

#[test]
fn component_destructured_params_bind_by_prop_name() {
    // Props bind by NAME (React semantics): declaration order and JSX
    // attribute order may differ without misbinding.
    let (_, r) = setup(
        r#"
        component Child({ b, a }) {
            return <span>{a}-{b}</span>;
        }
        component App() {
            return <div><Child b="B" a="A"/></div>;
        }
        export default App;
        "#,
    );
    let text = r.render_string();
    assert!(text.contains("<span>A-B</span>"), "got: {text}");
}

#[test]
fn component_param_defaults_fill_missing_props() {
    let (_, r) = setup(
        r#"
        component Child({ label, editing = false }) {
            return <span>{editing ? "E" : "R"}:{label}</span>;
        }
        component App() {
            return <div><Child label="L"/></div>;
        }
        export default App;
        "#,
    );
    let text = r.render_string();
    assert!(text.contains("<span>R:L</span>"), "got: {text}");
}

#[test]
fn component_body_destructuring_binds_names() {
    let (_, r) = setup(
        r#"
        component Child({ a, b, pair }) {
            const [x, y] = pair;
            return <span>{a}{b}{x}{y}</span>;
        }
        component App() {
            return <div><Child a="a" b="b" pair={[1, 2]}/></div>;
        }
        export default App;
        "#,
    );
    let text = r.render_string();
    assert!(text.contains("<span>ab12</span>"), "got: {text}");
}

#[test]
fn top_level_destructuring_binds_module_names() {
    let (_, r) = setup(
        r#"
        const config = { a: 1, b: 2 };
        const { a, b: renamed } = config;
        const [x, y] = [10, 20];
        component App() {
            return <span>{a}{renamed}{x}{y}</span>;
        }
        export default App;
        "#,
    );
    let text = r.render_string();
    assert!(text.contains("<span>121020</span>"), "got: {text}");
}

#[test]
fn template_with_two_interpolations() {
    let out = logs(
        r#"component App() {
            const n = 3;
            const word = "items";
            console.log(`${n} ${word} left!`);
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["3 items left!"]);
}

#[test]
fn control_flow_inside_try_runs_finally_then_propagates() {
    // `return` inside `try` runs `finally` first, then completes the call.
    let out = logs(
        r#"
        function f(x) {
            try {
                if (x > 0) {
                    return "pos";
                }
                return "norn";
            } finally {
                console.log("fin");
            }
        }
        component App() {
            console.log(f(1));
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["fin", "pos"]);
}

#[test]
fn break_inside_try_propagates_to_loop() {
    let out = logs(
        r#"
        function f() {
            let i = 0;
            while (true) {
                try {
                    i = i + 1;
                    if (i >= 3) {
                        break;
                    }
                } finally {
                    console.log("it");
                }
            }
            return i;
        }
        component App() {
            console.log(f());
            return <div/>;
        }
        export default App;
        "#,
    );
    assert_eq!(out, vec!["it", "it", "it", "3"]);
}
