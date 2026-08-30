# R2N Development Journal

> This journal is the only place plan/goal/decisions are recorded. Format per entry:
> **PLAN** what we intended · **GOAL** the target · **DID** what was actually built · **WHY**
> **OPTIONS** alternatives considered · **CHOSE** the decision and why.

---

## 2026-08-29 — Entry 1: Goal & Scope Decision

**PLAN**
Fulfill the active goal: develop R2N so that there are *no stubs, placeholders, mocks,
fakes, TODOs, or half-implementations* — everything real, working, production grade.
Maintain this journal recording plan, goal, what was done and why, plus the options
considered and the choice made.

**GOAL**
A real, end-to-end, tested Rust implementation of the R2N architecture that compiles a
React-like source program into a language-independent artifact and executes it on a
zero-JS runtime producing a correct, minimal patch stream — with genuine unit +
integration tests passing.

**DID**
- Decided to build a *vertical slice* that is honestly production-grade for a defined
  subset (not a fake full React). The subset: components, props, state (`useState`),
  `useEffect`, conditional rendering (`if`/ternary), `key`ed lists (reconciliation),
  event-driven re-renders, and a memory renderer that produces a `Patch[]` stream.
- Scaffolded a Cargo workspace with real, dependency-light crates:
  `r2n-ast`, `r2n-parser`, `r2n-ir`, `r2n-runtime`, `r2n-renderer-memory`,
  `r2n-compiler`, `r2n-cli`.

**WHY**
The repository contained only design docs — zero implementation. A faithful, *complete*
replica of the entire roadmap (M0–M7, dual frontends, Go port, native renderers) is
many person-months and would necessarily contain stubs if rushed. The goal's hard
constraint is "no stubs / everything working." The only way to satisfy that honestly is
to pick a coherent, fully-realized sub-product and implement *all* of it for real.

**OPTIONS**
1. Implement the literal full roadmap (M0–M7): rejected — impossible to do completely
   and stub-free in one pass; would violate the core constraint.
2. Build a thin CLI that "parses" with regex and prints a fake tree: rejected — that is
   exactly the placeholder/mock behavior the goal forbids.
3. Build a real, self-contained vertical slice: a small but genuinely correct compiler
   + zero-JS runtime + patch stream + tests, scoped to a defined feature set. **CHOSEN.**

**CHOSE**
Option 3. Rationale: it is the only path that satisfies "no stubs, everything real and
working." The architecture's *shape* (frontend → IR → interlinked IR → runtime ABI →
patch stream → renderer) is preserved exactly; only the *breadth* of supported surface
is bounded and explicitly documented. This honors the design's spirit (lang-neutral IR,
zero-JS runtime, patch-stream reconciliation) without faking the rest.

---

## 2026-08-29 — Entry 2: Tech & Design Decisions

**PLAN**
Choose the representation and module boundaries.

**GOAL**
Concrete, implementable design that maps to the locked ADRs (interlinked IR, zero-JS
runtime, template/instance split, patch stream).

**DID**
- Frontend: single source-in frontend over a real recursive-descent parser (subset of
  TSX-like syntax). The roadmap's "dual frontend" (compat ESM bundle-in + opt source-in)
  is a deployment concern; for a stub-free vertical slice we implement ONE real parser
  rather than two half parsers. Documented as a scope choice.
- IR: a single `r2n-ir` crate holding AST→IR lowering, with `JsExpr`/`ReactNode`/
  `RuntimeTemplate` types and an `interlink` relationship (React nodes reference JS
  closures by id; JS can call back into component render via a frame protocol).
- Runtime: `r2n-runtime` executes `RuntimeTemplate` against a `Host` providing state
  and scheduling; emits a `Patch` enum stream (Create/Update/Remove/Reorder/Text).
- Renderer: `r2n-renderer-memory` applies `Patch[]` to an in-memory node tree and can
  serialize it, so tests can assert exact output.

**WHY**
Keeps the architecture legible and the contracts real (patches are the ABI), while
staying fully implementable.

**OPTIONS**
- Use a graph IR with cyclic handles vs flat template + instance split: chose the
  template/instance split per ADR-010 (clearer reconciliation, deterministic patches).
- Eval model: AOT lift only, no dynamic eval (ADR-008): chosen.

**CHOSE**
Template/instance split + AOT-only evaluation. Rationale: matches locked ADRs, yields
deterministic, testable patches, and avoids the (out-of-scope) dynamic-eval complexity.

---

## 2026-08-30 — Entry 3: Full Re-Audit, Event System, True Checklist

**PLAN**
Re-verify every claim from scratch (do not trust prior state), fix what is
incomplete, then update the roadmap records to the true state.

**GOAL**
The codebase and its progress records agree, with every claim test-backed.

**DID**
- Re-read all ~4,200 lines of source + tests. Verdict: the compiler/IR/runtime
  skeleton was real (no stubs), but the audit found real gaps:
  1. `r2n-runtime` declared an unused dependency on `r2n-ast` (architecture
     violation — runtime must consume only Runtime IR). Removed; added
     `tests/architecture.rs` so CI enforces the boundary forever.
  2. **No event system.** The roadmap's M0.2/M0.3 exit test —
     `onClick={() => setN(n+1)}` → click → `SetText("1")` — did not exist.
     Implemented: `Value::Handler` (serializable: instance path + closure),
     handler registration during reconciliation, `Runtime::dispatch(node,
     event)` running the closure against the owning frame + saved scope, then
     flushing. Removed the `bump_first_state` test backdoors entirely — all
     reactive-loop tests now go through real events.
  3. Found + fixed a frame-identity bug while doing so: nested elements were
     evaluating against orphan per-node frames instead of their component's
     frame (two path domains — node paths vs instance paths — were conflated;
     now threaded separately).
  4. Parser gaps: JSX raw-text children (`<button>+1</button>`), block-bodied
     arrows (`() => { ... }`), expression statements (`useEffect(...);`).
     Added lexer `rescan_jsx_text` with byte offsets tracked per token.
  5. `useEffect` ordering: statements and `let` bindings now lower in source
     order (an effect after `let n` sees `n`).
  6. `key` prop no longer renders (React strips it too); handler props are
     runtime-internal and hidden from renderer output.
  7. Dead code removed: `Value::Component`, `Builtin`, `JsExpr::Builtin`
     (now an error — AOT-only), `Setter.generation`, `next_node_id`,
     `ser::roundtrip`, `render::apply_all`, `root_component_name`, unused
     `thiserror` deps. Captures are now genuinely computed (free vars).
  8. CLI `run` fires real events on the first clickable; counter example uses
     its `setN`.
- New tests: 5 events tests (click-minimal-patch, two-instance independence,
  useEffect mount/deps semantics, todo E2E, unknown-event error) +
  architecture guard. Suite: 30 green, clippy clean.
- Records updated honestly: M0.1 13/13 DONE; M0.2 12/14 (FIFO scheduler queue
  and the 14-criteria sweep remain); M0.3 7/9 (artifact version stamps,
  diagnostics breadth remain). CHECKLIST.md / ROADMAP.md / roadmap.yaml /
  roadmap.toml all agree.

**WHY**
Progress claims must never outrun the code; the roadmap's completion rule is
"acceptance tests pass, never because an API exists."

**OPTIONS**
- Keep `bump_first_state` as the loop driver: rejected — it was a backdoor
  mutating state outside the event path, exactly the kind of fake the project
  forbids.
- Model handlers as `ReadState/Add/WriteState/Schedule` instruction IR now:
  rejected for this pass — that is M3 specialization; Handler values +
  dispatch are the correct ABI-level semantics today.

**CHOSE**
Real events through the ABI value model, architecture guard in CI, records
matching reality.
