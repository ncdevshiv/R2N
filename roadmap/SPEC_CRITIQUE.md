# R2N — Spec Critique (Adversarial Review)

**Reviewed:** the JS_IR / REACT_IR / RUNTIME_ABI drafts, the M0.1–M0.2 working designs and code sketches, and the generated roadmap (ROADMAP.md, PLAN.md, CHECKLIST.md).
**Date:** 2026-08-29
**Method:** assume each claim is false until the spec proves it true; hunt for the app that breaks it.

---

## 0. Verdict

The architecture is **directionally sound** — the two-worlds separation, behavioral (not API-presence) compatibility, version stamping, the patch stream, and the ladder are all good engineering instincts, and several usual failure modes are already fenced off (premature multi-runtime, premature optimization, benchmark theater).

But the spec as written has:

- **2 showstopper design flaws** that will cause rework if not fixed before M0.3: the pipeline model misrepresents the *shape* of the problem (React IR and JS IR are interleaved, not sequential), and the IR conflates *compile-time template* with *runtime instance*.
- **1 strategic gap** the spec never confronts: what the compiler actually takes as input (raw source vs the app's own bundler output) — this decision dominates cost, timeline, and the meaning of "arbitrary existing apps."
- **1 honesty problem** at the heart of the pitch: implementing full ECMAScript semantics *is* building a JavaScript engine; the performance table doesn't survive that admission without an inline-cache tier.
- **~10 concrete bugs/holes** already visible in the drafted code and IR (string encoding, number formatting, reconciler determinism, Rust-closure event handlers that can't cross the ABI, missing React 18 hooks, eval, GC ownership, microtask ordering…).
- **A timeline that is off by a factor of 3–5×** in the middle milestones.

Each finding below carries severity, reasoning, and a concrete fix. Section 18 maps everything to roadmap edits.

Severity legend: **[BLOCKER]** wrong shape, must change before building further · **[MAJOR]** will cause production failure or months of rework · **[MODERATE]** real cost or credibility hit if unaddressed · **[MINOR]** fix cheaply now.

---

## 1. [BLOCKER] The three-IR pipeline misrepresents the problem's shape

The spec draws lowering as a clean sequence:

```
JS IR  →  React IR  →  Runtime IR
```

That is not what a React application is. A React component is a **JavaScript function that calls React APIs**. Concretely:

```jsx
function Dashboard({ data }) {
  const processed = useMemo(
    () => data.filter(x => x.score > 10).map(x => x.label).sort(),  // pure JS
    [data]
  );
  const [open, setOpen] = useState(false);                          // React
  return open ? <Panel rows={processed} /> : null;                   // both
}
```

The `useMemo` callback is arbitrary JS that runs on **every render, in the hot path**. So are event handlers, reducer bodies, effect callbacks, `React.memo` comparators, and context default-value computations. React IR cannot "lower from" JS IR the way a compiler lowers from an AST — instead, **React IR nodes must embed references back into JS IR function bodies** (render functions, handlers, comparators, cleanups). The real dataflow is bidirectional:

```
React IR  ⇄  JS IR     (React nodes contain JS closures; JS calls back into React)
     ↓
Runtime IR executes BOTH, with a defined calling convention between them
```

**What goes wrong if unfixed:** the team builds M0.3's sequential lowering, hits the first real component with a `useMemo`, and discovers the IR needs a closure/continuation reference type that doesn't exist in any of the three specs — then rewrites the IR and the compiler. This is a guaranteed mid-milestone architecture change.

**The missing spec — the frame protocol.** React itself solves "which component am I in / which hook am I at" with a module-global current-dispatcher + current-fiber. R2N needs the equivalent as an explicit contract, because `useState` is a call **inside interpreted JS IR** that must find native runtime state. Required decisions:

1. When the JS interpreter executes the component's body and hits `Call useState(initial)`:
   - how does the interpreter know the current `ComponentInstanceId` and hook index (a runtime frame-register? an interpreter-frame ↔ instance binding table?),
   - and who allocates the slot on first call vs. reads on later calls?
2. What is the **calling convention** when the native reconciler must call a JS IR closure (render, comparator, cleanup)? Value marshalling, error propagation across the boundary, re-entrancy (a render triggers a state update that schedules another render)?
3. Where does hook **indexing** happen — compile-time static slots, or runtime dynamic indices?

Point 3 is a spec hole with real consequences: the drafted `CreateState { slot }` assumes the compiler can statically assign hook slots. It cannot in general. Hooks are reached through arbitrary call chains — custom hooks (`useDebounce`, `useQuery`, every react-query hook internally calls 5 more), conditionally (rule violation but common), and via dynamic dispatch (`const hook = useAuth; hook()`). Static slot assignment is a whole-program analysis with dynamic-dispatch escape hatches. You need **React's own fallback**: dynamic hook-index-at-runtime with an "invalid hook call" diagnostic when the frame protocol is violated.

**Fix:** add a fourth spec, `HOOK_INTEROP.md` (or fold into REACT_IR): the interpreter-frame ↔ component-instance binding, the native ⇄ IR calling convention, and dynamic hook indexing. Rename the pipeline diagram from "lowering" to "extraction + interlink." Add roadmap tasks for dynamic hook resolution and closure-as-IR-value.

---

## 2. [BLOCKER] Compile-time template vs runtime instance conflation

The drafted IR bakes runtime facts into compile-time data:

- `ReactNode::Element { id: NodeId(1), ... }` — **NodeIds assigned at compile time** to what are actually template positions.
- `StateStore` keyed `(ComponentId, StateSlot)` where `ComponentId` identifies the *definition* — M0.2's own correction text then proposes `CreateState { component: ComponentInstanceId }`, which is the same mistake inverted: **instance IDs don't exist at compile time either.**

Both errors come from one missing distinction:

```
Artifact (compile time)   =  TEMPLATES: component definitions, hook slot layouts,
                             element skeletons with template-internal IDs
Runtime (execution)       =  INSTANCES: allocated per mount; instance-scoped state,
                             runtime node handles, runtime event registrations
```

**What goes wrong if unfixed:** the first `{items.map(item => <Row key={item.id} />)}` — where node count and identity are runtime-determined — cannot be represented at all. Static NodeIds only work while every demo has a fully static tree shape. The reconciler sketch compounds it by diffing on NodeId, which is exactly the identity React does *not* use: real reconciliation matches children by **(element type + key + structural position)**, because that's what survives a re-render.

Related gaps in the drafted data model:

- No **fragment**, no **component-as-child** slot in `ReactNode` (so two `Counter` instances in one tree — M0.2's own acceptance criterion #4 — has no representation).
- No handling of **false/null/undefined children** (React drops them entirely — not text nodes) and no placeholder concept (React inserts empty comment nodes in the DOM to stabilize positions; renderers must mirror this or sibling indices shift).
- No array-children with keys.
- Reconciler v0 iterates a `HashMap`, so **patch order is nondeterministic across runs** — but patch order is observable (DOM insert order, effect ordering) and will make tests flaky and behavior unrepeatable. Use an ordered traversal.

**Fix:** in the IR, template IDs become `TemplateNodeId` valid only within a component's skeleton. The runtime allocates `NodeHandle`s per instance; the Patch stream carries only runtime handles. Reconciliation keys: (type, key, position). State keys: `(ComponentInstanceId, SlotIndex)` where instance IDs are runtime-allocated and the artifact references the *template* + slot layout. One clean rule fixes four drafted bugs.

---

## 3. [BLOCKER] The strategic fork the spec never states: source-in vs bundle-in

"Arbitrary existing React applications" silently assumes the compiler consumes the app's **raw source tree**. That path requires R2N to reimplement:

- the **bundler** (path aliases, CSS imports, asset URLs, code splitting, tree shaking),
- the **build-plugin ecosystem** — Vite/webpack/Babel plugins are *JavaScript that executes at build time* (svgr turns SVGs into components, styled-components compiles the css-prop, macros expand at build). An "arbitrary" app's build pipeline is a JS program R2N cannot run without shipping Node at build time,
- **CommonJS/ESM interop** with all its bundler-specific observable differences (`__esModule` interop differs between Webpack and ESM; "arbitrary apps" behave per *their* bundler's rules),
- `process.env.NODE_ENV` inlining — **React itself and most libraries branch on it** (dev warnings, prop-types, StrictMode double-invocation). Without const-folding it, the dev/prod behavior fork is unimplementable,
- `import.meta`, the `browser` field, UMD wrappers, `new URL(..., import.meta.url)`, dynamic remote imports.

There is a second input contract the spec never considers: **compile the app's own bundler output** (standard ESM bundle). This kills the plugin/CJS/alias problem entirely — imports are resolved, NODE_ENV is inlined, assets are hashed — and it is how you actually reach "arbitrary existing apps" in this decade. The cost: JSX is already compiled to `React.createElement`/`jsx()` calls (extraction still works — it's pattern-matching on the jsx runtime, exactly what React DevTools does), and TS types are erased (losing type-driven optimization hints, which only matters to the specialization tier anyway).

**Recommendation — dual frontend, shared everything downstream of JS IR:**

```
                    ┌─ Compat frontend:  app's bundler output (ESM) ──┐
Existing app  ─────►│                                                    ├─► JS IR → …
                    └─ Opt frontend:    raw TSX source (greenfield)  ─┘
```

- The **compatibility program** (the "100% arbitrary apps" claim) uses bundle-in. This is the only credible route to the claim.
- The **specialization program** (the performance story, new apps) uses source-in for full static signal.
- The pipeline, IRs, ABI, and runtimes are identical below JS IR — nothing already designed is wasted.

Without deciding this explicitly, the project will discover it mid-M6 as "why is the compiler broken on every real repo," which is where compatibility programs die.

---

## 4. [MAJOR] The "no JavaScript engine" framing collides with the perf table — fix with an inline-cache tier

The spec's own hard truth (option C): existing React apps are JS programs; something must provide JS semantics. Implementing closures, prototypes, coercion, `Proxy`, async, generators, and GC **is implementing a JavaScript engine** — the only thing missing is the word "engine." Consequences the spec waves at but doesn't internalize:

1. **Dynamic JS is unavoidable and it's not rare.** Data transforms, lodash, date libs, immer produce structural clones, form-validation engines — real app logic. In the compatibility tier it all runs on *your* interpreter.
2. **A plain bytecode interpreter without tier-up loses to V8 by 10–50× on hot code.** The performance table's "UI updates 1.2–5× faster" implicitly assumes component bodies are statically specializable or cold. That's an empirical claim nobody has tested for this architecture — and the counter-case is easy to construct (a dashboard doing heavy client-side aggregation).
3. The M3 plan has it backwards: it leads with **compile-time static specialization** ("user.name → LoadField") and treats the interpreter as the fallback. Compile-time specialization over arbitrary JS needs whole-program proof (`user.name` is only a field load if `user` is provably a plain object with a data property — a getter or Proxy makes it arbitrary code; the sketch's own example is unsound as stated without that proof). Inline caches get most of the win **self-guarding, always soundly, without proofs**.

**Fix:**

- Re-order M3: **(a) shapes (hidden classes) + inline caches + fast paths in the compatibility interpreter** — the proven baseline-tier technology in every JS engine — then **(b) profile-guided tier-up** to specialized code, then **(c) static specialization** last, with a formal "observable behavior" definition (the trace contract: renderer patches, effects, console, timers, network, event order — everything else is free to change) that every optimizer transform must be proven against.
- Split the published targets into **native** vs **wasm-in-browser** columns (see §9 for why browser startup/memory claims don't transfer).
- Reword pillar 2 for honesty (see §10): the defensible claim is "no *application* JavaScript; the ECMAScript semantics live in our native runtime instead of V8's." The current phrasing invites the correct rebuttal: "you built a JS engine, without a JIT."

---

## 5. [MAJOR] Event-handler lowering as sketched is semantically wrong (stale closures)

The sketched M0.3 transformation:

```
onClick={() => setCount(count + 1)}   →   ReadState(0) → Add 1 → WriteState(0)
```

reads state **at event time**. JavaScript closure semantics say the handler reads `count` **as captured at render time**. For the Counter they coincide. They do not in general:

```jsx
onClick={async () => { await save(); setCount(count + 1); }}
```

Here `count` is frozen at render time across the `await`. The sketched lowering silently changes the program's observable behavior — the exact sin the spec forbids ("optimization must never change observable semantics"), committed in the compiler's very first transformation.

Worse patterns follow the same rule: handlers stored and invoked later, multiple handlers from different renders alive simultaneously, `useEffect` cleanups capturing old props, `useCallback`-memoized handlers intentionally holding stale values (the classic "you're reading stale state" bug is *load-bearing* in real apps).

**Fix:** the only safe lowering is the faithful one — **each render materializes a capture environment** (a render snapshot: the values the closures close over), and handlers read from the snapshot, never from live state. Live-state reading becomes a *provable optimization* later (safe only when the slot cannot change between render and dispatch — the analysis M3 wants anyway). Add a runtime IR concept: `CaptureEnvironment` / render-snapshot, and a conformance test bank of stale-closure scenarios (this is a top-10 source of real-world React bugs, so behavior parity here is visible to users immediately).

---

## 6. [MAJOR] Scheduling, microtasks, and the React 18 model are unspecified — and this is observable everywhere

The RUNTIME_ABI lists five priorities. That is not a scheduling model; it's an enum. Observable behavior in React 18/19 depends on:

- **Automatic batching** across event handlers, promises, timeouts (React 17 didn't batch in async contexts — the version-stamp field exists, but only one behavior is ever planned; pick and test per version),
- **Microtask-before-task ordering**: promise continuations must drain before the next task; render flushes are scheduled as a task, which is *why* a chain of `setState` in `.then()` batches into one render. Get the interleaving wrong and react-query-class libraries visibly misbehave,
- **rAF vs layout effects vs passive effects** ordering: layout effects run synchronously at commit; passive effects run in a later task; the browser paint sits between them — animation libraries (framer-motion) lean on this precisely,
- **Interruption semantics**: `startTransition` renders are discardable-and-restartable when higher-priority state arrives — that's not a priority level, it's a different execution model,
- **Tear rules** for external stores during concurrent rendering (see `useSyncExternalStore`, §7).

**Fix:** write `SCHEDULING.md` with explicit ordering tables (task queue, microtask queue, rAF, commit phases) and gate M1 on a **differential ordering test bank**: "in what order do these logs appear" scenarios run against real React and R2N, diffed. This harness, not a checklist of hooks, is the real M1 exit criterion (see §14).

---

## 7. [MAJOR] React 18/19 hooks are missing from M1 — including the one the whole ecosystem routes through

The M1 task list stops at `useId` and pre-18 concepts. Missing, all load-bearing in real apps:

- **`useSyncExternalStore`** — *the* integration hook of React 18. Zustand, Redux, Jotai, Recoil, Valtio, MobX-react, react-query's subscription core: all route through it. It also carries tear-detection semantics under concurrent rendering. Without it, M6's "state libraries" line is unimplementable, not just incomplete.
- **`useTransition` / `useDeferredValue`** — the concurrent model's actual API surface.
- **`useOptimistic`, `useActionState`/`useFormState`, `use()`** — React 19 surface; if `react_compatibility_version = 19` is the stamp, these are in scope by the spec's own rule.
- **Concurrent lifecycle**: retry lanes, `Offscreen`/Activity semantics, hydration errors in the React 19 model.

Also decide and document the negative scope: **legacy context API** (removed in 19 — fine to exclude if version 19 is the target; must be stated), `react-reconciler`-based custom renderers (see §8).

**Fix:** expand M1's task list with a versioned **React API table** — every exported symbol of `react` and `react-dom`, status per version, and behavioral conformance test references. This table is also the only credible basis for the "React Compatibility 99.x%" number the ladder promises.

---

## 8. [MAJOR] The closed-world model has known-impossible ecosystem members — say so or the scorecard is a lie

- **react-three-fiber**, react-pdf, react-three renderers: they build custom renderers *on top of* `react-reconciler`. R2N's runtime replaces the reconciler; these libraries are **incompatible by design**, not by shortfall. Same family: anything reaching into `__SECRET_INTERNALS` or `react-dom/test-utils`.
- **styled-components / emotion**: inject `<style>` elements into `document.head` at runtime, rely on the CSS cascade. Works in a browser renderer; **meaningless in a native renderer** (there is no cascade — see §9).
- Libraries leaning on `Element` measurement (`getBoundingClientRect`), `IntersectionObserver`, `ResizeObserver`, scroll position — need real implementations in the browser-API layer, or virtual-list libraries break.

**Fix:** the M6 scorecard needs an **"excluded by design"** category with reasons, published up front. A compatibility number that silently omits impossible cases will be — correctly — called a lie when someone tries react-three-fiber and it fails. Naming the boundary is a strength, not a weakness.

---

## 9. [MAJOR] The native renderer is the second-most under-scoped item on the roadmap — because of CSS

M4's line "native renderer — real platform windows/widgets" hides a multi-year subsystem: **virtually every real React app is styled with CSS**, and native widgets have no cascade, selectors, specificity, pseudo-classes, or media queries. A native renderer with real-world visual fidelity means implementing (or integrating) a CSS engine, flexbox/grid layout, text shaping, IME, and accessibility — each a subsystem on its own.

Meanwhile the WASM renderer has its own unstated constraint: **WASM cannot touch the DOM.** Every DOM operation needs JS glue (there is no wasm-native DOM interface, WasmGC or not). So pillar 2's literal "zero JavaScript at runtime" is **unachievable for the browser target specifically**. Also: shipping a Rust runtime + interpreter + app as wasm is realistically 2–5 MB against a typical React bundle's ~200 KB, with wasm compile time on top — the published "startup 2–10× faster" and "memory −20–70%" claims almost certainly **invert** for the in-browser case (V8 streams-parses gzipped JS very fast; wasm download+instantiate competes poorly). Native targets (no browser) are where those numbers are plausible.

**Fix:**

1. **Reword pillar 2** to the defensible claim: *"zero application JavaScript; a fixed, auditable platform shim (browser target) and none at all (native target)."* Ideological purity here will stall M4 for a quarter.
2. **Sequence M4 browser-first**: WASM-in-browser gets 90% visual fidelity immediately by reusing the browser's CSS/layout engine behind the shim; native is where the perf ceiling is.
3. For the native renderer, **integrate, don't write**: `taffy` (flexbox, used by Dioxus), `cosmic-text`/harfbuzz (text), `accesskit` (native accessibility). And scope it honestly: a **flexbox-subset** styling model (no cascade/specificity) or an explicit "native renderer supports the Tailwind-style layout subset" contract.
4. Split the performance-target table into native vs wasm-in-browser columns with independent numbers.

---

## 10. [MAJOR] `eval` / `new Function` contradict the runtime boundary — decide now

The boundary rule: "the runtime never sees source code — no parser dependency." ES semantics include `eval` and `new Function`. Plenty of library code uses them (template engines, older JSON-schema validators, some i18n pipelines). Three options, all with costs:

1. **Support at runtime** — requires a parser *inside* the runtime, breaking the boundary rule and bloating every artifact.
2. **AOT-lift eval'd strings where statically visible** — handles constant-string eval (the common case), fails on dynamic strings.
3. **Declare out of scope for L2/L3**, with a clear diagnostic and a documented escape hatch (e.g., a capability-gated host eval service).

**Fix:** pick 2+3 now and write it into JS_IR.md. The same section should settle the adjacent boundary cases the spec is silent on: dynamic `import()` of remote URLs, `WebAssembly.compile` on the web target, and the security story for the capability model (the capability list exists in the ABI; nothing audits that the compat tier can't bypass it — add a capability-leak review task to M2).

---

## 11. [MAJOR] GC ownership is the hidden multi-runtime killer — and it's also a Rust workstream the roadmap doesn't have

JS object graphs are **cyclic by construction** (DOM node ↔ handler closure ↔ state ↔ component ↔ back), so reference counting leaks, and arena allocation can't know JS lifetimes: the compatibility tier needs a real **tracing GC**. The spec says "runtime may use tracing/refcount/arena" as if these were interchangeable — they are not, and the choice exposes the multi-runtime plan's weakest seam:

- **Rust**: no host GC — you must write one (mark-sweep at minimum; incremental/generational to respect the 16 ms frame budget; that's a serious workstream on its own, absent from the roadmap).
- **Go**: host GC exists — fine.
- **Elixir/BEAM**: shared-nothing process model is *fundamentally at odds* with a shared mutable JS object graph; every cross-boundary object access becomes message-passing or a NIF-held heap with ownership gymnastics. This isn't a "runtime implementation detail" — it's a likely dead end for interpreter-class throughput.

Also observable-GC surface: **`WeakRef` + `FinalizationRegistry`** are in modern ES and make GC timing observable. The spec must pick semantics (support WeakRef against live objects; treat FinalizationRegistry callbacks as "eventually, on the task queue" — allowed nondeterminism, but write it down so conformance tests don't flake).

**Fix:** add a **GC design spike** as an M0.2-adjacent workstream (choose representation: `Vec<GcBox>` with index handles — which also satisfies the ABI's "handles, never pointers" rule; roots = stack maps or handle-stack; incremental barriers). Demote Elixir to a research stretch goal; make **Go** the ABI-portability proof. Publish that reasoning — "we prove the ABI with a GC'd host we didn't write" is a *stronger* argument than three green checkmarks.

---

## 12. [MAJOR] The dev-experience workstream is missing entirely — and it's not retrofittable

Nothing in the roadmap covers:

- **Source maps / spans end-to-end.** Every IR level must carry source spans; the runtime must map errors in compiled IR back to original TSX line/column, including through the React IR ⇄ JS IR interlink. Without this, debugging a compiled app is archaeology, and it must be designed in from the start — bolting spans onto three IRs after the fact is a rewrite of all three.
- **Error-message parity**: React's *minified error codes* ("Minified React error #185") are string-matched by real libraries; a parity decision belongs in the API table.
- **Dev loop**: watch-mode incremental compile (<300 ms target), a dev artifact with dev-mode React semantics (including whether StrictMode double-invocation is reproduced — libraries are tested under it), and eventually a DevTools-protocol-compatible inspector (stretch).

**Fix:** add a DX workstream starting in M0.3 (span preservation is cheapest the day the IRs are born) with named deliverables; decide the StrictMode parity question explicitly.

---

## 13. [MODERATE] Value-model bugs and gaps already in the drafted code

Concrete, fix-now items:

1. **Strings are UTF-16 in JS.** `s.length`, `s[i]`, `charCodeAt`, substring, non-`/u` regex all count **UTF-16 code units** (`"𝕊".length === 2`). Rust `String` is UTF-8; naive use gives wrong results for any astral-plane character (every emoji). Decide the representation now — `Vec<u16>` (simplest correct) or a latin1/UTF-16 hybrid like V8's — and write it into JS_IR's value model. This is *the* classic engine mistake.
2. **Number-to-string is spec'd and the sketch's is wrong**: `*v as i64` saturates for large values (1e300 prints garbage); Rust `Display` doesn't match ECMA-262 `Number::toString` for exponents (`1e21`, `1e-7`), and the shortest-round-trip rule applies (`0.30000000000000004`). Use a Ryu-style formatter plus the spec's exponent rules, with a test bank including `-0`, `NaN`, `Infinity`, `1e21`, `1e-7`.
3. **The sketch's `Value` has no Object/Array/BigInt/Symbol** — fine for M0.1's demo, but M0.3 "parse real source" hits object literals (props!) immediately. Sequence the value-model growth honestly in the plan.
4. **`EventSystem` stores `Box<dyn FnMut(Event)>`** — acknowledged as temporary in the spec, but it should be fixed *in M0.2, not later*: Rust closures cannot be serialized into an artifact, so the moment the demo uses them, the artifact is no longer runtime-independent and the conformance suite can't feed it from a file. Handlers must be `HandlerId → (component instance, IR continuation)` from the start.
5. **`Scheduler::schedule` does `queue.contains(&component)`** — O(n) per schedule; fine for the demo, but put it on a perf-debt list before the 10,000-row-list benchmark makes it a mystery.
6. **Reconciler iterates `HashMap`** — nondeterministic patch order (see §2).
7. **`Patch::Insert { index }`** assumes an index-stable children array; mixed text/element children and comment placeholders shift indices (see §2).

---

## 14. [MODERATE] Conformance methodology is asserted, not designed

"Publish React compatibility %" needs a mechanism or it's marketing. The credible approach is **differential testing**: run the same app/scenario in real React (Node + jsdom, or headless browser) and in R2N; record the **observable trace** (commit patches, effect order, console output, microtask/task interleaving, thrown errors); diff. Properties:

- It tests *behavior*, which is the spec's own definition of compatibility — no prose interpretation involved.
- It scales: generate random small React apps and fuzz the diff (add a **differential fuzzing** line to M2+; this is the cheapest way to find semantic drift).
- It yields the scorecard honestly: pass-rate over a curated scenario corpus, versioned, per React version.

**Fix:** the differential trace harness is the **M1 exit criterion** (it also forces the scheduling spec of §6 to exist by then, which is the point). Add: port N React Testing Library scenarios as trace scenarios; adopt test262 with a runner and a published subset score for M2; add the top-100-npm-deps triage table as the first M6 artifact (per-dep strategy: native reimplementation vs interpret vs excluded-by-design).

---

## 15. [MODERATE] Build-the-frontend decision: don't hand-write the TS/TSX parser

M0.3 plans "lexer, parser, AST" as in-house crates. A production-grade TypeScript/TSX parser (type assertions, generics in JSX, `satisfies`, decorators, error recovery, and *incremental* parsing for watch mode) is a multi-quarter project by itself and will stay subtly wrong for a year. Both **oxc** and **swc** are mature, spec-complete, **Rust** parser crates — using one violates nothing: "no Node/JS at build time" is preserved (they're Rust), and the boundary the spec actually cares about (runtime knows no source) is untouched. The specialization tier needs the type checker too, which is an even bigger lift (tsc is ~100k lines) — for type-driven hints, consume **types from the app's own `tsc`/build output** where possible rather than reimplementing inference.

**Fix:** M0.3 becomes "integrate oxc/swc frontend + write the AST→JS-IR lowering + the React extraction pass." Keep a thin own-lexer only if total toolchain control is a hard product requirement — and if so, say what that requirement is, because it's expensive.

---

## 16. [MODERATE] Timeline: re-baseline or lose credibility

Off-by-3–5× items:

- **M2 "full JavaScript semantics" in 9 weeks** — engines are multi-year, team-built efforts. Even an app-relevant *subset* with a GC is a quarter-plus. This milestone is the program's true center of mass and it's scheduled like a feature.
- **M4 native renderer in 7 weeks** — see §9; the CSS problem alone exceeds it.
- **M1 (18 hook/behavior tasks + conformance suite) in 6 weeks** — realistic only with the differential harness already built and 3+ engineers.
- **56 weeks through full ecosystem compatibility** — reads as one person-year; is realistically 5+ engineer-years. The architecture's staged levels are right; the calendar is fantasy.

**Fix:** re-publish with (a) a credible **v0.1: vertical slice + app-relevant JS subset + differential harness** (12–16 weeks is honest for that slice), (b) staffing assumptions stated per milestone, (c) **kill criteria** per stage gate (e.g., "if differential pass-rate on the top-100 scenario corpus is <80% after M2, stop and re-scope"), and (d) the compatibility ladder *decoupled from dates* — levels stay, weeks become "relative effort: S/M/L/XL."

---

## 17. What is sound — do not churn these

- **Two-worlds separation** and "runtime never sees source" (with the eval caveat of §10).
- **Behavioral compatibility over API-presence**, the compatibility ladder, and refusing to claim 100% up front.
- **Patch-stream renderers** — one diff contract for all renderers is right.
- **State keying to instance + hook position** (once the instance half is fixed per §2).
- **ABI-first, multi-runtime-gated** — proving the ABI on a second runtime *after* the first is mature is the correct sequencing (and Go, not Elixir, should be that proof — §11).
- **Benchmark-before-optimize**, and the honesty about UI apps not being CPU-bound (the 12 ms → 9 ms example is exactly right).
- The self-corrections already in the spec's history (instance IDs added, patches added, Go/Elixir deferred) show the design loop works.

---

## 18. Change list — mapped to the roadmap

**Specs to add (new docs):**
1. `HOOK_INTEROP.md` — frame protocol, native ⇄ IR calling convention, dynamic hook indexing (§1). **Before M0.3.**
2. `SCHEDULING.md` — task/microtask/rAF/commit ordering tables + batching + interruption (§6). **Before M1 exit.**
3. `REACT_API_TABLE.md` — full react + react-dom export inventory, per-version status + test refs (§7). **M1 entry.**
4. `ARTIFACT_FORMAT.md` — binary encoding, versioned sections, checksums, debug/spans section (§12). **Before M5 ABI freeze.**
5. Trace-contract definition (the "observable behavior" list the optimizer proves against) (§4). **M3 entry.**

**Roadmap task additions:**
- M0.2: template-vs-instance IR rework; `HandlerId → IR continuation` (no Rust closures); deterministic patch ordering. (§2, §13)
- M0.3: frontend decision (oxc/swc) with tradeoff note; span preservation begins; capture-environment/event-handler semantics (§5, §15).
- M1: useSyncExternalStore, useTransition, useDeferredValue, useOptimistic, useActionState, use(), dynamic hook resolution, legacy-context/React-version scoping decision, StrictMode parity decision; **exit = differential trace harness green on scenario corpus.** (§1, §6, §7, §14)
- M2: UTF-16 string representation; ECMA number formatting; GC design spike (tracing, incremental, WeakRef policy); capability-leak review; test262 runner; differential fuzzing line. (§10, §11, §13, §14)
- M3: reorder — inline caches/shapes → profile-guided tier-up → static specialization (last); split native vs wasm targets. (§4, §9)
- M4: pillar-2 rewording (zero *application* JS); browser-first sequencing; taffy/cosmic-text/accesskit integration; CSS strategy decision. (§9)
- M5: Go as the ABI proof; Elixir → research stretch. (§11)
- M6: NODE_ENV const-folding; CJS interop policy; top-100 triage table; "excluded by design" scorecard category; SSR/Next.js boundary statement. (§3, §8)
- New DX workstream (source maps end-to-end, error parity, watch mode) starting M0.3. (§12)
- Re-baseline calendar per §16; staffing + kill criteria per milestone.

**Decisions requiring an explicit call (do not let the code decide by accident):** input contract (source-in vs bundle-in — §3, recommend dual), eval policy (§10), string representation (§13), StrictMode parity (§12), React version target (§7), CSS strategy for native (§9), GC architecture (§11).

---

*This critique reviewed the drafts as shipped. The single highest-leverage change is §1 + §2 together: fix the IR shape (interlinked React IR ⇄ JS IR, template vs instance) before the compiler grows, because every subsequent milestone compiles into that shape.*
