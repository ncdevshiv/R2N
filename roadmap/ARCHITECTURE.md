# R2N — Decisive Architecture & Long-Term Plan

**React to Native.** The corrected, decision-locked architecture that resolves the open calls raised in [SPEC_CRITIQUE.md](SPEC_CRITIQUE.md), plus a realistic long-term plan (effort bands + kill criteria). This is the *"what is it really and how do we actually build it"* doc. The [ROADMAP.md](ROADMAP.md) remains the milestone list; [PLAN.md](PLAN.md) the execution order; this doc is the architecture you build into and the plan you can promise.

| | |
|---|---|
| Version | 1.0 |
| Updated | 2026-08-29 |
| Status | Design decisions locked · implementation not started |
| Companion | [SPEC_CRITIQUE.md](SPEC_CRITIQUE.md) (findings) · [ROADMAP.md](ROADMAP.md) (milestones) · [PLAN.md](PLAN.md) (execution) |

---

## 0. The one-sentence truth

> R2N compiles a React application into a **language-independent artifact** (IR + ABI + assets) that any conformant runtime — Rust first, Go second — executes **without a JavaScript engine**, and it compiles away JavaScript semantics when it can *prove* they're unnecessary.

Three words carry the entire program: **artifact** (not "app"), **conformant** (behavioral conformance is the only measure of compatibility), **prove** (optimization never touches observable semantics without a proof).

---

## 1. The corrected shape — extraction, not sequential lowering

### 1.1 The original pipeline was wrong

The first draft drew lowering as a clean sequence:

```
JS IR  →  React IR  →  Runtime IR
```

That is not what a React application is. A React component is **a JavaScript function that calls React APIs**:

```jsx
function Dashboard({ data }) {
  const processed = useMemo(                    // ARBITRARY JS in the hot path
    () => data.filter(x => x.score > 10).map(x => x.label).sort(),
    [data]
  );
  const [open, setOpen] = useState(false);       // React
  return open ? <Panel rows={processed} /> : null; // both
}
```

`useMemo` callbacks, event handlers, reducer bodies, effect callbacks, `React.memo` comparators, context default-value computations — all of these are **JavaScript that runs on every render**. React IR cannot "lower from" JS IR; instead **React IR nodes embed references back into JS IR function bodies**, and JS calls back into React. The real dataflow is bidirectional:

```
React IR  ⇄  JS IR      (React nodes contain JS closures; JS calls back into React)
     ↓
Runtime IR executes BOTH, under one defined calling convention
```

### 1.2 The corrected pipeline — extraction + interlink + dual frontend

```
 EXISTING APP   ┌──────────────────────────────────────────────────┐
   (bundler       │  COMPAT FRONTEND (bundle-in)                  │   THE COMPATIBILITY CLAIM
    output/ESM)──►│  app's own bundle → JS IR                      │   ──────────────────────────
                 │  JSX already compiled to jsx() calls           │   This is the only credible
                 │  (pattern-matched, like React DevTools)        │   route to "arbitrary apps".
                 └──────────────────────────────────────────────────┘
                                                                        │
 GREENFIELD  ┌──────────────────────────────────────────────────┐      │
   (raw TSX) ─►│  OPT FRONTEND (source-in)                       │      │  THE PERFORMANCE/SOURCE CLAIM
                 │  oxc/swc parser → AST → JS IR                  │      │  ─────────────────────────────
                 │  full static signal for the optimizer          │      │  Only for new apps / strong
                 └──────────────────────────────────────────────────┘      │  optimization.
                                                                              │
                        ┌────────────────────────────────────────┐         ▼
                        │        JS IR  ⇄  REACT IR  (interlink) │      Runtime IR
                        │        (extraction + frame protocol)   │         │
                        └────────────────────────────────────────┘         ▼
                                                                        Optimizer
                                                                          │
                                                            ┌───────────┴───────────┐
                                                        Runtime ABI ← the only contract
                                                                      │
                                          ┌─────────┬─────────┬─────────┬─────────┐
                                       Rust       Go        (wasm)     host      │
                                                                              │
                                              ┌───────────────────────────────┘
                                              ▼
                                     Renderers (memory / native / wasm-in-browser / terminal)
```

**Why this is sound:**

- **Both frontends converge at JS IR.** Everything below JS IR (React IR interlink, Runtime IR, optimizer, ABI, runtimes, renderers) is identical regardless of input contract. Nothing already designed is wasted.
- The **compat claim** ("arbitrary apps") rides bundle-in, which kills the bundler/plugin/CJS/alias/`NODE_ENV` problem in one stroke. The **performance claim** rides source-in. You are not picking one; you are sequencing two frontends over one spine.
- The pipeline is now **extraction + interlink**, not lowering. That is the honest name for what a React compiler does.

### 1.3 Template vs instance — the rule that fixes four bugs

The IR conflated compile-time template with runtime instance. One rule separates them:

```
ARTIFACT (compile time) = TEMPLATES   : component definitions, hook slot layouts,
                                        element skeletons with template-internal IDs
RUNTIME  (execution)    = INSTANCES   : allocated per mount; instance-scoped state,
                                        runtime node handles, runtime event registrations
```

| Concern | Compile time | Runtime |
|---|---|---|
| Node identity | `TemplateNodeId` (skeleton-local) | `NodeHandle` (per instance, runtime-allocated) |
| State identity | `(ComponentId, SlotIndex)` layout | `(ComponentInstanceId, SlotIndex)` |
| Events | `HandlerId → template` | `HandlerId → (instance, IR continuation)` |
| Reconciliation | n/a | key = **(element type, key, structural position)** |

This single rule fixes four of the critique's drafted bugs: static `NodeId` on elements (bug), instance IDs at compile time (bug), no fragment/array-children representation (bug), and diffing on the wrong identity (bug).

---

## 2. The two conceptual worlds

```
 SOURCE WORLD (build time)                      RUNTIME WORLD (ship)
 ─────────────────────────                      ───────────────────────
 JS / JSX / TS (app is already                  React IR
 a JS program; JS semantics are                    │
 what we must provide)                             ▼
        │  extraction + interlink               Runtime IR
        ▼                                          │
   JS IR ⇄ React IR                                ▼
        │                                       Optimizer
        ▼                                          │
   Runtime ABI ← the only contract  ──────────► Runtime ABI
                                                  │
                               Rust · Go · host shim
                                                  │
                                             Renderers
```

**Non-negotiable boundaries (unchanged, and still correct):**

1. The **runtime never sees source code** — no JSX, no JS syntax, no AST, no parser (with the `eval` caveat locked in §4.5).
2. **Rust is the first runtime, not the architecture.** The architecture is the IR + ABI; that is what keeps runtimes swappable.
3. The **ABI contains no language-specific concepts** — only integers, floats, strings, booleans, handles, buffers, arrays, maps, enums.
4. Every renderer consumes the **same patch stream**.
5. **Dependency direction, enforced by CI** (unchanged): `parser → ast → js-ir → react-ir → runtime-ir → runtime → renderer`. Forbidden forever: `runtime → parser`, `runtime → compiler`, `runtime → ast`.

---

## 3. The 7 locked decisions (Architecture Decision Records)

Each of the following was left open by the critique with instruction "do not let the code decide by accident." Each is now decided, with rationale. **ADR status: Accepted.**

### ADR-001 — Input contract: dual frontend, bundle-in compat, source-in opt

- **Status:** Accepted
- **Context:** "Arbitrary existing React apps" silently assumed raw-source input — which forces R2N to reimplement the bundler, the build-plugin ecosystem (JS that runs at build time), CJS/ESM interop, `NODE_ENV` inlining, and `import.meta`. That is where compatibility projects die.
- **Decision:** Two frontends, converged below JS IR:
  - **Compat frontend = bundle-in** (the app's own ESM bundler output). Imports resolved, JSX already compiled to `jsx()` calls, `NODE_ENV` inlined, assets hashed. Pattern-matching on the jsx runtime is exactly what React DevTools does.
  - **Opt frontend = source-in** (raw TSX for greenfield/new apps). Full static signal for the optimizer.
  - **Shared spine:** JS IR → React IR interlink → Runtime IR → optimizer → ABI → runtimes.
- **Consequences:** The compat claim becomes reachable; the plugin/CJS/alias problem disappears. Trade-off: bundle-in loses sugar-less source spans (Debug info comes from the bundle's sourcemap) and TS types (only matters to the specialization tier). Source-in costs a real parser — that's ADR-004.

### ADR-002 — React IR ⇄ JS IR are interlinked, not sequential

- **Status:** Accepted
- **Context:** React components embed arbitrary JS in the hot path. Sequential "JS IR → React IR" is unfixable; the compiler needs a closure/continuation reference type that doesn't exist in a flat IR.
- **Decision:** React IR nodes may reference JS IR function bodies (render, handlers, comparators, cleanups); JS IR calls back into React via a defined **frame protocol**. See ADR-003. The pipeline is **extraction + interlink**, never "lowering."
- **Consequences:** The IR needs a `Closure`/`Continuation` value type and a bidirectional calling convention. This must be designed in before M0.3 — retrofitting it after the compiler grows is a rewrite of the compiler and all three IRs. This is the single highest-leverage correction.

### ADR-003 — Interop layer: `HOOK_INTEROP.md` (frame protocol + dynamic hook indexing)

- **Status:** Accepted
- **Context:** `useState` is a call *inside interpreted JS IR* that must find native runtime state. The compiler cannot statically assign hook slots in general (custom hooks, conditional hooks, dynamic dispatch — `const hook = useAuth; hook()`). React's answer is a module-global current-dispatcher + current-fiber.
- **Decision:** Add spec **`HOOK_INTEROP.md`** before M0.3, defining:
  1. **Frame protocol** — how the interpreter knows the current `ComponentInstanceId` + hook index (an interpreter-frame ↔ instance binding table / frame-register), and who allocates the slot on first call vs reads on later calls.
  2. **Calling convention** — native ⇄ IR: value marshalling, error propagation, re-entrancy (a render triggers a state update that schedules another render).
  3. **Dynamic hook indexing** — not static slots as the draft assumed; runtime dynamic index with an "invalid hook call" diagnostic when the protocol is violated.
- **Consequences:** Faithful hook semantics across arbitrary call chains — the only way `useState` inside a real custom hook works. Cost: one more spec + a runtime frame-register. This is mandatory before M0.3.

### ADR-004 — Frontend: integrate oxc or swc, don't hand-write the parser

- **Status:** Accepted
- **Context:** M0.3 planned an in-house lexer/parser/AST. A production-grade TS/TSX parser is a multi-quarter project that stays subtly wrong for a year; and the specialization tier needs the *type checker* too (tsc is ~100k lines).
- **Decision:** Integrate **oxc** (currently the more spec-complete, actively developed Rust crate) or **swc** as the parser frontend. Write only the AST → JS IR lowering + the React extraction pass. Consume types from the app's own `tsc`/build output for type-driven optimization hints rather than reimplementing inference.
- **Consequence:** Preserves "no Node/JS at build time" (they're Rust); moves the effort to the *hard* part (extraction + the interlink), which is where the value is. Trade-off: dependency on a third-party crate — mitigated by a thin, swappable frontend trait.

### ADR-005 — Optimization order: inline caches → tier-up → static specialization (last)

- **Status:** Accepted
- **Context:** The M3 plan led with compile-time static specialization ("`user.name` → `LoadField`") and treated the interpreter as fallback. That's backwards and unsound: `user.name` is only a field load if `user` is provably a plain object with a data property — a getter or Proxy makes it arbitrary code. Compile-time proof over arbitrary JS is whole-program analysis.
- **Decision:** Re-order M3 to the proven engine baseline:
  1. **Shapes (hidden classes) + inline caches + fast paths in the compatibility interpreter** — self-guarding, always sound, no proofs needed. This is the technology in every JS engine.
  2. **Profile-guided tier-up** — specialize hot paths based on runtime observations.
  3. **Static specialization last** — only where whole-program analysis *proves* equivalence, against a formal **trace-contract** definition of observable behavior.
- **Consequences:** The realistic performance baseline becomes "compiles V8's own trick," not "Rust is fast." This is the only honest path to the published targets, and the trace-contract (patches, effects, console, timers, network, event order) guards every transform in CI.

### ADR-006 — The defensible performance framing (and target split)

- **Status:** Accepted
- **Context:** Implementing closures/prototypes/coercion/`Proxy`/async/GC **is** building a JavaScript engine — the only missing word is "engine." A plain bytecode interpreter without tier-up loses to V8 by 10–50× on hot code. And the in-browser (WASM) claims invert: WASM can't touch the DOM, and shipping a Rust runtime + interpreter + app as wasm (2–5 MB) competes poorly against V8 streaming-parsing gzipped JS (~200 KB).
- **Decision:**
  1. **Reword pillar 2** to the honest claim: *"zero application JavaScript; a fixed, auditable platform shim (browser target) and none at all (native target)."*
  2. **Sequence M4 browser-first**: WASM-in-browser reuses the browser's CSS/layout engine behind the shim (90% visual fidelity immediately); native is where the perf ceiling is.
  3. **Split the performance-target table** into **native** vs **wasm-in-browser** columns with independent numbers. The 2–10× claims only hold on native.
- **Consequences:** No false promise; M4 won't stall for a quarter chasing ideology. Trade-off: the pitch reads as "React, compiled, with a JS-engine-free runtime" — which is the true, defensible claim.

### ADR-007 — GC: tracing GC, Go as ABI proof, Elixir → research stretch

- **Status:** Accepted
- **Context:** JS object graphs are cyclic by construction (DOM ↔ handler ↔ state ↔ component ↔ back), so reference counting leaks; the compatibility tier needs a real **tracing GC**. Without one, the Rust runtime can't run a real app. And BEAM's shared-nothing model is fundamentally at odds with a shared mutable JS object graph — Elixir is a likely dead end for interpreter-class throughput.
- **Decision:**
  1. **Rust runtime**: host GC doesn't exist — write a tracing GC (mark-sweep minimum; incremental/generational to respect the 16 ms frame budget), represented as `Vec<GcBox>` + index handles (which also satisfies the ABI's "handles, never pointers" rule). This is a real workstream and must be in the roadmap from M0.2-adjacent.
  2. **Go is the ABI-portability proof** — it has a host GC we didn't write, which is *stronger* evidence the ABI is language-independent than three green checkmarks.
  3. **Elixir demoted to research stretch goal**, explicitly published as such.
  4. **WeakRef policy**: support WeakRef against live objects; `FinalizationRegistry` callbacks = "eventually, on the task queue" (allowed nondeterminism, written down so conformance tests don't flake).
- **Consequences:** The Rust runtime genuinely runs cyclic JS without leaking; the ABI is proven on a GC'd host we didn't build. Trade-off: Elixir is out of the critical path.

### ADR-008 — `eval`/`new Function`: AOT-lift constant strings + declare dynamic out of scope

- **Status:** Accepted
- **Context:** ES semantics include `eval` and `new Function`. Supporting them at runtime requires a parser inside the runtime — violating the boundary rule. Many libraries use them (template engines, old validators, some i18n).
- **Decision:** **Option 2 + 3.**
  - **AOT-lift** eval'd strings where statically visible (the common case: constant-string eval).
  - **Declare dynamic eval out of scope for L2/L3**, with a clear diagnostic and a documented, capability-gated host eval escape hatch.
  - Write the "no dynamic eval" boundary into JS_IR.md; add a **capability-leak review** task to M2 (the capability list exists in the ABI, but nothing audits that the compat tier can't bypass it).
- **Consequences:** The runtime boundary holds except for a gated escape hatch. Trade-off: library code that depends on dynamic eval gets a diagnostic, not silent breakage.

### ADR-009 — Strings are UTF-16; number formatting is ECMA-exact

- **Status:** Accepted
- **Context:** JS strings are UTF-16 (`"𝕊".length === 2`), Rust `String` is UTF-8 — naive use gives wrong results for every astral-plane char. `*v as i64` saturates; `Rust Display` doesn't match ECMA-262 `Number::toString` (`1e21`, `1e-7`, shortest-round-trip, `-0`, `NaN`, `Infinity`).
- **Decision:**
  - **String representation:** `Vec<u16>` (simplest correct) or a latin1/UTF-16 hybrid like V8's. Pick and write into JS_IR.value-model.
  - **Number → string:** Ryu-style formatter + the spec's exponent rules, with a test bank for `-0`/`NaN`/`Infinity`/`1e21`/`1e-7`.
- **Consequences:** No classic engine bug (UTF-16) or spec-vs-Display drift. This is the standard "engine must-miss" both engines get wrong — we get it right at the cost of a small, contained formatter.

### ADR-010 — Reconciliation identity: (type, key, structural position), deterministic order

- **Status:** Accepted
- **Context:** Drafted reconciler diffed on `NodeId` and iterated a `HashMap` (nondeterministic patch order — but patch order is observable). Real React reconciles children by (element type + key + structural position), because that's what survives re-render.
- **Decision:** Reconciliation keys on **(element type, key, structural position)**. Patch generation is **deterministic ordered traversal**. Keys are first-class (never assume position = identity when keys exist). Placeholders for `false/null/undefined` children (React drops them; comment nodes stabilize positions).
- **Consequences:** Deterministic, observable-stable patches; correct `{items.map(...)}` reconciliation. This is prerequisite for the M0.2 reactive loop to be trustworthy, and for any renderer conformance.

---

## 4. The remaining design specs to write (before their milestone)

The critique's change list, now sequenced:

| # | Spec | When required | Why |
|---|---|---|---|
| S1 | **HOOK_INTEROP.md** | Before M0.3 | The bug blocker — frame protocol + interlink. ADR-003. |
| S2 | **ARTIFACT_FORMAT.md** | Before M5 ABI freeze | Binary encoding, versioned sections, checksums, debug/spans. |
| S3 | **SCHEDULING.md** | Before M1 exit | Task/microtask/rAF/commit ordering tables + batching + interruption. ADR via §4.1. |
| S4 | **REACT_API_TABLE.md** | M1 entry | Every react + react-dom export, per-version status + test refs. |
| S5 | **Trace contract** | M3 entry | The observable-behavior list every optimizer transform is proven against. ADR-005. |

### 4.1 Scheduling — the observable core

The RUNTIME_ABI's five priorities are an enum, not a model. Observable behavior in React 18/19 depends on:

- **Automatic batching** across event handlers, promises, timeouts (the React 17/18 split is version-observable — pick and test per `react_compatibility_version`).
- **Microtask-before-task ordering**: promise continuations drain before the next task; render flushes are a task — which is *why* a chain of `setState` in `.then()` batches into one render.
- **rAF vs layout effects vs passive effects** ordering (framer-motion leans on this).
- **Interruption semantics**: `startTransition` renders are discardable-and-restartable — a different execution model, not a priority.
- **Tear rules** for external stores under concurrent rendering (`useSyncExternalStore`).

**Gate:** M1 exits on a **differential ordering test bank** — "in what order do these logs appear" scenarios run against real React and R2N, diffed. This harness, not a checklist of hooks, is the real M1 exit.

---

## 5. The versioned React/JS compatibility surface

### 5.1 React version target

- **Target `react_compatibility_version`:** decide React **19** (or 18) as the primary; be explicit about negative scope: legacy context API (removed in 19) is excluded if 19 is the target — must be stated.
- **The API table (S4)** is the only credible basis for the "99.x%" number. Every exported symbol of `react` and `react-dom` gets a status + conformance test ref.

### 5.2 Hooks the original M1 missed (all load-bearing)

`useSyncExternalStore` (the hook the whole state-ecosystem routes through — Zustand, Redux, Jotai, Recoil, Valtio, MobX-react, react-query's subscription core), `useTransition`/`useDeferredValue`, `useOptimistic`, `useActionState`/`useFormState`, `use()`. Without `useSyncExternalStore`, M6's "state libraries" line is unimplementable, not just incomplete.

### 5.3 Excluded-by-design — publish the boundary

Some ecosystem members are **incompatible by design**, not by shortfall. Name the boundary up front — a scorecard that silently omits them will be called a lie:

- **react-three-fiber / react-pdf / custom reconcilers** — build on `react-reconciler`; R2N replaces the reconciler, so they're incompatible by design.
- **styled-components / emotion** — inject `<style>` into `document.head`; meaningless in a native renderer (no cascade).
- **Libraries depending on `getBoundingClientRect`/`IntersectionObserver`/scroll** — need real browser-API implementations or virtual-list libraries break.

---

## 6. Realistic long-term plan (effort bands + kill criteria)

### 6.1 The honest timeline

The original 56-week calendar is off by 3–5× in the middle. **Re-baseline: effort bands (S/M/L/XL), decoupled from dates.** Staffing is per-milestone, and every stage gate has kill criteria.

| Milestone | Effort band | Core deliverable | Gate / kill criterion |
|---|---|---|---|
| **v0.1** — vertical slice + app-relevant JS subset + differential harness | **M (12–16 wk, 2–3 eng)** | source → compiler → artifact → Rust runtime → memory renderer; a real JS subset that runs a non-trivial app; the differential trace harness | Kill if differential pass-rate on the top-100 scenario corpus < 80% after M2 → stop, re-scope |
| M0.3 Compiler frontend | **S** | oxc/swc integration + AST→JS-IR lowering + React extraction + HOOK_INTEROP | — |
| M1 React compat (L1) | **M** | React 18/19 hook set, keys, reconciler, differential harness **green on scenario corpus** | Kill if > 6 mo to green on scenario corpus |
| **M2 JS compat (L2)** | **XL** | full value model, closures, classes, promises, modules, **tracing GC**, test262 subset | This is the program's center of mass — a quarter+ for an app-relevant *subset* with a GC |
| M3 Optimization | **M** | shapes + inline caches → tier-up → static specialization; trace contract; benchmark harness first | Never optimize before conformance is green (CI-gated) |
| M4 Renderers | **L** | browser-first (wasm behind shim reusing browser CSS/layout), then native (taffy/cosmic-text/accesskit) | Native renderer is honest **flexbox-subset** styling, not full cascade |
| M5 Multi-runtime | **M** | freeze ABI v1 + artifact format; **Go** as ABI proof | Elixir = research stretch, out of critical path |
| M6 Ecosystem (L3–L5) | **L** | module resolution, router/state/data, browser-API layer, real-app corpus, **top-100-npm-deps triage table**, "excluded by design" scorecard | Publish the scorecard honestly; name the boundary |
| M7 Production 1.0 | **M** | CLI, artifact packaging, hot reload, docs, semver + IR policy | 1.0 exit = a large existing React repo runs unchanged, no JS runtime |

**Staffing note:** M2 (JS semantics) and M4 (native renderer) are each multi-quarter, multi-engineer workstreams on their own. A solo effort is not credible for those. The staged levels are right; the calendar was the fantasy.

### 6.2 Kill criteria (being explicit saves credibility)

1. **M2 differential pass-rate < 80%** on the curated scenario corpus → stop, re-scope the JS subset (ADR-003's dynamic hooks + ADR-007's GC are the two most likely culprits).
2. **Compile-time static specialization never proves** a warm-path win → do not ship the "specialization" marketing; ship inline-cache/engine tier (ADR-005).
3. **WASM-in-browser startup/memory invert** (see ADR-006) → claim the native-target numbers only, publish the browser numbers separately.
4. **Any renderer produces a different patch stream** on the same input → that renderer is not conformant, not "done".

### 6.3 Sequencing constraints (from PLAN.md, still binding)

1. **No Go/Elixir before the ABI is frozen and proven by Rust.**
2. **No optimization before the conformance suite is green.** (And per ADR-005, when it does start: inline caches first, static specialization last.)
3. **Benchmarks exist before optimization does.**
4. **The runtime never sees source code** (with the gated-eval caveat of ADR-008).
5. **Observable semantics are sacred** — every transform is proven against the trace contract, in CI.

---

## 7. Value-model / engine pitfalls to fix now (not later)

These are the classic engine misses, already visible in the drafts:

1. **Strings UTF-16** (ADR-009) — `"𝕊".length === 2`; Rust `String` is UTF-8. Now.
2. **Number→string** (ADR-009) — Ryu + exponent rules; `-0`/`NaN`/`Infinity`/`1e21`/`1e-7`. Now.
3. **`Value` has no Object/Array/BigInt/Symbol** — fine for M0.1's demo; M0.3 hits object literals (props!) immediately. Sequence the value-model growth honestly.
4. **EventSystem `Box<dyn FnMut(Event)>`** — fix in M0.2, not later: Rust closures can't serialize into an artifact; handlers must be `HandlerId → (component instance, IR continuation)` from the start. (ADR-010 related.)
5. **`Scheduler::schedule` O(n) dedup** — perf debt before the 10k-row benchmark.
6. **Reconciler `HashMap` iteration** — nondeterministic patch order (ADR-010). Ordered traversal now.
7. **`Patch::Insert { index }`** assumes index-stable children; mixed text/element children + comment placeholders shift indices.

---

## 8. What is sound — do not churn

- **Two-worlds separation** and "runtime never sees source" (with the gated-eval caveat).
- **Behavioral compatibility over API-presence**, the compatibility ladder, refusing to claim 100% up front.
- **Patch-stream renderers** — one diff contract for all renderers.
- **State keying to instance + hook position** (once the instance half is fixed per §1.3).
- **ABI-first, multi-runtime-gated** — proving the ABI on a second runtime (Go) after the first is mature.
- **Benchmark-before-optimize**, and the honesty about UI apps not being CPU-bound (the 12 ms → 9 ms example).
- The **self-correcting design loop** — the instance-IDs, patches, and Go-deferral corrections show the process works.

---

## 9. Bottom line

1. Fix the IR shape now: **interlink React IR ⇄ JS IR** and **separate template from instance** — before the compiler grows. That is the highest-leverage change, and the critique's §1 + §2 are correct.
2. Decide the input contract (ADR-001, dual frontend) — it determines the meaning of "arbitrary existing apps."
3. Re-order optimization (ADR-005) and re-frame the performance pitch (ADR-006) so claims are defensible.
4. Write **HOOK_INTEROP.md** before M0.3; the frame protocol is what makes `useState` inside a real custom hook work.
5. Build the **tracing GC** and make **Go** the ABI proof (ADR-007); demote Elixir to research.
6. Re-baseline to **effort bands + kill criteria** and publish the "excluded by design" boundary (ADR-008, §5.3, §6.2).

The architecture is directionally strong. These corrections keep it honest and buildable.
