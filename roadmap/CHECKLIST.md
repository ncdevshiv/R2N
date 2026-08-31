# R2N — Complete Execution Checklist

> **R2N — React to Native** — Native compiler + runtime platform that executes existing React applications with zero JavaScript at runtime
> Generated 2026-08-29 · updated 2026-08-30 after full code re-audit · **56/106** tasks done (53%).
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

`DONE` · weeks 7–12 · progress **18/18** (100%)

_Behavioral compatibility with React core: full hook set, keys, context, effects, class components, error boundaries, portals, Suspense — validated by a behavioral conformance suite, not API presence._

- [x] **P0** — Props & children propagation through component calls (`Value::Children` pre-lowered nodes ride the `children` prop; `ReactNode::Children` splice point; children close over the PARENT's scope — composition by reference; re-renders re-derive splices from fresh props, minimal SetText patches; tests/props_children.rs, 8 tests)
- [x] **P0** — Keys as first-class identity + keyed reconciliation (move vs recreate): author `key` on any static child (host or component, incl. through conditional branches) becomes `k:{value}` identity evaluated in the parent scope; a keyed child that changes position emits `Move`, never Remove+Create, and its component instance (hook state) follows the key — proven by a genuine slot-swap test; `key` never reaches renderers; `diff_children` Move indices fixed from survivor-relative to absolute (tests/keys.rs, 8 tests)
- [x] **P0** — Fragments (`<>...</>` shorthand parses as an empty-tag element; lowers to `ReactNode::Fragment { key, children }`; runtime renders the transparent FRAGMENT host so children splice into the parent — siblings flow around, nested fragments flatten transitively; fragment-child keys are scoped `<parent-seg>:<i>` so spliced children never collide with the parent's positional keys; fragments work as `.map` items (children interleave in item order, keyed siblings survive branch flips); diff computes FLAT renderer positions for fragment siblings — a fragment occupies `children.len()` slots, not one; tests/fragments.rs, 9 tests)
- [x] **P0** — Conditional rendering & lists (map → keyed children): `{cond && <el/>}` / `{cond || <el/>}` lower structurally to If with an empty-fragment "nothing" branch; `{false}`/`{null}` render nothing while `{0}`/`{NaN}` render (React children semantics incl. the `0` footgun); `.get` index access renders as a JSX child; `arr.filter(pred).map(el)` chains evaluate with keyed items; ternary chains act as else-if; conditional unmount destroys hook state (frame-pass staleness detection — a frame absent a full render pass resets on remount) (tests/control_flow.rs, 8 tests)
- [x] **P0** — useReducer — reducer IR + dispatch actions: `HookSlot::Reducer` stores the reducer arrow as IR data (params + body, never a function pointer); `Value::Dispatcher { slot }` carries the frame slot; a dispatch call evaluates `reducer(state, action)` in a fresh env of its params and writes the frame (dirty → flush); multiple dispatches in one handler batch into a single render; per-instance independence; batch dedup test-verified (tests/use_reducer.rs, 6 tests)
- [x] **P0** — useEffect — setup/cleanup/dependency-change lifecycle: parser gains `return expr;` in block-bodied arrows (cleanup spelling, mirrored in the recovery parser); `HookSlot::Effect` stores deps + the armed cleanup (body + captured env); deps change → old cleanup runs BEFORE the new setup (React order, log-verified); `begin_render`/`take_unmounted_cleanups` run armed cleanups ONCE at unmount (frame absent a pass) and disarm them; no-deps runs every render with prior cleanup first; empty-deps runs once; multi-effect order preserved (tests/use_effect.rs, 6 tests)
- [x] **P1** — useLayoutEffect — synchronous pre-commit ordering: same lifecycle mechanics as useEffect (deps + cleanup) with phase separated on `EffectBody.layout` — layout drains inline during the render walk (before the diff), passive after the diff; cleanup carries its effect's phase (a layout cleanup precedes its layout setup in the same queue — caught a bug where it hardcoded passive); handler-captured effects drain post-flush (tests/use_layout_effect.rs, 5 tests)
- [x] **P1** — useMemo / useCallback with dependency-tracked caching: `HookSlot::Memo` (deps + cached value; recompute only when deps changed — deps recorded AT recompute, otherwise every render sees them as changed); `HookSlot::Callback` caches a `Value::Handler` carrying a per-registration identity number (React function identity: stable while deps unchanged, new when they change — observable via effect-dep arrays); pre-existing scheduler bug fixed (a frame dirty before a pass was re-scheduled AFTER it → redundant extra pass → no-deps effects/memos fired twice per change; dirty flags now cleared at pass start); useCallback works as an onClick target (tests/use_memo.rs, 6 tests)
- [x] **P0** — useRef — stable identity across renders: assignment expressions added to the parser/lowering (`target = value`, right-assoc, ident + member targets — twinned recovery parser); `Value::Ref { slot }` box whose `.current` reads/writes the hook-frame slot (same identity every render, writes persist without re-render, no dirty); `effectbodies now resolve the owning component's frame (EffectBody.frame_path) so hook handles inside effect bodies work — before, a throwaway frame broke ref reads in effects (tests/use_ref.rs, 3 tests)
- [x] **P0** — useContext — Context / Provider / Consumer + value propagation: dotted JSX tags (`<Ctx.Provider>`, closes too — parser + recovery twin, previously unsupported); `createContext(default)` returns a `Value::Context { id, default }` handle (default lives on the handle, React contract); `ReactNode::ContextProvider` pushes (id, value) onto a SHARED per-pass context stack (`Env::ctx` Rc — child envs inherit it via `Env::child_of`); `useContext` reads the nearest value else the default; no-op `.map`-only child-call restriction broadened (any value call renders as text); return-validation gate now accepts Fragment/ContextProvider renderables (tests/use_context.rs, 6 tests)
- [x] **P2** — useId: `HookSlot::Id` stores a globally-unique `:rN:` id generated once (atomic counter) — stable across renders of the instance, distinct per call site (slot-indexed), fresh after unmount/remount (slot cleared on frame reset, React behavior); frame-path requirement enforced (tests/use_id.rs, 3 tests)
- [x] **P1** — Class components — state, props, lifecycle methods: `class X extends Component { state = ...; render() {...} }` (parser both twinned; AST Decl::Class; lowering via ClassInfo/ClassMethod — render body becomes the component body, other methods IR blocks); `this` = Map{state, setState (a Setter on the state slot), methods (callable Handler values — call_value now invokes handler values in the current env, enabling this.method()); setState applies + dirty → minimal SetText (verified); shared `setup_class_env` used by BOTH component arm and render_root (a class can be the root — caught by the smoke test); lifecycle: componentDidMount once, componentDidUpdate on re-render, componentWillUnmount armed as an effect cleanup fired once at unmount (tests/class_components.rs, 5 tests)
- [x] **P0** — Error boundaries — capture, fallback, recovery: the component body render is wrapped (Err arm); a class with `getDerivedStateFromError`/`componentDidCatch` captures subtree render errors — derives new state (bound to `err` param, applied via the useState setter to the state slot), runs the catch hook (log-observable), RESETS the frame's hook cursor (`begin_render` same-pass — the mid-pass re-render would otherwise read the willUnmount effect slot as state and re-init to 0 — a real bug the tests caught), rebuilds the class env, re-renders the body (fallback). No boundary → error propagates. `RuntimeError::error_text()` added for the hook's `err` arg (tests/error_boundaries.rs, 4 tests)
- [x] **P1** — Portals — logical parent vs rendering parent: `<Portal target="className">` (special tag in lowering); children render under the FIRST host element with that className — a different RENDERING parent; reconciliation identity and keys follow the LOGICAL position (old portal located by path + key so re-renders reconcile — the naive old=None duplicated content, caught by tests); missing-target renders children at the logical position (no crash); renderer clamps sparse patch indices (portal creates target an external parent whose child count is unknown) (tests/portals.rs, 4 tests)
- [x] **P1** — Suspense — Active → Suspended → Resolved with fallback: `<Suspense fallback={...}>` special tag (IR `ReactNode::Suspense`); `useResource(key)` = a real pending source (`Value::Pending` + resolver Setter; the stored value is read on re-render — first cut always returned Pending, caught by the resolve smoke); the Text arm converts a Pending read into a `RenderedNode::Suspended` marker; the Suspense arm scans RECURSIVELY (a Pending text inside a host child — deep suspension) and swaps the whole subtree for the fallback; resolve → single SetProp+SetText (no duplicate trees, zero Remove/Create — test-verified); resolved state sticks across unrelated re-renders; per-instance boundaries independent (tests/suspense.rs, 4 tests)
- [x] **P1** — StrictMode dev-only semantics kept out of production artifacts: `ReactNode::StrictMode` (transparent wrapper) + `RuntimeTemplate.strict_mode` flag (serde default false — absent in production); `lower_dev` (keeps node + sets flag) vs `lower` production (STRIPS the node — the stripping is test-verified on the serialized JSON); runtime: dev artifacts double-invoke BOTH layout and passive effects (setup → cleanup → setup, log-verified) (tests/strict_mode.rs, 4 tests)
- [x] **P0** — Conformance suite v1 — behavioral tests (observable behavior, not API presence): `tests/conformance.rs` consolidates ten CONF-NN checks, each pinning ONE React semantic via rendered-tree/patch-stream observations (minimal patches, keyed identity, parent-scope children, context propagation, effect cleanup ordering, error-boundary capture, suspense fallback, class this/setState/lifecycle, portal rendering parent, StrictMode dev-vs-prod) — assertions are behavior-first (no API-presence checks) (tests/conformance.rs, 10 tests)
- [x] **P2** — react_compatibility_version recorded per artifact: `ArtifactManifest.react_version` (18.2.0 — the React semantics level this artifact implements) stamped with format/compiler versions in `RuntimeTemplate::new`; round-trips through the artifact JSON (tests/lower.rs manifest assertions updated)

## M2 — JavaScript Compatibility — Level 2

`IN PROGRESS` · weeks 12–20 · progress **2/15** (13%)

_Full ECMAScript semantics in the compatibility engine: closures, classes, prototypes, coercion, exceptions, promises, generators, modules — the layer that makes arbitrary React code actually run._

- [x] **P0** — Full value model: Undefined/Null/Boolean/Number/BigInt/String/Symbol/Object/Function/External: `Value` gains Undefined (keyword literal), BigInt(i64), Symbol(id/key — identity-distinct, `Symbol(key)` builtin), Object (shared mutable property bag — `Object()` builtin, member get/set/index, missing prop → undefined), Function (first-class: arrows assigned to bindings are callable with param binding, missing arg → undefined), External (opaque handle); ECMA ToBoolean (undefined/null/±0/NaN/""/0n falsy) and ToNumber (undefined→NaN, null→0, bool→0|1, string parse); `typeof` builtin ("undefined"/"bigint"/"symbol"/"object"/"function"/"external"); Closure eval now yields a real Function value (was Null) (tests/value_model.rs, 7 tests)
- [x] **P0** — Objects — dynamic properties, prototypes, shape-friendly layout: `Value::Object(Rc<RefCell<ObjData>>)` — own props + `proto` link; reads walk the prototype chain (missing → undefined); writes create own data props (shadowing); `Object.create(proto)` / `Object.create(null)` (no proto), `Object.getPrototypeOf(o)`, `o.__proto__` read (null when none) and write (sets/swaps the link, null clears); `Object()` constructor (own empty bag, per-object isolation); `__proto__ = p` assignment validated (object-or-null). Prototype chain now a real walk, not a flat bag (tests/prototypes.rs, 6 tests)
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
