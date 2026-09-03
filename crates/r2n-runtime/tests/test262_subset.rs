//! M2-T15 — test262-ALIGNED conformance harness + published compatibility score.
//!
//! Upstream test262 files cannot run here: they require `var`, loops,
//! `switch`, `assert()`, `includes()`, template literals, hex/exponent
//! numeric literals — the component dialect has none of those (statements are
//! exactly `let|const|return|expr`; strings are double-quote-only; numbers are
//! dec-int/float literals). So each case below pins ONE ECMA-262 semantic,
//! authored in the dialect, with a `ref` to the ECMA section it parallels.
//! Observation is behavioral only: `console.log` lines compared exactly after
//! one `flush()` (async jobs drain to fixpoint inside flush, so promise/await
//! cases observe their continuations).
//!
//! Scoring is honest, not all-green:
//! - `ecma_pass: true` cases pin ECMA-matching behavior — hard regression
//!   gates (a behavior change fails the category test).
//! - `ecma_pass: false` cases pin CURRENT divergent behavior (known gap or
//!   documented divergence). They still pin today's exact output, so engine
//!   drift anywhere fails CI; the score counts them as not-passing.
//! - `published_scorecard_matches_harness` recomputes per-category and overall
//!   fractions and asserts `docs/COMPATIBILITY.md` agrees — the published
//!   score cannot go stale (same enforcement philosophy as the README
//!   test-count check in scripts/verify-audit-claims.sh).
//!
//! Set `R2N_TRIAGE=1` to dump actual-vs-expected for every case (used when
//! pinning or re-pinning expectations).

use r2n_compiler::compile_source;
use r2n_runtime::Runtime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cat {
    Values,
    ToNumber,
    NumToString,
    Strings,
    AbstractEq,
    StrictEq,
    Operators,
    Closures,
    Objects,
    Exceptions,
    Promises,
    Generators,
    Classes,
    Divergences,
}

impl Cat {
    const ALL: [(Cat, &'static str); 14] = [
        (Cat::Values, "Values"),
        (Cat::ToNumber, "ToNumber"),
        (Cat::NumToString, "NumToString"),
        (Cat::Strings, "Strings"),
        (Cat::AbstractEq, "AbstractEq"),
        (Cat::StrictEq, "StrictEq"),
        (Cat::Operators, "Operators"),
        (Cat::Closures, "Closures"),
        (Cat::Objects, "Objects"),
        (Cat::Exceptions, "Exceptions"),
        (Cat::Promises, "Promises"),
        (Cat::Generators, "Generators"),
        (Cat::Classes, "Classes"),
        (Cat::Divergences, "Divergences"),
    ];

    fn name(&self) -> &'static str {
        Self::ALL.iter().find(|(c, _)| c == self).unwrap().1
    }
}

struct Case {
    id: &'static str,
    /// ECMA-262 section / test262 area this case parallels.
    ref_: &'static str,
    cat: Cat,
    src: String,
    /// Exact `console.log` lines the engine must produce today (all cases,
    /// gaps included — engine drift on a known gap is still a regression).
    expect: &'static [&'static str],
    /// Does the pinned behavior match ECMA-262? `false` = known gap or
    /// documented divergence; excluded from the published score and listed in
    /// docs/COMPATIBILITY.md with `note` as the reason.
    ecma_pass: bool,
    note: Option<&'static str>,
}

/// Standard program shell for a body snippet.
fn wrap(body: &str) -> String {
    format!("component App() {{\n{body}\n    return <div/>;\n}}\nexport default App;")
}

/// An ECMA-conformant case.
fn tc(
    id: &'static str,
    ref_: &'static str,
    cat: Cat,
    src: impl Into<String>,
    expect: &'static [&'static str],
) -> Case {
    Case {
        id,
        ref_,
        cat,
        src: src.into(),
        expect,
        ecma_pass: true,
        note: None,
    }
}

/// A known-gap or documented-divergence case: pins today's (non-ECMA) output,
/// counts as not-passing in the score.
fn div(
    id: &'static str,
    ref_: &'static str,
    cat: Cat,
    src: impl Into<String>,
    expect: &'static [&'static str],
    note: &'static str,
) -> Case {
    Case {
        id,
        ref_,
        cat,
        src: src.into(),
        expect,
        ecma_pass: false,
        note: Some(note),
    }
}

/// Observe a case: the program's `console.log` lines, or a single synthetic
/// `<error: ...>` line when flush fails (compile errors are out of scope —
/// every case in the table must compile).
fn observe(c: &Case) -> Vec<String> {
    let template = match compile_source(&c.src) {
        Ok(t) => t,
        Err(e) => return vec![format!("<compile error: {e}>")],
    };
    let mut rt = Runtime::new(template);
    match rt.flush() {
        Ok(_) => rt.logs().to_vec(),
        Err(e) => vec![format!("<error: {e}>")],
    }
}

// ---------------------------------------------------------------- cases ---

#[allow(clippy::vec_init_then_push)] // the table reads as a flat list of pushes
fn cases() -> Vec<Case> {
    use Cat::*;
    let mut v: Vec<Case> = Vec::new();

    // --- Values: ToBoolean (7.2.13) + typeof (12.5.6, call form) ----------
    v.push(tc(
        "T262-VALUES-001",
        "ECMA 7.2.13 ToBoolean",
        Values,
        wrap("    console.log(!undefined, !null, !false);"),
        &["true true true"],
    ));
    v.push(tc(
        "T262-VALUES-002",
        "ECMA 7.2.13 (+0/-0 falsy)",
        Values,
        wrap("    console.log(!0, !-0);"),
        &["true true"],
    ));
    v.push(tc(
        "T262-VALUES-003",
        "ECMA 7.2.13 (empty string, 0n falsy)",
        Values,
        wrap("    console.log(!\"\", !BigInt(0));"),
        &["true true"],
    ));
    v.push(tc(
        "T262-VALUES-004",
        "ECMA 7.2.13 (NaN falsy)",
        Values,
        wrap("    console.log(!(0 / 0));"),
        &["true"],
    ));
    v.push(tc(
        "T262-VALUES-005",
        "ECMA 7.2.13 (number/string truthy)",
        Values,
        wrap("    console.log(!!1, !!\"a\");"),
        &["true true"],
    ));
    v.push(tc(
        "T262-VALUES-006",
        "ECMA 12.5.6 (undefined/object)",
        Values,
        wrap("    console.log(typeof(undefined), typeof(null));"),
        &["undefined object"],
    ));
    v.push(tc(
        "T262-VALUES-007",
        "ECMA 12.5.6 (primitives)",
        Values,
        wrap("    console.log(typeof(1), typeof(\"a\"), typeof(true));"),
        &["number string boolean"],
    ));
    v.push(tc(
        "T262-VALUES-008",
        "ECMA 12.5.6 (NaN is number)",
        Values,
        wrap("    console.log(typeof(0 / 0));"),
        &["number"],
    ));
    v.push(tc(
        "T262-VALUES-009",
        "ECMA 12.5.6 (bigint/symbol)",
        Values,
        wrap("    console.log(typeof(BigInt(1)), typeof(Symbol(\"s\")));"),
        &["bigint symbol"],
    ));
    v.push(tc(
        "T262-VALUES-010",
        "ECMA 12.5.6 (object)",
        Values,
        wrap("    console.log(typeof(Object()));"),
        &["object"],
    ));
    v.push(tc(
        "T262-VALUES-011",
        "ECMA 12.5.6 (function)",
        Values,
        wrap("    let f = (x) => x;\n    console.log(typeof(f));"),
        &["function"],
    ));
    v.push(tc(
        "T262-VALUES-012",
        "ECMA 12.5.6 (typeof typeof)",
        Values,
        wrap("    console.log(typeof(typeof(1)));"),
        &["string"],
    ));

    // --- ToNumber (7.1.4) via `+`, which coerces non-string operands ------
    v.push(tc(
        "T262-TONUM-001",
        "ECMA 7.1.4 (null -> 0)",
        ToNumber,
        wrap("    console.log(1 + null);"),
        &["1"],
    ));
    v.push(tc(
        "T262-TONUM-002",
        "ECMA 7.1.4 (true -> 1)",
        ToNumber,
        wrap("    console.log(1 + true);"),
        &["2"],
    ));
    v.push(tc(
        "T262-TONUM-003",
        "ECMA 7.1.4 (false -> 0)",
        ToNumber,
        wrap("    console.log(1 + false);"),
        &["1"],
    ));
    v.push(tc(
        "T262-TONUM-004",
        "ECMA 7.1.4 (undefined -> NaN)",
        ToNumber,
        wrap("    console.log(1 + undefined);"),
        &["NaN"],
    ));
    v.push(tc(
        "T262-TONUM-005",
        "ECMA 6.1.9.1 (string concat wins)",
        ToNumber,
        wrap("    console.log(1 + \"\");"),
        &["1"],
    ));
    v.push(tc(
        "T262-TONUM-006",
        "ECMA 6.1.9.1 (\"\" -> \"0\")",
        ToNumber,
        wrap("    console.log(\"\" + 0);"),
        &["0"],
    ));
    v.push(tc(
        "T262-TONUM-007",
        "ECMA 7.1.4 (null + null)",
        ToNumber,
        wrap("    console.log(null + null);"),
        &["0"],
    ));
    v.push(tc(
        "T262-TONUM-008",
        "ECMA 7.1.4 (bool + bool)",
        ToNumber,
        wrap("    console.log(true + true);"),
        &["2"],
    ));
    v.push(tc(
        "T262-TONUM-009",
        "ECMA 6.1.9.1 (number wins over concat order)",
        ToNumber,
        wrap("    console.log(0 + \"7\");"),
        &["07"],
    ));
    v.push(div(
        "T262-TONUM-010",
        "ECMA 6.1.9.1 (BigInt + Number throws)",
        ToNumber,
        wrap("    try { console.log(1 + BigInt(2)); } catch (e) { console.log(\"TypeError\"); }"),
        &["3"],
        "Add coerces BigInt via ToNumber to 3; JS raises TypeError mixing BigInt and Number",
    ));

    // --- Number -> String (ECMA Number::toString vs format_number) --------
    v.push(tc(
        "T262-NUMSTR-001",
        "ECMA 6.1.6.1.20 (fraction)",
        NumToString,
        wrap("    console.log(0.5);"),
        &["0.5"],
    ));
    v.push(tc(
        "T262-NUMSTR-002",
        "ECMA 6.1.6.1.20 (mixed)",
        NumToString,
        wrap("    console.log(100.25);"),
        &["100.25"],
    ));
    v.push(tc(
        "T262-NUMSTR-003",
        "ECMA 6.1.6.1.20 (integer)",
        NumToString,
        wrap("    console.log(1000000);"),
        &["1000000"],
    ));
    v.push(tc(
        "T262-NUMSTR-004",
        "ECMA 6.1.6.1.20 (1e15, pre-exponent)",
        NumToString,
        wrap("    console.log(1000000000000000);"),
        &["1000000000000000"],
    ));
    v.push(tc(
        "T262-NUMSTR-005",
        "ECMA 6.1.6.1.20 (1e16, still plain)",
        NumToString,
        wrap("    console.log(10000000000000000);"),
        &["10000000000000000"],
    ));
    v.push(tc(
        "T262-NUMSTR-006",
        "ECMA 6.1.6.1.20 (NaN)",
        NumToString,
        wrap("    console.log(0 / 0);"),
        &["NaN"],
    ));
    v.push(tc(
        "T262-NUMSTR-007",
        "ECMA 6.1.6.1.20 (Infinity)",
        NumToString,
        wrap("    console.log(1 / 0);"),
        &["Infinity"],
    ));
    v.push(tc(
        "T262-NUMSTR-008",
        "ECMA 6.1.6.1.20 (-Infinity)",
        NumToString,
        wrap("    console.log(-1 / 0);"),
        &["-Infinity"],
    ));
    v.push(tc(
        "T262-NUMSTR-009",
        "ECMA 6.1.6.1.20 (shortest round-trip)",
        NumToString,
        wrap("    console.log(0.1 + 0.2);"),
        &["0.30000000000000004"],
    ));
    v.push(div(
        "T262-NUMSTR-010",
        "ECMA 6.1.6.1.20 (>=1e21 switches to exponent)",
        NumToString,
        wrap("    console.log(1000000000000000 * 1000000);"),
        &["1000000000000000000000"],
        "format_number has no 1e21 exponent switch; JS prints 1e+21",
    ));
    v.push(div(
        "T262-NUMSTR-011",
        "ECMA 6.1.6.1.20 (<1e-6 switches to exponent)",
        NumToString,
        wrap("    console.log(0.0000001);"),
        &["0.0000001"],
        "format_number has no small-exponent switch; JS prints 1e-7",
    ));

    // --- Strings (6.1.4) ---------------------------------------------------
    v.push(tc(
        "T262-STR-001",
        "ECMA 6.1.9.1 (concat)",
        Strings,
        wrap("    console.log(\"a\" + \"b\");"),
        &["ab"],
    ));
    v.push(tc(
        "T262-STR-002",
        "ECMA 6.1.5.1 (length)",
        Strings,
        wrap("    console.log(\"abc\".length);"),
        &["3"],
    ));
    v.push(tc(
        "T262-STR-003",
        "ECMA String index (in range)",
        Strings,
        wrap("    console.log(\"abc\"[0]);"),
        &["a"],
    ));
    v.push(tc(
        "T262-STR-004",
        "ECMA 6.1.9.1 (string + number)",
        Strings,
        wrap("    console.log(\"a\" + 1);"),
        &["a1"],
    ));
    v.push(tc(
        "T262-STR-005",
        "ECMA escape \\n counts as one unit",
        Strings,
        wrap("    console.log(\"a\\nb\".length);"),
        &["3"],
    ));
    v.push(tc(
        "T262-STR-006",
        "ECMA escape \\\\ counts as one unit",
        Strings,
        wrap("    console.log(\"a\\\\b\".length);"),
        &["3"],
    ));
    v.push(tc(
        "T262-STR-007",
        "ECMA 7.2.13 (string/string relational)",
        Strings,
        wrap("    console.log(\"abc\" < \"abd\");"),
        &["true"],
    ));
    v.push(tc(
        "T262-STR-008",
        "ECMA 6.1.9.1 (left-assoc concat)",
        Strings,
        wrap("    console.log(\"1\" + 1 + 1);"),
        &["111"],
    ));
    v.push(tc(
        "T262-STR-009",
        "ECMA 6.1.9.1 (number then concat)",
        Strings,
        wrap("    console.log(1 + 1 + \"1\");"),
        &["21"],
    ));
    v.push(tc(
        "T262-STR-010",
        "ECMA 6.1.4 (UTF-16 length)",
        Strings,
        wrap("    console.log(\"héllo\".length);"),
        &["5"],
    ));
    v.push(div(
        "T262-STR-011",
        "ECMA String index (out of range -> undefined)",
        Strings,
        wrap("    console.log(\"abc\"[5]);"),
        &["null"],
        "out-of-range string index yields null; JS yields undefined",
    ));

    // --- Abstract equality (7.2.14) ----------------------------------------
    v.push(tc(
        "T262-EQ-001",
        "ECMA 7.2.14 (null/undefined pair)",
        AbstractEq,
        wrap("    console.log(null == undefined);"),
        &["true"],
    ));
    v.push(tc(
        "T262-EQ-002",
        "ECMA 7.2.14 (null never coerces)",
        AbstractEq,
        wrap("    console.log(null == 0, null == \"\", null == false);"),
        &["false false false"],
    ));
    v.push(tc(
        "T262-EQ-003",
        "ECMA 7.2.14 (undefined never coerces)",
        AbstractEq,
        wrap("    console.log(undefined == 0, undefined == \"\");"),
        &["false false"],
    ));
    v.push(tc(
        "T262-EQ-004",
        "ECMA 7.2.14 (number<->string)",
        AbstractEq,
        wrap("    console.log(\"5\" == 5);"),
        &["true"],
    ));
    v.push(tc(
        "T262-EQ-005",
        "ECMA 7.2.14 (number<->string, other order)",
        AbstractEq,
        wrap("    console.log(5 == \"5\");"),
        &["true"],
    ));
    v.push(tc(
        "T262-EQ-006",
        "ECMA 7.2.14 (boolean coerces first)",
        AbstractEq,
        wrap("    console.log(true == 1);"),
        &["true"],
    ));
    v.push(tc(
        "T262-EQ-007",
        "ECMA 7.2.14 (bool -> number -> string)",
        AbstractEq,
        wrap("    console.log(true == \"1\");"),
        &["true"],
    ));
    v.push(tc(
        "T262-EQ-008",
        "ECMA 7.2.14 (false == 0)",
        AbstractEq,
        wrap("    console.log(false == 0);"),
        &["true"],
    ));
    v.push(tc(
        "T262-EQ-009",
        "ECMA 7.2.14 (bool vs other numbers)",
        AbstractEq,
        wrap("    console.log(true == 2);"),
        &["false"],
    ));
    v.push(tc(
        "T262-EQ-010",
        "ECMA 7.2.14 (NaN never equals)",
        AbstractEq,
        wrap("    let nan = 0 / 0;\n    console.log(nan == nan);"),
        &["false"],
    ));
    v.push(tc(
        "T262-EQ-011",
        "ECMA 7.2.14 (BigInt mathematical)",
        AbstractEq,
        wrap("    console.log(BigInt(5) == 5, BigInt(5) == \"5\");"),
        &["true true"],
    ));
    v.push(tc("T262-EQ-012", "ECMA 7.2.14 (methodless object raises TypeError)", AbstractEq, wrap(
        "    let o = Object();\n    try { console.log(o == 1); } catch (e) { console.log(\"TypeError:\", e); }"),
        &["TypeError: Cannot convert object to primitive value (no callable valueOf/toString)"]));

    // --- Strict equality (7.2.15) -------------------------------------------
    v.push(tc(
        "T262-SEQ-001",
        "ECMA 7.2.15 (same number)",
        StrictEq,
        wrap("    console.log(1 === 1);"),
        &["true"],
    ));
    v.push(tc(
        "T262-SEQ-002",
        "ECMA 7.2.15 (no coercion)",
        StrictEq,
        wrap("    console.log(1 === \"1\");"),
        &["false"],
    ));
    v.push(tc(
        "T262-SEQ-003",
        "ECMA 7.2.15 (NaN !== NaN)",
        StrictEq,
        wrap("    let nan = 0 / 0;\n    console.log(nan === nan);"),
        &["false"],
    ));
    v.push(tc(
        "T262-SEQ-004",
        "ECMA 7.2.15 (+0 === -0)",
        StrictEq,
        wrap("    console.log(0 === -0);"),
        &["true"],
    ));
    v.push(tc(
        "T262-SEQ-005",
        "ECMA 7.2.15 (nullish)",
        StrictEq,
        wrap("    console.log(null === null, undefined === undefined, null === undefined);"),
        &["true true false"],
    ));
    v.push(tc(
        "T262-SEQ-006",
        "ECMA 7.2.15 (!== negation)",
        StrictEq,
        wrap("    console.log(1 !== \"1\");"),
        &["true"],
    ));
    v.push(tc("T262-SEQ-007", "ECMA 7.2.15 (object identity)", StrictEq, wrap(
        "    let a = Object();\n    let b = a;\n    let c = Object();\n    console.log(a === b, a === c);"),
        &["true false"]));
    v.push(div("T262-SEQ-008", "ECMA 7.2.15 (function identity)", StrictEq, wrap(
        "    let f = (x) => x;\n    let g = (x) => x;\n    console.log(f === g, f === f);"),
        &["false false"],
        "function equality compares the captured-env pointer, but reading a function value clones its captured Env — so even f === f is false; JS: true"));

    // --- Operators -----------------------------------------------------------
    v.push(tc(
        "T262-OP-001",
        "ECMA 12.7.3 (% fmod)",
        Operators,
        wrap("    console.log(7 % 3, -7 % 3);"),
        &["1 -1"],
    ));
    v.push(tc(
        "T262-OP-002",
        "ECMA 12.8 (precedence)",
        Operators,
        wrap("    console.log(2 + 3 * 4, (2 + 3) * 4);"),
        &["14 20"],
    ));
    v.push(tc(
        "T262-OP-003",
        "ECMA 12.9 (relational, number/number)",
        Operators,
        wrap("    console.log(1 < 2, 2 <= 2, 3 > 4, 4 >= 5);"),
        &["true true false false"],
    ));
    v.push(tc(
        "T262-OP-004",
        "ECMA 12.13 (logical)",
        Operators,
        wrap("    console.log(true && false, true || false);"),
        &["false true"],
    ));
    v.push(tc(
        "T262-OP-005",
        "ECMA 12.13 (&& returns operand)",
        Operators,
        wrap("    console.log(1 && 2);"),
        &["2"],
    ));
    v.push(tc(
        "T262-OP-006",
        "ECMA 12.13 (|| short-circuit)",
        Operators,
        wrap("    console.log(0 || \"fallback\");"),
        &["fallback"],
    ));
    v.push(tc(
        "T262-OP-007",
        "ECMA 12.13 (empty string falsy)",
        Operators,
        wrap("    console.log(\"\" || \"x\");"),
        &["x"],
    ));
    v.push(tc(
        "T262-OP-008",
        "ECMA 13.15 (assignment)",
        Operators,
        wrap("    let x = 5;\n    x = x + 1;\n    console.log(x);"),
        &["6"],
    ));
    v.push(tc(
        "T262-OP-009",
        "ECMA 13.14 (conditional)",
        Operators,
        wrap("    console.log(1 < 2 ? \"y\" : \"n\");"),
        &["y"],
    ));
    v.push(tc(
        "T262-OP-010",
        "ECMA 12.7.3 (division yields fraction)",
        Operators,
        wrap("    console.log(10 / 4);"),
        &["2.5"],
    ));
    v.push(div(
        "T262-OP-011",
        "ECMA 12.7.3 (- coerces via ToNumber)",
        Operators,
        wrap("    try { console.log(\"5\" - 1); } catch (e) { console.log(\"caught:\", e); }"),
        &["caught: non-number operand"],
        "Sub/Mul/Div/Mod require strictly-number operands (no ToNumber coercion); JS yields 4",
    ));
    v.push(div(
        "T262-OP-012",
        "ECMA 12.9 (< coerces number<->string)",
        Operators,
        wrap("    try { console.log(\"5\" < 10); } catch (e) { console.log(\"caught:\", e); }"),
        &["caught: incomparable operands"],
        "relational ops compare same-kind only; JS coerces to number and yields true",
    ));

    // --- Closures & scope -----------------------------------------------------
    v.push(tc(
        "T262-CLO-001",
        "ECMA 9.4.1 (live capture)",
        Closures,
        wrap("    let n = 1;\n    let get = () => n;\n    n = 2;\n    console.log(get());"),
        &["2"],
    ));
    v.push(tc("T262-CLO-002", "ECMA 9.4.1 (counter state in closure env)", Closures, wrap(
        "    let makeCounter = () => { let n = 0; return () => { n = n + 1; return n; }; };\n    let c = makeCounter();\n    c();\n    c();\n    console.log(c());"),
        &["3"]));
    v.push(tc("T262-CLO-003", "ECMA 9.4.1 (instances independent)", Closures, wrap(
        "    let makeCounter = () => { let n = 0; return () => { n = n + 1; return n; }; };\n    let a = makeCounter();\n    let b = makeCounter();\n    a();\n    a();\n    let av = a();\n    let bv = b();\n    console.log(av, bv);"),
        &["3 1"]));
    v.push(tc(
        "T262-CLO-004",
        "ECMA 9.4.1 (nested closures chain)",
        Closures,
        wrap("    let outer = (x) => (y) => (z) => x + y + z;\n    console.log(outer(1)(2)(3));"),
        &["6"],
    ));
    v.push(tc(
        "T262-CLO-005",
        "ECMA 9.2 (param shadows outer)",
        Closures,
        wrap("    let x = 1;\n    let f = (x) => x + 10;\n    console.log(f(5), x);"),
        &["15 1"],
    ));
    v.push(tc(
        "T262-CLO-006",
        "ECMA 15.1 (let-rec via env lookup)",
        Closures,
        wrap("    let fact = (n) => n <= 1 ? 1 : n * fact(n - 1);\n    console.log(fact(5));"),
        &["120"],
    ));

    // --- Objects & prototypes ---------------------------------------------------
    v.push(tc(
        "T262-OBJ-001",
        "ECMA 10.1 (own props, missing -> undefined)",
        Objects,
        wrap("    let o = Object();\n    o.x = 1;\n    console.log(o.x, o.y);"),
        &["1 undefined"],
    ));
    v.push(tc("T262-OBJ-002", "ECMA 10.1 (reference semantics)", Objects, wrap(
        "    let o = Object();\n    o.x = 1;\n    let p = o;\n    p.x = 2;\n    console.log(o.x);"),
        &["2"]));
    v.push(tc("T262-OBJ-003", "ECMA 10.1 (prototype chain read)", Objects, wrap(
        "    let base = Object();\n    base.greet = () => \"hi\";\n    let child = Object.create(base);\n    console.log(child.greet());"),
        &["hi"]));
    v.push(tc("T262-OBJ-004", "ECMA 10.1 (own prop shadows proto)", Objects, wrap(
        "    let base = Object();\n    base.x = 1;\n    let child = Object.create(base);\n    child.x = 2;\n    console.log(child.x, base.x);"),
        &["2 1"]));
    v.push(tc("T262-OBJ-005", "ECMA 19.1.2.2 (getPrototypeOf identity)", Objects, wrap(
        "    let base = Object();\n    let child = Object.create(base);\n    console.log(Object.getPrototypeOf(child) === base);"),
        &["true"]));
    v.push(tc("T262-OBJ-006", "ECMA B.2.2 (__proto__ read)", Objects, wrap(
        "    let base = Object();\n    let child = Object.create(base);\n    console.log(child.__proto__ === base);"),
        &["true"]));
    v.push(tc("T262-OBJ-007", "ECMA B.2.2 (__proto__ write)", Objects, wrap(
        "    let a = Object();\n    let c = Object();\n    c.__proto__ = a;\n    console.log(c.inherited === undefined);\n    a.inherited = 7;\n    console.log(c.inherited);"),
        &["true", "7"]));
    v.push(tc(
        "T262-OBJ-008",
        "ECMA 19.1.2.2 (null proto)",
        Objects,
        wrap("    let o = Object.create(null);\n    console.log(o.anything);"),
        &["undefined"],
    ));
    v.push(tc(
        "T262-OBJ-009",
        "ECMA 10.1 (per-object isolation)",
        Objects,
        wrap("    let a = Object();\n    a.x = 1;\n    let b = Object();\n    console.log(b.x);"),
        &["undefined"],
    ));
    v.push(tc(
        "T262-OBJ-010",
        "ECMA 7.3 (array length + index)",
        Objects,
        wrap("    console.log([1, 2, 3].length, [1, 2, 3][1]);"),
        &["3 2"],
    ));
    v.push(tc("T262-OBJ-011", "ECMA 19.1.3 (method receiver binding)", Objects, wrap(
        "    let o = Object();\n    o.n = 5;\n    o.get = () => this.n;\n    console.log(o.get());"),
        &["5"]));

    // --- Exceptions -----------------------------------------------------------
    v.push(tc(
        "T262-EX-001",
        "ECMA 13.15 (catch binds thrown string)",
        Exceptions,
        wrap("    try { throw \"boom\"; } catch (e) { console.log(e); }"),
        &["boom"],
    ));
    v.push(tc(
        "T262-EX-002",
        "ECMA 13.15 (any value throwable)",
        Exceptions,
        wrap("    try { throw 42; } catch (e) { console.log(e); }"),
        &["42"],
    ));
    v.push(tc(
        "T262-EX-003",
        "ECMA 13.15.7 (finally on success)",
        Exceptions,
        wrap("    let x = 0;\n    try { x = 1; } finally { x = x + 10; }\n    console.log(x);"),
        &["11"],
    ));
    v.push(tc("T262-EX-004", "ECMA 13.15.7 (finally after catch)", Exceptions, wrap(
        "    try { throw \"e\"; } catch (e) { console.log(\"caught\"); } finally { console.log(\"fin\"); }"),
        &["caught", "fin"]));
    v.push(tc("T262-EX-005", "ECMA 13.15.7 (finally error replaces outcome)", Exceptions, wrap(
        "    let r = \"\";\n    try { try { throw \"a\"; } finally { throw \"b\"; } } catch (e) { r = e; }\n    console.log(r);"),
        &["b"]));
    v.push(tc("T262-EX-006", "ECMA 13.15 (propagate across calls)", Exceptions, wrap(
        "    let f = () => { throw \"deep\"; };\n    try { f(); } catch (e) { console.log(e); }"),
        &["deep"]));
    v.push(tc("T262-EX-007", "ECMA 13.15 (rethrow transforms)", Exceptions, wrap(
        "    try { try { throw \"x\"; } catch (e) { throw e + \"!\"; } } catch (e2) { console.log(e2); }"),
        &["x!"]));
    v.push(tc(
        "T262-EX-008",
        "ECMA 13.15.1 (optional catch binding)",
        Exceptions,
        wrap("    try { throw \"z\"; } catch { console.log(\"no binding\"); }"),
        &["no binding"],
    ));
    v.push(tc(
        "T262-EX-009",
        "ECMA 8.7 (unbound identifier throws, catchable)",
        Exceptions,
        wrap("    try { nope; } catch (e) { console.log(typeof(e)); }"),
        &["string"],
    ));
    v.push(tc(
        "T262-EX-010",
        "ECMA 19.5 (Error shape)",
        Exceptions,
        wrap("    try { throw new Error(\"bad\"); } catch (e) { console.log(e.name, e.message); }"),
        &["Error bad"],
    ));

    // --- Promises & async (25.4 / 27.7) ----------------------------------------
    v.push(tc(
        "T262-PR-001",
        "ECMA 25.4.3.1 (executor + then)",
        Promises,
        wrap("    new Promise((resolve) => { resolve(\"v\"); }).then((x) => { console.log(x); });"),
        &["v"],
    ));
    v.push(tc(
        "T262-PR-002",
        "ECMA 25.4.5.3 (catch on reject)",
        Promises,
        wrap("    Promise.reject(\"no\").catch((e) => { console.log(e); });"),
        &["no"],
    ));
    v.push(tc(
        "T262-PR-003",
        "ECMA 25.4.5.3 (chaining transforms)",
        Promises,
        wrap("    Promise.resolve(1).then((x) => x + 1).then((x) => { console.log(x); });"),
        &["2"],
    ));
    v.push(tc(
        "T262-PR-004",
        "ECMA 25.4.5.3.3 (pass-through identity)",
        Promises,
        wrap("    Promise.resolve(9).then().then((x) => { console.log(x); });"),
        &["9"],
    ));
    v.push(tc("T262-PR-005", "ECMA 25.4.5.3.3 (returned promise adopts)", Promises, wrap(
        "    Promise.resolve(1).then((x) => Promise.resolve(x + 10)).then((x) => { console.log(x); });"),
        &["11"]));
    v.push(tc(
        "T262-PR-006",
        "ECMA 25.4.5.3 (rejection falls through then)",
        Promises,
        wrap("    Promise.reject(\"e1\").then((x) => x).catch((e) => { console.log(e); });"),
        &["e1"],
    ));
    v.push(tc("T262-PR-007", "ECMA 25.4 (settle is idempotent)", Promises, wrap(
        "    new Promise((resolve, reject) => { resolve(\"ok\"); reject(\"bad\"); }).then((v) => { console.log(v); });"),
        &["ok"]));
    v.push(tc("T262-PR-008", "ECMA 27.7 (await binds value)", Promises, wrap(
        "    let p = Promise.resolve(\"done\");\n    let f = async () => { let v = await p; console.log(v); };\n    f();"),
        &["done"]));
    v.push(tc("T262-PR-009", "ECMA 27.7 (await rejection catchable)", Promises, wrap(
        "    let f = async () => { let v = await Promise.reject(\"nope\"); console.log(\"unreached\"); };\n    f().catch((e) => { console.log(e); });"),
        &["nope"]));
    v.push(tc("T262-PR-010", "ECMA 27.7 (sequential awaits)", Promises, wrap(
        "    let f = async () => { let a = await Promise.resolve(1); let b = await Promise.resolve(a + 1); console.log(b); };\n    f();"),
        &["2"]));

    // --- Generators & iterators (27.5 / 25.1) -----------------------------------
    v.push(tc("T262-GEN-001", "ECMA 27.5 (pull protocol + return done)", Generators,
        "function* g() {\n    yield 1;\n    yield 2;\n    return 3;\n}\ncomponent App() {\n    let it = g();\n    console.log(it.next().value, it.next().value, it.next().value, it.next().done);\n    return <div/>;\n}\nexport default App;",
        &["1 2 3 true"]));
    v.push(tc("T262-GEN-002", "ECMA 27.5.3 (next(arg) injects)", Generators,
        "function* echo() {\n    let x = yield \"ready\";\n    yield x;\n}\ncomponent App() {\n    let it = echo();\n    console.log(it.next().value);\n    console.log(it.next(\"sent\").value);\n    return <div/>;\n}\nexport default App;",
        &["ready", "sent"]));
    v.push(tc("T262-GEN-003", "ECMA 27.5.3 (lazy until first next)", Generators,
        "function* g() { console.log(\"ran\"); yield 1; }\ncomponent App() {\n    let it = g();\n    console.log(\"created\");\n    it.next();\n    return <div/>;\n}\nexport default App;",
        &["created", "ran"]));
    v.push(tc("T262-GEN-004", "ECMA 27.5.3 (state persists across nexts)", Generators,
        "function* g() {\n    let n = 0;\n    n = n + 1;\n    yield n;\n    n = n + 1;\n    yield n;\n}\ncomponent App() {\n    let it = g();\n    console.log(it.next().value, it.next().value);\n    return <div/>;\n}\nexport default App;",
        &["1 2"]));
    v.push(tc("T262-GEN-005", "ECMA 27.5.3 (return early-completes)", Generators,
        "function* g() { yield 1; }\ncomponent App() {\n    let it = g();\n    it.next();\n    console.log(it.return(9).value, it.next().done);\n    return <div/>;\n}\nexport default App;",
        &["9 true"]));
    v.push(tc("T262-GEN-006", "ECMA 25.1 (array values iterator)", Generators, wrap(
        "    let it = [10, 20].values();\n    console.log(it.next().value, it.next().value, it.next().done);"),
        &["10 20 true"]));

    // --- Classes, this, new (14.3) ----------------------------------------------
    v.push(tc("T262-CLS-001", "ECMA 14.3 (constructor + prototype method)", Classes,
        "class Point extends Object {\n    constructor(x, y) {\n        this.x = x;\n        this.y = y;\n    }\n    sum() { return this.x + this.y; }\n}\ncomponent App() {\n    let p = new Point(3, 4);\n    console.log(p.sum());\n    return <div/>;\n}\nexport default App;",
        &["7"]));
    v.push(tc("T262-CLS-002", "ECMA 14.3 (instances independent)", Classes,
        "class Counter extends Object {\n    constructor() { this.n = 0; }\n    inc() { this.n = this.n + 1; return this.n; }\n}\ncomponent App() {\n    let a = new Counter();\n    let b = new Counter();\n    a.inc();\n    a.inc();\n    b.inc();\n    console.log(a.inc(), b.n);\n    return <div/>;\n}\nexport default App;",
        &["3 1"]));
    v.push(tc(
        "T262-CLS-003",
        "ECMA 16 (strict this outside member call)",
        Classes,
        wrap("    console.log(this);"),
        &["undefined"],
    ));
    v.push(tc("T262-CLS-004", "ECMA 14.3 (methods live on the prototype)", Classes,
        "class P extends Object {\n    constructor() { this.x = 1; }\n    get() { return this.x; }\n}\ncomponent App() {\n    let p = new P();\n    let m = Object.getPrototypeOf(p).get;\n    console.log(typeof(m), p.get());\n    return <div/>;\n}\nexport default App;",
        &["function 1"]));
    v.push(tc("T262-CLS-005", "ECMA 14.3 (method mutates instance across calls)", Classes,
        "class Acc extends Object {\n    constructor() { this.total = 0; }\n    add(n) { this.total = this.total + n; return this.total; }\n}\ncomponent App() {\n    let a = new Acc();\n    a.add(2);\n    a.add(30);\n    console.log(a.total);\n    return <div/>;\n}\nexport default App;",
        &["32"]));

    // --- Documented divergences (pinned CURRENT behavior; score = 0 here) ------
    v.push(div(
        "T262-DIV-001",
        "ECMA 7.1.17 (Array::toString joins without spaces)",
        Divergences,
        wrap("    console.log([1, 2, 3]);"),
        &["[1, 2, 3]"],
        "display() renders [1, 2, 3]; JS String([1,2,3]) is 1,2,3",
    ));
    v.push(div(
        "T262-DIV-002",
        "ECMA 7.2.15 (arrays compare by identity)",
        Divergences,
        wrap("    console.log([1] === [1]);"),
        &["true"],
        "Array/Map compare structurally (value-copy representation); JS false",
    ));
    v.push(div(
        "T262-DIV-003",
        "ECMA 7.1.17 (array to string in concat)",
        Divergences,
        wrap("    console.log(\"\" + [1, 2]);"),
        &["[1, 2]"],
        "concat uses display(); JS 1,2",
    ));
    v.push(div(
        "T262-DIV-004",
        "ECMA 7.2.13 ([] is truthy)",
        Divergences,
        wrap("    console.log([] ? \"t\" : \"f\");"),
        &["f"],
        "arrays truthy iff non-empty; JS always truthy",
    ));
    v.push(div(
        "T262-DIV-005",
        "ECMA 7.2.13 (NaN comparisons are false)",
        Divergences,
        wrap("    let nan = 0 / 0;\n    console.log(nan <= 1, nan >= 1);"),
        &["true true"],
        "partial_cmp fallback treats NaN as Equal; JS false for both",
    ));
    v.push(div(
        "T262-DIV-006",
        "ECMA 7.1.1 (object ToPrimitive in +)",
        Divergences,
        wrap("    console.log(1 + Object());"),
        &["NaN"],
        "Add falls back to ToNumber for objects; JS 1[object Object]",
    ));

    v
}

// ---------------------------------------------------------------- runner ---

fn check_case(c: &Case) -> Result<(), String> {
    let actual = observe(c);
    if actual == c.expect {
        Ok(())
    } else {
        Err(format!(
            "{} [{}] expected {expect:?}, got {actual:?}{}",
            c.id,
            c.ref_,
            c.note.map(|n| format!(" — note: {n}")).unwrap_or_default(),
            expect = c.expect,
        ))
    }
}

fn cases_in(cat: Cat) -> Vec<&'static Case> {
    // Leaked per-process: the table is immutable after construction.
    Box::leak(Box::new(cases()))
        .iter()
        .filter(|c| c.cat == cat)
        .collect()
}

fn assert_category(cat: Cat) {
    let cs = cases_in(cat);
    assert!(!cs.is_empty(), "category {} has no cases", cat.name());
    let mut failures = Vec::new();
    for c in cs {
        if let Err(e) = check_case(c) {
            failures.push(e);
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn values_truthiness_and_typeof() {
    assert_category(Cat::Values);
}
#[test]
fn tonumber_coercion() {
    assert_category(Cat::ToNumber);
}
#[test]
fn number_to_string() {
    assert_category(Cat::NumToString);
}
#[test]
fn string_semantics() {
    assert_category(Cat::Strings);
}
#[test]
fn abstract_equality_ladder() {
    assert_category(Cat::AbstractEq);
}
#[test]
fn strict_equality() {
    assert_category(Cat::StrictEq);
}
#[test]
fn operators() {
    assert_category(Cat::Operators);
}
#[test]
fn closures_and_scope() {
    assert_category(Cat::Closures);
}
#[test]
fn objects_and_prototypes() {
    assert_category(Cat::Objects);
}
#[test]
fn exceptions() {
    assert_category(Cat::Exceptions);
}
#[test]
fn promises_and_async() {
    assert_category(Cat::Promises);
}
#[test]
fn generators_and_iterators() {
    assert_category(Cat::Generators);
}
#[test]
fn classes_this_new() {
    assert_category(Cat::Classes);
}
#[test]
fn documented_divergences_stable() {
    assert_category(Cat::Divergences);
}

/// The published score in docs/COMPATIBILITY.md must equal what the harness
/// actually computes: per-category pass/total/%, the overall row, and the
/// Known-gaps list (ecma_pass=false case ids). Run the suite whenever engine
/// behavior changes; a mismatch here means the published score is stale.
#[test]
fn published_scorecard_matches_harness() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/COMPATIBILITY.md");
    let doc = std::fs::read_to_string(path).expect("read docs/COMPATIBILITY.md");

    let cs = Box::leak(Box::new(cases()));
    let mut computed: Vec<(Cat, usize, usize)> = Vec::new(); // (cat, ecma_pass_count, total)
    let mut gaps: Vec<&str> = Vec::new();
    for (cat, _) in Cat::ALL {
        let in_cat: Vec<&Case> = cs.iter().filter(|c| c.cat == cat).collect();
        let pass = in_cat.iter().filter(|c| c.ecma_pass).count();
        computed.push((cat, pass, in_cat.len()));
        for c in &in_cat {
            if !c.ecma_pass {
                gaps.push(c.id);
            }
        }
    }
    let total: usize = computed.iter().map(|(_, _, t)| t).sum();
    let passed: usize = computed.iter().map(|(_, p, _)| p).sum();
    let pct = |p: usize, t: usize| format!("{}%", (p * 100 + t / 2) / t);

    // `R2N_SCORECARD=1` prints the computed table in scorecard format (paste
    // into docs/COMPATIBILITY.md when the numbers legitimately change).
    if std::env::var("R2N_SCORECARD").is_ok() {
        for (cat, pass, total_n) in &computed {
            println!(
                "| {} | {} | {} | {} |",
                cat.name(),
                pass,
                total_n,
                pct(*pass, *total_n)
            );
        }
        println!(
            "| **Overall** | **{}** | **{}** | **{}** |",
            passed,
            total,
            pct(passed, total)
        );
        println!("gaps: {:?}", gaps);
    }

    // Parse score rows: `| <name> | <pass> | <total> | <pct>% |`
    let mut rows: Vec<(String, usize, usize, String)> = Vec::new();
    for line in doc.lines() {
        let line = line.trim();
        if !line.starts_with("| ") || line.contains("---") || line.starts_with("| Category") {
            continue;
        }
        let cells: Vec<String> = line
            .trim_matches('|')
            .split('|')
            .map(|s| s.trim().trim_matches('*').to_string())
            .collect();
        if cells.len() != 4 {
            continue;
        }
        match (cells[1].parse::<usize>(), cells[2].parse::<usize>()) {
            (Ok(p), Ok(t)) => rows.push((cells[0].clone(), p, t, cells[3].clone())),
            _ => continue, // not a score row (e.g. prose cells)
        }
    }
    assert!(
        rows.len() == Cat::ALL.len() + 1,
        "scorecard rows: {}",
        rows.len()
    );

    for (cat, pass, total_n) in &computed {
        let row = rows
            .iter()
            .find(|(n, _, _, _)| n == cat.name())
            .unwrap_or_else(|| panic!("no scorecard row for {}", cat.name()));
        assert_eq!(&row.1, pass, "{} pass", cat.name());
        assert_eq!(&row.2, total_n, "{} total", cat.name());
        assert_eq!(row.3, pct(*pass, *total_n), "{} pct", cat.name());
    }
    let overall = rows.last().unwrap();
    assert_eq!(overall.0, "Overall");
    assert_eq!(overall.1, passed, "overall pass");
    assert_eq!(overall.2, total, "overall total");
    assert_eq!(overall.3, pct(passed, total), "overall pct");

    // Known-gaps bullets: `- **T262-...**` must exactly match ecma_pass=false ids.
    let mut published_gaps: Vec<&str> = Vec::new();
    for line in doc.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("- **") {
            if let Some(id) = rest.split("**").next() {
                if id.starts_with("T262-") {
                    published_gaps.push(Box::leak(id.to_string().into_boxed_str()));
                }
            }
        }
    }
    gaps.sort_unstable();
    published_gaps.sort_unstable();
    assert_eq!(
        published_gaps, gaps,
        "published known-gaps must equal ecma_pass=false case ids"
    );
}

/// Dev tool: `R2N_TRIAGE=1 cargo test --test test262_subset triage_dump -- --nocapture`
/// prints actual-vs-expected for every case (used when pinning expectations).
#[test]
fn triage_dump() {
    if std::env::var("R2N_TRIAGE").is_err() {
        return;
    }
    for c in cases() {
        let actual = observe(&c);
        let status = if actual == c.expect { "OK " } else { "DIFF" };
        println!(
            "{} {} [{}] ecma_pass={}\n  expect: {:?}\n  actual: {:?}",
            status, c.id, c.ref_, c.ecma_pass, c.expect, actual
        );
    }
}
