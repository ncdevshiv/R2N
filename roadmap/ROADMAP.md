# R2N — Master Roadmap

**React to Native** — a native compiler and runtime platform that executes existing React applications **without a JavaScript runtime**.

| | |
|---|---|
| Version | 1.0 |
| Updated | 2026-08-29 |
| Status | Design complete (M0.1–M0.2 specs) · implementation not started |
| Horizon | 56 weeks, 10 milestones, 106 tracked tasks |
| Interactive tracker | [index.html](index.html) |
| Machine-readable | [roadmap.yaml](roadmap.yaml) · [roadmap.toml](roadmap.toml) |
| Execution detail | [PLAN.md](PLAN.md) · [CHECKLIST.md](CHECKLIST.md) |

---

## 1. North star

> A **native compatibility platform** capable of compiling existing React/JSX applications into a JavaScript-runtime-free executable representation — while progressively compiling their dynamic behavior into specialized native code.

This is deliberately **not** phrased as "React without JavaScript." That framing is impossible in its strongest reading: existing React apps *are* JavaScript programs, and something must provide their semantics. What can be eliminated is the JavaScript **runtime** — the semantics get compiled into another representation ahead of time.

**The pipeline end-state:**

```
Existing React Application
        │  unchanged source
        ▼
   Native Compiler          ← JS + JSX + React analysis
        ▼
   Compatibility IR         ← JavaScript IR → React IR → Runtime IR
        ▼
   Optimizer                ← specialization when provably safe
        ▼
   Runtime ABI  ────────►  Rust · Go · Elixir runtimes
        ▼
   Platform (native / WASM / terminal)
```

**Production artifact:** `native runtime + compiled application + assets + configuration` — no Node.js, no JavaScript engine, no JavaScript execution at runtime.

## 2. Three pillars

1. **Existing React compatibility** — accept real-world React/JSX applications unchanged. Compatibility is *behavioral* (observable semantics), never merely API-presence.
2. **Zero JavaScript at runtime** — production executes native/WASM artifacts. JS/Node may exist at *build* time only, as input tooling — the shipped runtime knows nothing about source code.
3. **Optimization beyond React** — specialization is the optimization strategy, not "Rust is fast": transform *dynamic React + dynamic JavaScript* into *known component graph + typed state + specialized operations + compact IR + predictable scheduling*.

## 3. Architecture — two worlds, one boundary

```
SOURCE WORLD (build time)                 RUNTIME WORLD (ship)

 JS / JSX / TypeScript                     React IR
        │                                     │
        ▼                                     ▼
     Lexer                                Runtime IR
        ▼                                     │
     Parser                                   ▼
        ▼                                  Optimizer
       AST                                    │
        ▼                                     ▼
     JS IR                              Runtime ABI  ← the only contract
        │                                     │
        ▼                            ┌────────┼────────┐
    React IR ────────────────────►  Rust      Go     Elixir
                                          │
                                          ▼
                                    Renderers: Memory · Native · WASM · Terminal
```

**Non-negotiable boundary rules:**

- The **runtime never sees source code** — no JSX, no JavaScript syntax, no AST, no parser dependency. It executes compiled Runtime IR only.
- **Rust is the first runtime implementation, not the architecture.** The architecture is the IR + ABI; that is what keeps runtimes swappable.
- The **Runtime ABI contains no language-specific concepts** (no Rust structs, Go interfaces, or Elixir processes as semantic requirements — only integers, floats, strings, booleans, handles, buffers, arrays, maps, enums).
- Every renderer consumes the **same Patch stream**.

## 4. Compatibility ladder

Compatibility is promoted only by conformance results — published as percentages, never claimed because a demo works:

| Level | Scope | Milestone |
|---|---|---|
| **L0** | JSX — elements, attributes, children, expressions | M0.3 |
| **L1** | React core — components, hooks, reconciliation, context, suspense, portals | M1 |
| **L2** | JavaScript language — full ECMAScript semantics | M2 |
| **L3** | React ecosystem — router, state, data, UI, animation libraries | M6 |
| **L4** | Browser APIs — events, timers, fetch, storage | M6 |
| **L5** | Frameworks — Next.js/Vite-style app structures | M6 |
| **L6** | Arbitrary production applications | M7 |

Conformance suite end-state: `React Compatibility: 99.x% · JavaScript: 99.x% · Web APIs: xx% · Ecosystem: xx%`.

## 5. Milestone roadmap

| ID | Milestone | Weeks | Tasks | Status |
|---|---|---|---|---|
| M0.1 | Foundation — Workspace & Vertical Slice | 1–2 | 13 | **DONE** (13/13) |
| M0.2 | Reactive Runtime Loop | 2–4 | 14 | **DONE** (14/14) |
| M0.3 | Compiler Frontend — JS/JSX → IR | 4–7 | 9 | in progress (7/9) |
| M1 | React Compatibility — Level 1 | 7–12 | 18 | planned |
| M2 | JavaScript Compatibility — Level 2 | 12–20 | 15 | planned |
| M3 | Optimization Pipeline — Specialization | 20–26 | 10 | planned |
| M4 | Renderers — Native, WASM, Terminal | 26–32 | 5 | planned |
| M5 | Multi-Runtime — Go & Elixir | 32–38 | 6 | planned |
| M6 | Ecosystem Compatibility — Levels 3–5 | 38–50 | 10 | planned |
| M7 | Productionization — 1.0 | 50–56 | 6 | planned |

### M0.1 — Foundation: Workspace & Vertical Slice
Rust workspace (8 crates: `ast, parser, js-ir, react-ir, runtime-ir, runtime, renderer-memory, compiler`), the three foundational specs ([JS_IR](#61-js_irmd), [REACT_IR](#62-react_irmd), [RUNTIME_ABI](#63-runtime_abimd) — drafted, in this repo's history), a minimal AST, both IRs, a runtime skeleton, memory renderer, and `lower()` — proven by `counter_creates_tree`.
**Exit:** counter source → compiler → artifact → Rust runtime → memory renderer shows `button → "0"`; zero deps on JS/Node in the runtime graph.

### M0.2 — Reactive Runtime Loop
The core loop: `event → state mutation → dirty component → scheduler → render → reconcile → patch → renderer`. State keyed by `(ComponentId, StateSlot)`; deterministic FIFO scheduler (no concurrency yet); `Patch` enum (`Create/Remove/Insert/Move/SetText/SetProperty`); minimal-diff reconciler; `Runtime::flush()`.
**Exit:** all 14 acceptance criteria green — including click → `SetText("1")` with no parent recreation, and two Counter instances holding independent state. Then Todo E2E.

### M0.3 — Compiler Frontend
Stop hand-building IR. Lexer → parser → AST → JS IR → React IR. The first genuinely interesting transformation: `useState(0)` → `StateSlot { slot: 0, initial: 0 }`, and `onClick={() => setCount(count + 1)}` → handler IR (`ReadState(0) → Add 1 → WriteState(0) → Schedule`).
**Exit:** Counter compiled from a real `.jsx` file — no manually constructed IR anywhere.

### M1 — React Compatibility (L1)
Full hook set (`useReducer, useEffect, useLayoutEffect, useMemo, useCallback, useRef, useContext, useId`), props/children, **keys** as first-class identity, fragments, lists, class components, error boundaries, portals, Suspense (`Active → Suspended → Resolved`), StrictMode kept out of production artifacts.
**Exit:** behavioral conformance suite v1 green; `react_compatibility_version` stamped per artifact.

### M2 — JavaScript Compatibility (L2)
Full value model (incl. `BigInt`, `Symbol`), closures/lexical environments, classes/`this`/prototypes, equality & coercion (`==` vs `===`, `ToPrimitive`), exceptions, promises + async/await with scheduler-driven continuations, generators/iterators, modules (incl. dynamic import + init order), destructuring/spread, Proxy/Reflect, RegExp, GC honoring observable lifetime semantics, TypeScript consumed as **optimization hints only**.
**Exit:** test262-subset harness running; published JS compatibility score.

### M3 — Optimization Pipeline
Static analyzer (purity, mutability, escape, constant, shape hints) → specialization (`user.name` → typed field op; `obj[key]` stays generic) → **dual-tier execution**: fast path + compatibility fallback. Benchmark harness built *now*, not after optimization: startup, first render, update latency, reconciliation, memory, allocations, binary size — across the 10-app corpus, comparing reference React vs compatibility runtime vs optimized runtime.
**Exit:** Tier-1 targets met; no observable-semantics regressions (CI-gated).

### M4 — Renderers
Renderer conformance tests first (identical patch stream everywhere), then: **Native** renderer (component → runtime tree → native widget — no browser, no DOM, no JS engine, no bindings), **WASM** renderer (browser artifact, zero JS at runtime), **Terminal** (cheap integration target), per-platform event normalization.
**Exit:** same conformance suite passes through every renderer.

### M5 — Multi-Runtime (Go & Elixir)
Freeze **Runtime ABI v1** (ops, handles, errors, capabilities, discovery, versioning). Artifact format spec (manifest + ABI version + IR + assets). Go and Elixir runtimes passing **exactly the same conformance suite** — proving the artifact is language-independent. Hot replacement: `ReplaceComponentImplementation` + deterministic state migration.
**Exit:** cross-runtime CI — one artifact, three runtimes, identical observable behavior.

### M6 — Ecosystem Compatibility (L3–L5)
The hardest phase, explicitly scoped: module/package resolution (`node_modules` mapping, import maps), react-router, Redux/Zustand, TanStack Query, form/UI/animation library subsets, browser-API layer, real-app corpus (TodoMVC, RealWorld, dashboard, 10k-row table).
**Exit:** compatibility scorecard published across all four dimensions.

### M7 — Productionization
`r2n build / run / check` CLI, production artifact packaging, state-preserving hot reload, docs, semver + IR forward-compatibility policy.
**Exit (v1.0):** a large existing React repository compiles and runs unchanged — zero JavaScript at runtime.

## 6. Foundation specs (drafted M0.1)

### 6.1 JS_IR.md
Value model (`Undefined/Null/Boolean/Number/BigInt/String/Symbol/Object/Function/External`); instruction set (constants, locals/globals, objects/arrays/closures, property access incl. dynamic, arithmetic, comparison, control flow, iterators, await/yield, throw); lexical environments for closures; modules; explicit try/catch/finally; two execution modes (compatibility = exact semantics, optimized = proven-equivalent specialization); optimization metadata as hints, not semantics; `ir_version` stamping.

### 6.2 REACT_IR.md
Entities: Application, Module, Component, ComponentInstance, Element, Fragment, Props, State, Effect, Context, Ref, Event, Suspension, Portal. Component identity is **stable application identity**, independent of implementation version (enables hot replacement). State identity = component + **hook position** (ordering is semantic). Effects carry `dependencies/setup/cleanup`. Reconciliation represents PreviousTree + NewTree → Create/Update/Delete/Move/Replace. Keys are first-class — never assume list position equals identity when keys exist. Dev-only (StrictMode) behavior must never leak into optimized production artifacts.

### 6.3 RUNTIME_ABI.md
Runtime responsibilities: component instances, state, effects, events, scheduling, reconciliation, memory, rendering, I/O capabilities, errors. Lifecycle (initialize → load → create root → render → commit → ready; symmetric shutdown). Node/component/state/effect ops. Scheduler priorities (Immediate, UserBlocking, Normal, Low, Idle) — algorithm implementation-specific, **observable ordering compatible**. Handles never expose raw pointers. Standard `RuntimeError` codes. Explicit capability requests (`ui, network, filesystem, storage, clock, process, crypto`). ABI versioning: minor = additive, major = breaking. Every runtime passes the same conformance suite or it is not compatible.

## 7. Performance engineering targets

Engineering targets, not promises. The honest baseline: a first-generation compatibility runtime will **not** automatically beat V8 — decades of hidden classes, inline caches, and JIT work stand behind `obj[key] = value`. The wins come from compilation and specialization, not from "Rust."

| Tier | Startup | Memory | UI updates | CPU workloads |
|---|---|---|---|---|
| **Tier 1** — Compatibility runtime | 1.5× | −10–20% | ≈ parity | — |
| **Tier 2** — Native IR optimization | 3–5× | −30–50% | 2–5× | 2–10× |
| **Tier 3** — Aggressive specialization | 5–10× | −50–70% | 3–10× | 5–20×+ |

**Reality check (UI workloads):** if the browser spends 8 ms on layout/paint, making a 4 ms reconciler 4× faster only improves the total from 12 ms → 9 ms (1.33×). The structural win is native rendering — no browser, no DOM, no JS engine, no DOM bindings: `component → runtime tree → native widget`. Memory may be the bigger win overall (compact component instances vs JS objects + fiber + closures + GC metadata).

**Benchmark suite:** Counter · TodoMVC · Large table · 10,000-row list · Complex form · Dashboard · Animation-heavy UI · Large component tree · CPU-heavy app logic · Real-world React apps — each measured on startup, time-to-interactive, first render, update latency, reconciliation time, memory, CPU, binary size, battery, allocation pressure. Never optimize from intuition.

## 8. The hard truths (carried explicitly)

1. **"100% React API compatibility" ≠ "100% arbitrary ecosystem compatibility."** The second is vastly harder; libraries like Three.js or Framer Motion lean on deep JS behavior.
2. **Don't promise 100% at the start.** Compatibility is a measured, published progression (the ladder above).
3. **V8 is the benchmark to respect.** Generic compatibility interpretation may initially be *slower* than V8; the win comes from the optimized tier.
4. **IR explosion risk.** That's why Go/Elixir are gated behind a proven, frozen ABI — never multiply an unstable design across three implementations.
5. **Semantic correctness before speed.** No JIT, no native codegen, no parallel rendering until the reactive loop and conformance suite are solid.

## 9. Definition of done — v1.0

- [ ] Vertical slice: source → compiler → artifact → Rust runtime → memory renderer, zero JS/Node in the runtime.
- [ ] Real reactive loop with minimal patches and batched updates.
- [ ] Instance semantics: independent state per instance; state identity = component + hook position.
- [ ] Behavioral conformance suites for React core and JS language, published as percentages.
- [ ] Same artifact executes on Rust, Go, Elixir — identical observable behavior.
- [ ] Compatibility scorecard (React / JS / Web API / Ecosystem) published.
- [ ] Large existing React repository runs unchanged, zero JavaScript at runtime.
