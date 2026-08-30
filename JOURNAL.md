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

## 2026-08-30 — Entry 4: M0.2 Status Reconciliation & Cross-Surface CI Guard

**PLAN**
After #79 (FIFO scheduler) and #80 (14-criteria acceptance suite) merged and
the README declared M0.2 DONE (#82), reconcile every remaining status surface
and add a CI check so this drift class cannot recur silently.

**WHAT**
- CHECKLIST.md M0.2: `IN PROGRESS — exit review pending` → `DONE` (the 14
  acceptance criteria are all green in `tests/acceptance_m02.rs`; the "pending
  review" note predated #82 and was never cleared).
- roadmap.yaml / roadmap.toml: M0.2 `status: "in-progress"` (with a comment
  claiming the FIFO queue and acceptance sweep "remain" — false since #79/#80)
  → `status: "done"`; audit-basis line now cites the 54-test suite (30 was
  the v0.1.0 release-commit count, stale after #79/#81 added 24 tests).
- ROADMAP.md header: "implementation not started" (2026-08-29) → true state
  (M0.1–M0.2 done, M0.3 8/9, 35/106).
- roadmap/README.md: "No implementation code exists yet" → baseline note
  pointing at CHECKLIST/yaml/toml as the source of truth.
- docs/CHANGELOG.md "30 tests" left as-is: it describes the v0.1.0 release
  commit (94dfe01), where the count was exactly 30 — a historical record.
- scripts/verify-audit-claims.sh check [4] rewritten: previously it only
  compared the M0.1 line of README vs CHECKLIST, so M0.2's README-DONE vs
  yaml-in-progress contradiction passed CI. Now it derives every milestone's
  status from task flags in yaml AND toml, cross-checks the declared phase
  status, CHECKLIST status word, README table (incl. grouped M1–M7 rows),
  and ROADMAP.md table. Negative-tested both directions.
- PR #86 (relax cargo-deny wildcards) closed as superseded: #84 versioned the
  intra-workspace path deps — the root-cause fix — so the strict
  `wildcards = "deny"` policy stays.

**WHY**
Records drifted in opposite directions from the same events: the README moved
forward (#82), the yaml/toml stayed behind, and CI had no cross-surface check
to catch it. The project's rule is that claims must be re-derivable, so the
fix is a check, not just an edit.

**OPTIONS**
- Hand-edit the four files and move on: rejected — same drift would recur on
  the next milestone.
- Generate all record files from yaml/toml: rejected for now — more moving
  parts than the project needs at 35/106; the extended check covers the gap.

**CHOSE**
Reconcile the records, then enforce agreement in CI: status is derived from
task flags, and every surface must agree or the build fails.

## 2026-08-30 — Entry 5: M0.3-T08 Diagnostics — Multi-Error Reporting with Recovery

**PLAN**
Close the last M0.3 task: turn single-error, Debug-formatted parse failures
into friendly multi-error diagnostics — all errors in one pass, rendered with
source lines and carets, printed by the CLI.

**WHAT**
- `TokenKind::describe()`: friendly token names — `` `;` ``, `` `=>` ``,
  "end of file" — replacing `{:?}` Debug output in every parser message.
- `ParseError::render(src)`: rustc-style snippet — line-number gutter, the
  offending source line, a caret under the error column (tabs expanded).
- Lexer bug found and fixed while testing carets: `Token::column` had
  inconsistent semantics — single-char tokens recorded the *pre-consumption*
  column, multi-char tokens the *post-consumption* one, so a caret could land
  anywhere. Tokens now record their own first-character position
  (`token_line`/`token_col`, 1-based) captured in `next_token` before any
  character is consumed.
- `parse_with_recovery` (new `recovery.rs`): re-runs the same grammar over a
  flat token list with two recovery levels — a failed statement inside a
  component body is recorded and the parser re-syncs at the next
  `let`/`const`/`return`/`;`/`}`; a failed top-level declaration re-syncs at
  the next `import`/`component`/`export`. Lexer errors (unterminated
  string/comment) stay fatal — no sane resumption point.
- Grammar parity is enforced by test: on error-free sources (all three
  examples + a full-grammar exercise) the recovering parser must produce an
  AST identical (`PartialEq`) to the strict parser's, and report zero errors.
  JSX text children are sliced from source byte offsets between token
  boundaries — the same span the strict lexer's `rescan_jsx_text` yields.
- `r2n_compiler::collect_diagnostics`: parse-with-recovery rendered output;
  lowering errors appended when the parse is clean.
- CLI: on compile failure prints every diagnostic with its rendered snippet
  and a `found N error(s)` summary; exit code 1 unchanged.
- 11 new tests (`crates/r2n-parser/tests/diagnostics.rs`): parity, multi-error
  collection (statement and component level), recovery keeping later valid
  statements/declarations, friendly messages, caret alignment, strict-parser
  semantics unchanged. Suite: 65 green, clippy clean.

**WHY**
One error per edit-compile round-trip is hostile to users; compilers are
judged by their diagnostics. The parity test exists because recovery parsers
accept-and-recover differently than strict ones by nature — the only honest
way to claim "recovery" is to prove it changes nothing on valid input.

**OPTIONS**
- Error recovery inside expressions (skip to matching paren): rejected —
  cascades: one bad expression would report every following token as an error.
  Statement-level granularity is the standard sweet spot (rustc does the same).
- Reuse the strict parser struct with an error-collecting mode: rejected —
  the strict parser drives a live lexer (needed for `looks_like_arrow` and
  JSX rescanning); threading recovery through it would entangle the two. The
  flat-token-list twin mirrors the grammar 1:1 and is parity-tested instead.
- Keep Debug token names: rejected — `expected Semicolon, found Ident("x")`
  is compiler-speak; users read `` expected `;`, found `x` ``.

**CHOSE**
Full recovery with proven parity, friendly names, carets; records updated in
the same PR (CHECKLIST/yaml/toml M0.3 → DONE 9/9, 36/106 overall), issue #7
closed by the merge.

## 2026-08-30 — Entry 6: M1-T01 Props & Children Propagation

**PLAN**
Open M1 (React Compatibility) with its P0 foundation: components receive
props through declared params (already worked) and — the actual gap — JSX
children of a component element must compose into the child's tree exactly
like React: `<Card><b>hi</b></Card>` makes `children` readable inside Card,
and the children still evaluate in the parent's scope.

**WHAT**
- `JsExpr::Children(Vec<ReactNode>)` / `Value::Children`: component-element
  JSX children lower NOW, in the parent's context, into pre-built React-IR
  nodes that ride the `children` prop as pure serializable data (no Rust
  function pointers — the ABI rule holds).
- `ReactNode::Children`: the splice point, lowered from the `children`
  identifier wherever it appears in render position (host child, sole
  expression, ternary branch — all covered by `lower_child`/`lower_renderable`).
- Engine: a `SpliceMap` (instance path → nodes + parent env + parent inst
  path) threaded through `render_node`. A component call records its splice
  from the fresh `children` prop each pass (stale splices are removed when
  the prop disappears); the splice point renders the stored nodes against the
  PARENT's env and hook frame as a transparent `cfrag` fragment — riding the
  existing fragment mechanism (render-time splice + keyed diff), so children
  reconcile with stable `^i` keys and sibling positions line up on every
  renderer.
- 8 acceptance tests (tests/props_children.rs): props through params,
  basic composition, parent-scope closure (child's `{n+ 1}` reads the
  parent's n — the shadowing test proves composition-by-reference), splice
  among siblings, liveness across click → SetText re-renders with zero
  removals, no-children renders nothing, nested component+children chains,
  two independent slot instances in order. Suite: 73 green, clippy clean.
- examples/composition.r2n (Card/Badge/App) renders through the CLI; the
  JSON artifact carries the new variants and stays valid.

**WHY**
Children composition is the load-bearing React pattern — layout components,
slots, lists of anything — and it is the M1 gate the other hooks build on.
The design decision that matters: children lower EARLY (parent context) and
evaluate LATE (parent scope at splice time), which preserves both capture
semantics (`{n}` in a child slot is the parent's n, exactly like React) and
the artifact rule (nodes are data; the runtime never sees source).

**OPTIONS**
- Render children eagerly in the parent and pass a rendered subtree value:
  rejected — breaks reactivity (the subtree's expressions would evaluate in
  the parent's render pass, not the child's) and bloats the value set.
- `children` as a reserved env name only (no IR node): rejected — the splice
  point must be visible in the artifact for cross-runtime parity; a name
  convention would be invisible in serialized form.
- Suspected an instance-path collision bug for same-name sibling components
  (my first nested test failed); a minimal repro proved the engine right —
  the test had declared `component Middle()` without a param, so
  `label="override"` was correctly dropped. Test fixed; no engine change.

**CHOSE**
Children as first-class ABI data: lower early, splice late, close over the
parent. Records updated in the same PR (M1 1/18 in progress, 37/106).

## 2026-08-30 — Entry 7: M1-T02 Keys as First-Class Identity

**PLAN**
React keys on ANY child (not just `.map` list items): an author-provided
`key` is the child's reconciliation identity — a keyed child that changes
position MOVES (same node id, same component instance, hook state intact)
instead of being destroyed and recreated.

**WHAT**
- `static_key_expr(node)`: peeks the `key` prop expression off a static
  child — host element, component call, or LOOKED THROUGH a conditional
  (a keyed child rendered by either ternary branch is the same child).
- Host-children loop: the key is evaluated in the PARENT's scope (where the
  element is written — React evaluates keys at element-creation time) and
  becomes the child's identity segment `k:{value}`; unkeyed siblings keep
  positional `#i`. The `If` arm passes the keyed path through AS-IS (the
  sibling loop already appended the key segment; appending again would
  double it and break id_map agreement between render and diff).
- Component instances keyed by their element key follow the key across
  position changes — the decisive test moves a keyed `<Tick/>` from slot 0
  to slot 2 past a static sibling and its `useState` survives (101, not
  re-initialized 100), with `Move` patches and no Remove/Create for it.
- `key` never reaches renderers (both diff paths already stripped it; now
  test-enforced).
- PRE-EXISTING RECONCILER BUG found and fixed: `diff_children`'s Move
  patches used SURVIVOR-RELATIVE indices as the move target — wrong
  whenever new nodes are created interleaved among moved survivors (the
  moved child lands at the wrong index; the tree ends up misordered).
  Moves now use the child's ABSOLUTE position in the new list, while the
  relative-order comparison still decides WHETHER to move (removals alone
  must not trigger Moves).
- 8 tests (tests/keys.rs): identity across state change, reorder via Move,
  genuine slot-swap with state survival, key-stripping at the renderer,
  mixed keyed/positional siblings, keys through children splices,
  duplicate keys render both without panic.

**WHY**
Keys are React's core reconciliation tool; without them on static children,
any conditional re-layout silently re-initializes component state (the
exact bug class React's docs warn about). The strengthened decisive test
was needed because my first passing version never actually moved the child.

**OPTIONS**
- Evaluate the key INSIDE the child's render: rejected — identity must be
  known before the child's node path is built; a post-hoc key breaks the
  render/diff path agreement (learned the hard way: double-appended key
  segments caused Remove+Create despite correct identity).
- Warn on duplicate keys (React behavior): deferred — needs a diagnostics
  channel from runtime to compiler; tracked for the conformance suite (M1-T17).

**CHOSE**
Keys as position-free identity everywhere: static children, conditionals,
and (already) list items — one rule, ADR-010 semantics, proven by a real
slot-swap. Records updated in the same PR (M1 2/18, 38/106).

## 2026-08-30 — Entry 8: M1-T03 Fragments

**PLAN**
The `<>...</>` shorthand: a group of children with no host element, exactly
React's Fragment. The runtime already had a transparent fragment mechanism
(internal `\0frag` tag used by lists and children splices) — the work is
the JSX syntax, the IR node, and reconciliation correctness when fragments
become SIBLINGS.

**WHAT**
- Parser: `<>` / `</>` parse as an Element with an empty tag (the only shape
  the existing JSX machinery threads uniformly through children parsing,
  text rescanning, and self-closing). Closing `</>` needs no tag match.
- IR: `ReactNode::Fragment { key, children }` (key is the only prop React
  fragments accept; `LowerError::InvalidFragmentProp` guards it — currently
  unreachable through the grammar because shorthand fragments take no
  attributes, matching JSX/Babel, but enforced for future named-fragment
  syntax). subst_node / collect_free_node / try_lower_list (fragment list
  items contribute their key to the List) all handle it.
- Runtime: the Fragment arm renders children and returns the transparent
  FRAGMENT host — the parent's children loop splices them in place, so
  siblings flow around fragments and nested fragments flatten transitively.

**BUGS FOUND BY THE ACCEPTANCE TESTS**
1. Fragment-child keys collided with the parent's positional keys: spliced
   fragment children used bare `#i`, and the parent's button at IR index 1
   is ALSO `#1` — the keyed old/new maps aliased, so a ternary flip left
   STALE nodes in the tree and a pure text update churned a fragment
   sibling through Remove+Create+Move. Fix: fragment-child keys are scoped
   by the fragment's own path segment (`then:0`, `#0:1`, `a:2`...) — unique
   per fragment instance, stable across re-renders.
2. PRE-EXISTING diff bug: flat renderer positions assumed one slot per
   sibling (`index + i` for fragment children). With MULTIPLE fragment
   siblings (fragment `.map` items), later fragments' children landed at
   wrong Create indices and the rendered order came out grouped instead of
   interleaved. Fix: `flat_positions()` — a fragment sibling occupies
   `children.len()` flat slots; diff's Create indices and Move targets use
   flat positions everywhere.

**WHY**
Fragments are the last structural JSX primitive before the hook set; the
sibling-collision bug class (keyed maps aliasing on spliced children)
would have resurfaced for any future splicing feature, so the scoped-key
rule is the load-bearing decision. Flat positions make the diff's index
arithmetic honest about the tree shape the renderer actually sees.

**OPTIONS**
- Renumber keys at splice time (rewrite spliced children's keys to continue
  the parent's numbering): rejected — recursive key rewriting would clobber
  the stable keyed identity of nested list items.
- Fragment items move as a unit (React moves the group): NOT DONE — our
  Move patch is single-node and fragment hosts have no renderer node; the
  id lookup misses and the move is skipped (children move individually).
  Documented limitation, deferred to the M1-T17 conformance hardening.

**CHOSE**
Scoped keys + flat positions: one rule each, proven by 9 tests (no-host
render, siblings, nested flattening, ternary branches, list-item
interleaving + keyed survival, children-splice composition, self-closing,
positional diffing). 89 green; records updated (M1 3/18, 39/106).

## 2026-08-30 — Entry 9: M1-T04 Conditional Rendering & Lists

**PLAN**
First-class control flow: the idioms every React codebase uses. Ternaries
and `.map` existed since M0.2/M0.3; the gaps were `&&`/`||` element
short-circuits, nullish children semantics, index access in JSX, and
filter chains.

**WHAT**
- `{cond && <el/>}` / `{cond || <el/>}` lower STRUCTURALLY (in lower_child)
  to `If` with an empty-fragment "nothing" branch — the element side is the
  renderable branch, the other side the condition. Value short-circuits
  (`{flag && "text"}`) ride the existing Text path.
- React children semantics in the Text arm: `true`/`false`/`null`/null
  render NOTHING (empty fragment splices zero children); numbers (incl. 0)
  and strings render — the classic `{count && <el/>}` renders `0` footgun
  is parity, deliberately preserved.
- `arr[i]` as a JSX child renders the indexed value (the parser emits
  `.get(i)`; child-position calls that aren't `.map` were rejected — index
  access now falls through to Text).
- `arr.filter(pred)` evaluates (per-item protocol shared with `call_map`:
  param 0 = element, param 1 = index) so `filter(...).map(...)` chains work.
- Conditional UNMOUNT destroys hook state: FrameStore counts render passes;
  HookFrame records its last-seen pass; begin_render resets slots when the
  frame skipped a full pass (React: unmount = state gone; remount = fresh).
  Continuous renders never reset.

**WHY**
These are the load-bearing render idioms — without `&&` and nullish
suppression, every real app's conditional UI compiles to wrong output
("false" rendered as text). The unmount-reset closes the semantic loop the
conditional tests exposed: my first test asserted remount-fresh state and
the engine returned the OLD value — the frame lived forever.

**OPTIONS**
- Reset on branch-flip only (then/else key change): rejected — the same
  keyed component moving between slots must KEEP state (M1-T02 semantics);
  pass-staleness is position-agnostic and matches React's tree-presence rule.
- Clean up removed frames from the store: deferred — stale frames are
  harmless post-reset (re-init on next use); leak-bounded by app size.

**CHOSE**
Structural short-circuit lowering + pass-staleness reset. 8 tests
(tests/control_flow.rs): && element, || element, falsy-vs-0 semantics,
value short-circuit, ternary chains, index child, filter().map() chain,
conditional unmount/remount lifecycle. 97 green; records updated
(M1 4/18, 40/106).

## 2026-08-30 — Entry 10: M1-T05 useReducer

**PLAN**
`useReducer(reducer, initial)` → `[state, dispatch]` with React semantics:
dispatch(action) runs `reducer(state, action)`, writes the frame, marks it
dirty, and the flush loop re-renders. The reducer must be ABI-safe IR data,
never a Rust function pointer.

**WHAT**
- `HookSlot::Reducer { params, body, state }` — the reducer arrow (params +
  body, a `JsExpr`) stored in the hook frame; `use_reducer` skips
  re-registration on subsequent renders and re-initializes from `initial`
  only on first mount.
- `Value::Dispatcher { slot }` — the dispatch handle (like `Setter`, it
  carries only the frame slot index and is serializable).
- `call_value` grew a `Dispatcher` arm: it reads the stored reducer, builds
  a fresh env bound to `(state, action)`, evaluates the body, and writes
  the result back — dirty → flush → render. `call_value`'s signature now
  carries the evaluator context (env/host/components/effects) that reducers
  need; `SetValue`/Handler arms unchanged.
- `call_var("useReducer")` extracts the reducer from the arg as IR
  (`JsExpr::Closure` params + body) — never evaluated as a value.

**WHY**
The dispatch handle must stay a plain ABI value (like `Setter` and
`Handler`) — the frame slot index is all that crosses boundaries; the
reducer itself is IR the runtime owns. The setter machinery couldn't be
reused directly because a reducer needs BOTH the old state and the action
to compute the next value.

**OPTIONS**
- Reducer as a functional setter (`apply_setter(s, closure)`) — the setter
  API takes a Value, and closures aren't real values yet (M2); storing the
  body in the slot is the same mechanism effects use, minus the env capture.
- Reducer as a `Value::Handler`-like (inst_path + body): overkill — the
  reducer is pure (params bound each call); a fresh env is correct and
  cheaper than a scopes-map round trip.

**CHOSE**
Slot-stored reducer IR + dispatcher slot handle; 6 tests (event dispatch,
multi-action transitions, batching to one render, per-instance
independence, toggle semantics, persistence across parent renders).
Suite 103 green; clippy/fmt/audit clean; records updated (M1 5/18, 41/106).

## 2026-08-30 — Entry 11: M1-T06 useEffect Lifecycle

**PLAN**
React's full effect lifecycle beyond the mount-only setUp that existed:
cleanup before re-run (deps changed) and cleanup on unmount — with the
EXACT ordering (cleanup-old before setup-new) verified by host logs.

**WHAT**
- Parser: `return expr;` as the terminal statement of block-bodied arrows
  (the React cleanup spelling `() => { s(); return () => c(); }` — the
  returned expr is the block VALUE). Mirrored in the recovery parser.
- `HookSlot::Effect { deps, cleanup }`: is the armed cleanup (body +
  captured env) stored per effect slot.
- `use_effect(deps, cleanup)` returns `(should_run, old_cleanup)`; when deps
  changed, the OLD cleanup is returned to run BEFORE the new setup; when
  deps did NOT change, the previously-armed cleanup stays armed (React).
- Cleanup extraction: `cleanup_of` reads the effect arrow's VALUE — block
  last-stmt closure or the whole body for the `() => () => c()` shorthand.
- UNMOUNT-CLEANUP at unmount, not at remount (first version missed this): a
  frame absent from a render pass is unmounted — per-pass
  `take_unmounted_cleanups` runs its armed cleanups immediately and DISARMS
  them so a later remount cannot run them again (confirming the first
  implementation ran them late, at remount, which the log test caught).
- Removing the now-dead `HookSlot::Ref` (effects moved to Effect).

**WHY**
Without cleanup-on-unmount, resources (timers, listeners in real apps) leak
until a remount — the classic React footgun the lifecycle exists to
prevent. The twice-built ordering guard (cleanup BEFORE setup on deps
change) is the precise React behavior the acceptance tests pin down via
`rt.logs()` order.

**OPTIONS**
- Cleanup = a second hook (`useCleanup`): rejected — not React's API; the
  return-value form is what user code compiles against.
- Run cleanups lazily at remount: rejected — log test caught the late run;
  unmount-time release is the observable behavior.

**CHOSE**
Return-value cleanup + per-pass unmount drain. 6 tests; suite 109 green;
records updated (M1 6/18, 42/106).
