# R2N — Complete Execution Checklist

> **R2N — React to Native** — Native compiler + runtime platform that executes existing React applications with zero JavaScript at runtime
> Generated 2026-08-29 · updated 2026-08-30 after full code re-audit · **36/106** tasks done (34%).
> Audit basis: every task was verified against the actual implementation and test suite (54 tests green, clippy clean, CLI verified end-to-end on all examples), not against earlier claims.

**How to use:** check items off in the interactive tracker ([index.html](index.html)) — it saves live in your browser. This file, [roadmap.yaml](roadmap.yaml), and [roadmap.toml](roadmap.toml) are the portable record; update them when a milestone closes.

**Legend:** **P0** = critical path, blocks everything downstream · **P1** = important for milestone exit · **P2** = valuable, can slip

---

## M0.1 — Foundation — Workspace & Vertical Slice

`DONE` · weeks 1–2 · progress **13/13** (100%)

_Rust workspace, IR data models, runtime skeleton, memory renderer, and the first compiler pipeline: source → parser → AST → React IR → Runtime IR → runtime → memory renderer._

- [x] **P0** — Project charter locked: three pillars (React compatibility, zero-JS runtime, specialization)
- [x] **P0** — Spec JS_IR.md v0.1 — value model, instruction set, closures, modules, async, exceptions, optimization metadata
- [x] **P0** — Spec REACT_IR.md v0.1 — components, elements, hooks, effects, reconciliation, keys, context, suspense, portals
- [x] **P0** — Spec RUNTIME_ABI.md v0.1 — handles, node/state/component ops, scheduler, capabilities, errors, versioning
- [x] **P0** — Rust workspace: 7 crates (ast, parser, ir [JS+React+Runtime IR as modules], runtime, renderer-memory, compiler, cli) — the 8-crate split was consolidated; module boundaries preserve the separation
- [x] **P0** — crates/ast — Expr / BinOp / UnOp / Literal / Element / Prop / Program / Stmt models
- [x] **P0** — crates/ir — JsExpr (JS IR), RuntimeComponent/RuntimeTemplate (Runtime IR), serde-serializable artifact
- [x] **P0** — React IR — ReactNode::{Host, Component, If, List, Text} + ComponentRef + keyed list model
- [x] **P0** — crates/runtime — zero-JS evaluator + Renderer trait + keyed reconciler + event dispatch
- [x] **P0** — crates/renderer-memory — MemoryRenderer with node/children stores + XML serialization
- [x] **P0** — crates/compiler — compile_source(): parse → lower → RuntimeTemplate (+ JSON round-trip verified)
- [x] **P0** — E2E counter tests green — initial tree + click-driven updates (button + text in memory renderer)
- [x] **P2** — Architecture guard — `r2n-runtime/tests/architecture.rs` fails the build if runtime gains a parser/ast/compiler dep

## M0.2 — Reactive Runtime Loop

`DONE` · weeks 2–4 · progress **14/14** (100%)

_The core reactive loop: event → state mutation → dirty component → scheduler → render → reconcile → patch → renderer. State keyed by (ComponentId, StateSlot); deterministic FIFO scheduler; minimal-diff reconciler._

- [x] **P0** — Reactive loop design locked: event → state → dirty → scheduler → render → diff → patch
- [x] **P0** — RenderedNode rendered tree + prev-tree diffing (host/text/fragment nodes)
- [x] **P0** — HookFrame StateStore: slots keyed by (instance path, slot index), persist across renders
- [x] **P0** — Scheduler: deterministic FIFO queue with dedup (`scheduler.rs`; batched-setter E2E: intermediate states never render)
- [x] **P0** — EventSystem: handlers keyed by (NodeId, event); Runtime::dispatch runs handler then flushes
- [x] **P0** — ComponentInstance: FrameStore keyed by instance path; two-instance independence tested
- [x] **P0** — Patch enum: Create / CreateText / SetProp / SetText / Remove / Move
- [x] **P0** — Reconciler v0: keyed (type, key, path) diff, minimal Vec<Patch> (one SetText per click proven)
- [x] **P0** — Renderer trait: apply(&[Patch]) — one patch stream for all renderers
- [x] **P0** — Runtime::flush() core loop: render → reconcile → dirty check → swap tree (guarded)
- [x] **P0** — Counter E2E: click → SetText("1") with no parent recreation (test-verified)
- [x] **P0** — Two-instance test: Counter A and Counter B hold independent state (test-verified)
- [x] **P0** — All 14 M0.2 acceptance criteria green (`tests/acceptance_m02.rs`: mount, unmount, minimal updates, batching, identity stability, keyed reorder/append/removal, instance independence, props, effect lifecycle, conditional swap, error determinism, patch-stream determinism)
- [x] **P1** — Todo app E2E through the full reactive loop (click → keyed list append, minimal patches)

## M0.3 — Compiler Frontend — JS/JSX → IR

`DONE` · weeks 4–7 · progress **9/9** (100%)

_Stop hand-building IR. Lexer → parser → AST → JS IR → React IR, and the first genuinely interesting transformation: compiling Counter from real source, including event-handler extraction._

- [x] **P0** — Lexer: JS tokens with line:col + byte offsets, JSX-text rescanning, comments (line/block/nested)
- [x] **P0** — Parser: precedence-climbing expressions, ternary/if-else, arrays, member/call/index, arrows (expr + block body), JSX elements/attrs/children (expr, nested, raw text)
- [x] **P0** — AST → JS IR lowering (JsExpr: Lit/Var/Get/Index/Bin/Un/Call/Closure/Array/Block/If)
- [x] **P0** — JS IR → React IR: component table, JSX → ReactNode, children.map → keyed List node, free-var captures
- [x] **P0** — Handler extraction: onClick closures carried as serializable Handler values (inst path + closure); dispatch executes them — ReadState/Add/WriteState instruction lowering is deferred to M3 specialization
- [x] **P0** — compile(source) → RuntimeTemplate artifact API (serde JSON) — **remaining**: format/ABI version stamps in the artifact
- [x] **P0** — Counter compiled from real source E2E — no manually constructed IR anywhere in tests
- [x] **P1** — Diagnostics: friendly multi-error reporting with recovery (`parse_with_recovery`: statement/declaration re-sync, one pass reports all errors), `TokenKind::describe()` names (`` `;` `` not `Semicolon`), rendered carets (`ParseError::render`), CLI prints every diagnostic; recovering parser proven AST-identical to the strict parser on valid sources (tests/diagnostics.rs, 11 tests)
- [x] **P1** — IR determinism (snapshot foundation) + artifact manifest stamps: format_version + compiler_version, JSON round-trip verified

## M1 — React Compatibility — Level 1

`PLANNED` · weeks 7–12 · progress **0/18** (0%)

_Behavioral compatibility with React core: full hook set, keys, context, effects, class components, error boundaries, portals, Suspense — validated by a behavioral conformance suite, not API presence._

- [ ] **P0** — Props & children propagation through component calls
- [ ] **P0** — Keys as first-class identity + keyed reconciliation (move vs recreate)
- [ ] **P0** — Fragments
- [ ] **P0** — Conditional rendering & lists (map → keyed children)
- [ ] **P0** — useReducer — reducer IR + dispatch actions
- [ ] **P0** — useEffect — setup/cleanup/dependency-change lifecycle
- [ ] **P1** — useLayoutEffect — synchronous pre-commit ordering
- [ ] **P1** — useMemo / useCallback with dependency-tracked caching
- [ ] **P0** — useRef — stable identity across renders
- [ ] **P0** — useContext — Context / Provider / Consumer + value propagation
- [ ] **P2** — useId
- [ ] **P1** — Class components — state, props, lifecycle methods
- [ ] **P0** — Error boundaries — capture, fallback, recovery
- [ ] **P1** — Portals — logical parent vs rendering parent
- [ ] **P1** — Suspense — Active → Suspended → Resolved with fallback
- [ ] **P1** — StrictMode dev-only semantics kept out of production artifacts
- [ ] **P0** — Conformance suite v1 — behavioral tests (observable behavior, not API presence)
- [ ] **P2** — react_compatibility_version recorded per artifact

## M2 — JavaScript Compatibility — Level 2

`PLANNED` · weeks 12–20 · progress **0/15** (0%)

_Full ECMAScript semantics in the compatibility engine: closures, classes, prototypes, coercion, exceptions, promises, generators, modules — the layer that makes arbitrary React code actually run._

- [ ] **P0** — Full value model: Undefined/Null/Boolean/Number/BigInt/String/Symbol/Object/Function/External
- [ ] **P0** — Objects — dynamic properties, prototypes, shape-friendly layout
- [ ] **P0** — Closures & lexical environments with correct capture semantics
- [ ] **P0** — Classes, this, new, prototype chain
- [ ] **P0** — Equality & coercion — == vs ===, ToPrimitive, ToString/ToNumber
- [ ] **P0** — Exceptions — try/catch/finally propagation across calls
- [ ] **P0** — Promises + async/await with scheduler-driven continuations
- [ ] **P1** — Generators & iterator protocol
- [ ] **P0** — Modules — import/export/dynamic import + initialization order
- [ ] **P1** — Destructuring, spread, rest
- [ ] **P2** — Symbols, Proxy, Reflect
- [ ] **P2** — RegExp support (embed engine or implement subset)
- [ ] **P0** — GC strategy (tracing / refcount / hybrid) honoring observable lifetime semantics
- [ ] **P2** — TypeScript type consumption → optimization hints only (semantics stay JS)
- [ ] **P0** — test262-subset conformance harness + published compatibility score

## M3 — Optimization Pipeline — Specialization

`PLANNED` · weeks 20–26 · progress **0/10** (0%)

_Make unnecessary JavaScript semantics disappear when static analysis proves they aren't needed: analyzer → specialization → dual-tier execution, with a benchmark harness from day one._

- [ ] **P0** — Static analyzer — purity, mutability, escape, constant, shape hints
- [ ] **P0** — Specialization — dynamic op → typed field op when proven safe (user.name vs obj[key])
- [ ] **P0** — Dual-tier execution — fast path + compatibility fallback path
- [ ] **P1** — Component specialization — known component graph → specialized native IR
- [ ] **P0** — Benchmark harness — startup, first render, update latency, reconcile, memory, binary size
- [ ] **P0** — 10-app benchmark corpus (counter → 10k rows → real apps)
- [ ] **P0** — Baseline comparisons: reference React vs compatibility runtime vs optimized runtime
- [ ] **P1** — Target Tier 1 met: startup 1.5×, memory −10–20%, UI ≈ parity
- [ ] **P1** — Target Tier 2 met: startup 3–5×, memory −30–50%, UI 2–5×, CPU 2–10×
- [ ] **P2** — Target Tier 3 met: startup 5–10×, memory −50–70%, UI 3–10×, CPU 5–20×+

## M4 — Renderers — Native, WASM, Terminal

`PLANNED` · weeks 26–32 · progress **0/5** (0%)

_Every renderer consumes the same patch stream. Native widgets for real platforms; WASM artifact for the browser with zero JS at runtime; terminal as the cheap integration target._

- [ ] **P0** — Renderer conformance tests — identical patch stream verified on every renderer
- [ ] **P0** — Native renderer — real platform windows/widgets (component → runtime tree → native widget)
- [ ] **P0** — WASM renderer — browser artifact with zero JavaScript at runtime
- [ ] **P2** — Terminal renderer — cheap end-to-end integration target
- [ ] **P0** — Per-platform event normalization into runtime Event

## M5 — Multi-Runtime — Go & Elixir

`PLANNED` · weeks 32–38 · progress **0/6** (0%)

_Freeze Runtime ABI v1, then prove the artifact is language-independent: Go and Elixir runtimes passing exactly the same conformance suite as Rust._

- [ ] **P0** — Freeze Runtime ABI v1 — ops, handles, errors, capabilities, discovery
- [ ] **P0** — Artifact format spec — manifest + ABI version + IR + resources/assets
- [ ] **P0** — Go runtime passing full conformance suite
- [ ] **P1** — Elixir runtime passing full conformance suite
- [ ] **P0** — Cross-runtime CI — same artifact executes on all runtimes
- [ ] **P1** — Hot replacement — ReplaceComponentImplementation + deterministic state migration

## M6 — Ecosystem Compatibility — Levels 3–5

`PLANNED` · weeks 38–50 · progress **0/10** (0%)

_The hard part: npm-package and browser-API compatibility. Module resolution, router/state/data/UI/animation libraries, real-app corpus, and a published compatibility scorecard._

- [ ] **P0** — Module/package resolution strategy (node_modules mapping, import maps)
- [ ] **P1** — react-router compatibility
- [ ] **P1** — State libraries — Redux, Zustand
- [ ] **P1** — Data-fetching — TanStack Query subset
- [ ] **P2** — Form libraries — React Hook Form subset
- [ ] **P2** — UI libraries — MUI / Ant Design render subset
- [ ] **P2** — Animation libraries — framer-motion subset
- [ ] **P0** — Browser API layer — events, timers, fetch, storage
- [ ] **P0** — Real-app corpus — TodoMVC, RealWorld, dashboard, 10k-row table
- [ ] **P0** — Compatibility scorecard — React % / JS % / Web API % / Ecosystem %

## M7 — Productionization — 1.0

`PLANNED` · weeks 50–56 · progress **0/6** (0%)

_Ship it: CLI, artifact packaging, state-preserving hot reload, docs, semver policy — and the 1.0 exit test: a large existing React repo runs unchanged with no JavaScript at runtime._

- [ ] **P0** — CLI — r2n build / run / check
- [ ] **P0** — Production artifact packaging — runtime + compiled app + assets + config
- [ ] **P1** — Dev workflow — hot reload with state-preserving component replacement
- [ ] **P1** — Docs — guides, IR/ABI specs, tutorials
- [ ] **P1** — Semver + IR forward-compatibility policy
- [ ] **P0** — 1.0 exit test — large existing React repo runs unchanged, zero JS at runtime

---

## Completion rules

- A milestone is **complete only when its acceptance tests pass** — never because an API "exists".
- Compatibility claims come from the conformance suite (behavioral), published as percentages.
- Never optimize before the current compatibility layer is correct; never add a second runtime (Go/Elixir) before the ABI is proven by the first.
- Observable semantics are sacred: optimization may transform behavior only when equivalence is proven.
