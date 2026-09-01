# R2N Audit Report — 2026-08-30

> **⚠️ Superseded for live status.** This is the original 2026-08-30 audit
> snapshot. The current baseline is **62/106 tasks, 251 tests green**, maintained
> live in `roadmap/CHECKLIST.md` and re-verified automatically by
> `scripts/verify-audit-claims.sh` on every PR. Use those for current numbers;
> this document records the findings of the v0.1.0 audit.

**Method:** every source file (~4,600 lines across 7 crates) was re-read and
every roadmap claim re-verified against the actual implementation and test
suite. Nothing was trusted from prior sessions. `scripts/verify-audit-claims.sh`
now automates the record-vs-code checks in CI.

**Result:** 32/106 roadmap tasks genuinely complete; records were brought
into agreement with the code (previously the checklist claimed 5/106, which
understated completed work, while the repo lacked any enforcement that claims
stay true).

## What was verified

| Area | Evidence |
|---|---|
| Lexer | hand-written, byte-offset + line:col tracking, JSX-text rescanning, line/block comments — `r2n-parser/src/lexer.rs` |
| Parser | precedence climbing, ternary/if-else, arrays, member/call/index, arrows (expr + block body), JSX with raw-text children — `r2n-parser/src/parser.rs`, tests in `r2n-parser` |
| IR lowering | AST → JsExpr → ReactNode → RuntimeComponent/RuntimeTemplate, free-variable captures, keyed List nodes, JSON round-trip — `r2n-ir`, tests in `crates/r2n-ir/tests/lower.rs` |
| Zero-JS eval | closed value set (no eval, no JS engine), UTF-16 strings, ECMA truthiness/format — `r2n-runtime/src/eval.rs`, `value.rs` |
| Hooks | per-instance `HookFrame` slots, dirty-flag re-render, `useEffect` mount/deps semantics — `r2n-runtime/src/hooks.rs`, events tests |
| Reconciliation | keyed (type, key, path) diff; minimal patches (1 SetText per click); list append without removals — `r2n-runtime/src/engine.rs`, engine + events tests |
| Events | `Value::Handler` (serializable: instance path + closure), registration during diff, `Runtime::dispatch` runs handler + flushes — events tests (5) |
| Boundaries | runtime has no parser/AST/compiler dep — `tests/architecture.rs` + CI static check |

## Issues found and fixed during this audit

1. `r2n-runtime` carried an unused `r2n-ast` dependency — removed; boundary now CI-enforced.
2. No event system existed; the reactive loop was driven by a test backdoor (`bump_first_state`) that mutated state directly — replaced by the real handler/dispatch path; the backdoor is deleted.
3. Frame-identity bug: nested elements evaluated against orphan per-node frames; node paths and component-instance paths were conflated — now two separately-threaded paths.
4. Parser gaps: JSX raw-text children, block-bodied arrows, expression statements — implemented (incl. token byte-offsets for JSX text rescanning).
5. `useEffect` deps evaluated before the bindings they referenced — statements now lower in source order.
6. `key` prop leaked into rendered output; handler props would have rendered — both stripped.
7. Dead code removed (`Value::Component`, `Builtin`, `JsExpr::Builtin`, `Setter.generation`, `next_node_id`, `roundtrip`, `apply_all`, unused `thiserror`).

## Standing rules (now machine-enforced)

- A milestone closes only when acceptance tests pass — never because an API exists.
- Records (`CHECKLIST.md`, `roadmap.yaml`, `roadmap.toml`, `ROADMAP.md`, README) must match code; CI fails on drift.
- The runtime never sees source; renderers see only patches; the artifact is language-neutral data.
- No stubs: CI greps for `todo!` / `unimplemented!` / placeholder markers in shipped code.
