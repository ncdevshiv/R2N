# R2N — JavaScript Compatibility Scorecard

> **JS compatibility: 90%** of the test262-aligned subset (117 of 130 cases).
> Generated from `crates/r2n-runtime/tests/test262_subset.rs` and re-verified
> automatically by the `published_scorecard_matches_harness` test — if the
> engine's behavior or the case table changes without this document being
> updated, CI fails.

## Methodology

Upstream [test262](https://github.com/tc39/test262) files cannot run on the
R2N engine yet: they require general statement syntax (`var`, loops,
`switch`), `assert()`, template literals, and numeric literal forms the
component dialect does not have. The M2 conformance harness is therefore
**test262-aligned, not test262-derived**: 130 authored cases, each pinning ONE
ECMA-262 semantic in the R2N dialect, each carrying a reference to the ECMA
section it parallels. Observation is behavioral only (`console.log` output
compared exactly) — never API-presence.

- Cases marked **pass** pin ECMA-conformant behavior; they are hard regression
  gates.
- Cases marked as **known gaps** pin the engine's CURRENT non-conformant
  output, count as not-passing in the score, and are listed below with the
  reason. When one is fixed, the harness test fails until it is promoted and
  the score updated — the published number can never silently rot.
- Syntax the dialect does not have (single-quoted strings, template literals,
  `1e5` literals, `var`, loops, `switch`) is **out of scope**, listed at the
  bottom — not scored as failures, since a missing parser form is a language
  task, not a semantics bug.

## Score by category

| Category | Pass | Total | % |
|---|---|---|---|
| Values | 12 | 12 | 100% |
| ToNumber | 9 | 10 | 90% |
| NumToString | 9 | 11 | 82% |
| Strings | 10 | 11 | 91% |
| AbstractEq | 12 | 12 | 100% |
| StrictEq | 7 | 8 | 88% |
| Operators | 10 | 12 | 83% |
| Closures | 6 | 6 | 100% |
| Objects | 11 | 11 | 100% |
| Exceptions | 10 | 10 | 100% |
| Promises | 10 | 10 | 100% |
| Generators | 6 | 6 | 100% |
| Classes | 5 | 5 | 100% |
| Divergences | 0 | 6 | 0% |
| **Overall** | **117** | **130** | **90%** |

## Known gaps (13)

Each entry pins the engine's current output; the fix is a future task
prioritized by this list.

- **T262-TONUM-010** — `1 + BigInt(2)` yields `3`; ECMA raises TypeError when
  mixing BigInt and Number operands.
- **T262-NUMSTR-010** — numbers ≥ 1e21 print without an exponent
  (`1000000000000000000000`); JS switches to `1e+21`.
- **T262-NUMSTR-011** — numbers < 1e-6 print without an exponent
  (`0.0000001`); JS switches to `1e-7`.
- **T262-STR-011** — out-of-range string index yields `null`; JS yields
  `undefined`.
- **T262-SEQ-008** — function identity is broken: `f === f` is `false`
  because reading a function value clones its captured env, so the pointer
  comparison never matches. JS: `true`.
- **T262-OP-011** — `"5" - 1` errors ("non-number operand"); JS coerces via
  ToNumber and yields `4`. (`+` coerces correctly; `- * / %` do not.)
- **T262-OP-012** — `"5" < 10` errors ("incomparable operands"); JS coerces
  and yields `true`.
- **T262-DIV-001** — array display `[1, 2, 3]` vs JS `1,2,3` (Array::toString
  joins without spaces). Deliberate divergence, tracked since M2-T05.
- **T262-DIV-002** — structural Array/Map `===` (value-copy representation);
  JS compares by identity. Deliberate, tracked since M2-T05.
- **T262-DIV-003** — array display in string concat (`"" + [1, 2]` gives
  `[1, 2]`); JS gives `1,2`.
- **T262-DIV-004** — empty array is falsy; JS arrays are always truthy.
- **T262-DIV-005** — `NaN <= 1` and `NaN >= 1` are `true`; JS is `false` for
  every NaN comparison.
- **T262-DIV-006** — object ToPrimitive skipped in `+` (`1 + Object()` gives
  `NaN`); JS gives `1[object Object]`.

## Out of scope (not scored)

Language forms and builtins the dialect does not implement; each is a task,
not a semantics bug. Upstream test262 coverage becomes possible once these
land.

- Statements: `var`, `for`/`while`/`do` loops, `switch`, `break`/`continue`,
  labeled statements, top-level `if` (there is no general statement grammar —
  module-level declarations are `import|component|class|function*|export`).
- Lexical forms: single-quoted strings, template literals, hex/octal/binary/
  exponent numeric literals, BigInt literals (`10n`), `\u`/`\x` string
  escapes, comma operator, compound assignment (`+=`, `++`), `**`, bitwise
  ops, `??`, `in`/`instanceof`, `typeof x` as a keyword (only the
  `typeof(x)` call form exists).
- Builtins: `String.prototype` methods, `Array` methods beyond
  `map`/`filter`/iterators, `Object.keys/values/assign`, `Math`, `JSON`,
  `Number`/`String` namespaces, `parseInt`/`isNaN`, `RegExp`, `Date`,
  `Set`/`Map` (JS-level), `Error` subclasses, `Proxy`/`Reflect`.

## Re-verifying

```bash
cargo test --test test262_subset            # all cases + scorecard consistency
R2N_TRIAGE=1 cargo test --test test262_subset triage_dump -- --nocapture
                                            # actual-vs-expected per case
```
