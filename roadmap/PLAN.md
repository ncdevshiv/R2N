# R2N — Execution Plan

Companion to [ROADMAP.md](ROADMAP.md) (the *what*) — this is the *how and in what order*, plus the working designs already settled for the first milestones. Task-level tracking lives in [CHECKLIST.md](CHECKLIST.md) and the interactive [index.html](index.html).

---

## 1. Strategic execution order

The order is the contract; the calendar weeks are indicative only.

```
Vertical slice (M0.1)          ← prove the pipeline shape
  → reactive runtime (M0.2)    ← prove real component/state semantics
    → compiler frontend (M0.3) ← stop hand-building IR
      → React compat (M1)      ← behavioral conformance
        → JS compat (M2)       ← the compatibility foundation
          → optimizer (M3)     ← only now, speed
            → renderers (M4)   ← native + WASM
              → ABI freeze + Go/Elixir (M5)
                → ecosystem (M6) → production (M7)
```

Three deliberate sequencing decisions:

1. **Do not build Go or Elixir yet.** The Rust runtime must first prove the IR/ABI can represent real React behavior; otherwise an unstable design gets multiplied across three implementations. Runtimes are gated behind an ABI freeze (M5).
2. **Do not optimize yet.** No JIT, no native codegen, no SIMD, no parallel rendering, no advanced GC. Semantic correctness first — optimization is gated behind a green conformance suite (M3) and is CI-guarded against observable-semantics regressions forever after.
3. **Benchmark harness exists before optimization does** (M3 starts with the harness + corpus, before any specialization work).

## 2. Repository layout & dependency law

```
native-react/
├── Cargo.toml            # workspace, resolver = "2"
├── crates/
│   ├── ast/              # minimal typed AST (grows with compatibility)
│   ├── parser/           # lexer + parser (JS/JSX/TS)
│   ├── js-ir/            # JavaScript semantics IR
│   ├── react-ir/         # component/hook/reconciliation IR
│   ├── runtime-ir/       # language-independent execution format
│   ├── runtime/          # the engine (no source knowledge)
│   ├── renderer-memory/  # first renderer (tests without a browser)
│   └── compiler/         # lowering pipeline + public compile() API
├── examples/counter/
├── tests/{parser,ir,runtime,conformance}/
├── benchmarks/{startup,render,update,reconciliation,state,events,memory,large-tree}/
└── docs/{JS_IR,REACT_IR,RUNTIME_ABI}.md
```

**Dependency direction (enforced by CI):**

```
parser → ast → js-ir → react-ir → runtime-ir → runtime → renderer
```

Forbidden forever: `runtime → parser`, `runtime → compiler`, `runtime → ast`. The runtime must not know that source code exists. The Source World (parser/AST/JS IR/React IR) is build-time only; the Runtime World (Runtime IR → optimizer → ABI → runtimes) is what ships.

## 3. Working designs already settled

### 3.1 State identity (M0.1 → M0.2 correction)
State is keyed by **(ComponentId, StateSlot)** — not a bare slot. Component *identity* + *hook position* = state identity. This single rule is what makes existing hook-based React code map correctly, and it was retrofitted into the IR instructions:

```rust
CreateState { component: ComponentInstanceId, slot: StateSlot, initial: Value }
ReadState   { component: ComponentInstanceId, slot: StateSlot }
WriteState  { component: ComponentInstanceId, slot: StateSlot, value: Value }
```

Definition vs instance is explicit: one `Counter` definition, N mounted instances, each with independent state.

### 3.2 The patch stream (M0.2)
Renderers no longer receive imperative instructions; they receive diffs:

```rust
enum Patch {
    Create { node: NodeId, element_type: String },
    Remove { node: NodeId },
    Insert { parent: NodeId, child: NodeId, index: usize },
    Move   { parent: NodeId, child: NodeId, index: usize },
    SetText { node: NodeId, value: String },
    SetProperty { node: NodeId, name: String, value: Value },
}
```

`Render → Tree → Diff → Patch[] → Renderer`. The reconciler v0 is deliberately primitive (id-keyed, create/update/remove); keyed list reconciliation with moves lands in M1 when keys become first-class. This is the point where the system stops being an instruction interpreter and becomes a rendering engine.

### 3.3 The core loop (M0.2)

```rust
impl<R: Renderer> Runtime<R> {
    pub fn flush(&mut self) {
        while let Some(instance) = self.scheduler.next() {
            self.update_component(instance);   // render → reconcile → apply → swap tree
        }
    }
}
```

`WriteState` never touches the renderer directly: it marks the component dirty, schedules it, and the flush loop produces the minimal patch set. Batching falls out for free (multiple writes → one schedule → one flush).

### 3.4 First compiler transformation (M0.3 target)

Input (real source, no manual IR):

```jsx
function Counter() {
  const [count, setCount] = useState(0);
  return <button onClick={() => setCount(count + 1)}>{count}</button>;
}
```

Output (React IR ≈):

```
Component Counter
  State:    slot 0 = Number(0)
  Render:   Element button
              Child: StateText(slot 0)
  Event:    click → ReadState(0) → Add 1 → WriteState(0) → Schedule(Counter)
```

The handler extraction (`onClick={() => setCount(count + 1)}` → ordered state IR) is the first genuinely interesting compiler transformation and the gate for M0.3 exit.

## 4. M0.2 acceptance criteria (the 14 gates)

M0.2 is complete **only** when all of these pass:

1. Component can mount
2. Component can unmount
3. State persists between renders
4. Two instances have independent state   ← the "legitimate runtime, not a toy" test
5. Event changes state
6. State update schedules component
7. Multiple state updates are batched
8. Render produces a new tree
9. Reconciler produces minimal patches
10. Renderer applies patches
11. Text updates without recreating the parent
12. Child insertion works
13. Child removal works
14. Component identity remains stable

## 5. Compatibility expansion order

After the vertical slice, compatibility widens in bands — never jumping ahead:

- **Phase A:** JSX, function components, props, children, useState, events
- **Phase B:** useReducer, useEffect, useRef, useMemo, useCallback, useContext
- **Phase C:** class components, refs, portals, error boundaries, Suspense, async behavior
- **Phase D:** arbitrary JavaScript semantics (M2 scope)

Only once the compatibility machinery is strong does aggressive specialization begin (M3).

## 6. Optimization strategy (M3)

Dual-tier execution — the system never forces everything through the slowest abstraction:

```
JS IR → Static Analysis ─┬─ provably static  → Specialized IR (typed fields, native ops)
                         └─ dynamic          → Compatibility IR (generic semantics)
```

- `user.name` → `LoadField User.name` (when analysis proves the shape)
- `obj[key]`  → `DynamicGetProperty` (unless proven otherwise)

Optimization **never changes observable semantics** — equivalence must be proven, and the conformance suite guards every transform in CI. The optimization strategy is compilation and specialization, not "Rust."

## 7. Testing strategy

| Layer | Method |
|---|---|
| IR | Snapshot tests + `ir_version` stamping |
| Runtime | The 14 acceptance criteria; two-instance test; batching test |
| React compat | **Behavioral** conformance suite — observable behavior, never API-presence |
| JS compat | test262 subset harness; published score |
| Renderers | Identical patch-stream conformance across memory/native/WASM/terminal |
| Runtimes | Same conformance suite on Rust, Go, Elixir; cross-runtime CI |
| Performance | Benchmark corpus vs reference React vs optimized runtime, tracked over time |
| Architecture | CI dependency-direction check (runtime cannot import source-world crates) |

## 8. Risk register

| Risk | Impact | Mitigation |
|---|---|---|
| Compatibility runtime slower than V8 | Credibility, adoption | Dual-tier execution; specialize what analysis proves; publish honest per-tier targets |
| IR churn breaks artifacts | Rework | `ir_version` stamping + forward-compat policy from M0.3 |
| Ecosystem long tail (L3–L5) | Schedule slip | Scorecard culture: ship percentages, gate claims on conformance, subsets first (M6) |
| Premature multi-runtime | Design multiplied ×3 | ABI freeze gate before Go/Elixir (M5) |
| Performance theater | Wasted effort | Benchmarks exist before optimization; no intuition-driven tuning |
| Dev-only semantics leak to prod | Subtle bugs | StrictMode/dev metadata represented separately in IR; prod artifacts strip it (M1) |

## 9. Cadence & gates

- **Milestone gate:** a milestone closes only when its acceptance tests pass — then CHECKLIST/yaml/toml are updated and the tracker is re-baselined.
- **Spec gate:** before any new subsystem, the spec lands in `docs/` first (JS_IR → REACT_IR → RUNTIME_ABI established the pattern).
- **Review questions per gate:** Does the runtime still know nothing about source? Did observable semantics change? Do the benchmarks regress? Is the ABI still language-neutral?
