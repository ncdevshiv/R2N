# Changelog

All notable changes. The format is based on Keep a Changelog, and releases
correspond to git tags that must point at a green CI commit on main.

## Unreleased

### Added

- Multi-module linking (M2-T09): `link_source` / `link_source_dev` flatten an
  entry source and every module it reaches into ONE global `RuntimeTemplate`,
  resolving cross-module `import`/`export` bindings to global component indices;
  `import("path")` binds a module namespace as a `ComponentRefVal` record.
- Dynamic-import discovery + canonicalization (M2-T09): the linker walks the AST
  for `import("path")`, so a module reachable ONLY dynamically is still linked,
  laid out, and bound as a namespace; its specifier is rewritten to the resolved
  canonical module id so the `@module:{id}` key matches where the runtime binds it.
- `FsResolver` / `MemResolver` normalize relative specifiers (`./`, `..`) so
  relative dynamic imports resolve deterministically.
- Component-as-value rendering (M2-T09): a `ComponentRefVal` in value/children
  position (e.g. `{ns.Widget}`, or `const C = ns.Widget; {C}`) now MOUNTS the
  referenced component instead of printing the placeholder `<component#N>` handle
  — the runtime re-dispatches to a real component mount, so the caller needs no
  static import.
- Dynamic-component JSX tag (M2-T09): a component value in TAG position
  (`<C/>`, `<C prop=...>child</C>`) where `C` is a local `let`/`const`/param
  resolves at RENDER time — the lowerer emits a `ReactNode::ComponentExpr` and the
  engine mounts the referenced component with props and children. This lets a
  component be passed as a value (prop, namespace member, or local) and rendered
  as a JSX tag without any static binding.
- Namespace-member JSX tag (M2-T09): `<ns.X/>` where `ns` is a local bound to a
  module namespace (or a namespace object passed as a prop) resolves the member
  at RENDER time — the lowerer emits a `ReactNode::ComponentExpr` for the member
  access (`m.Widget`) and the engine mounts the referenced component. A dotted
  JSX tag is now always treated as a component form (never a host element), so
  `<m.Widget/>` works like the equivalent `<C/>` when a namespace is in scope.

### Tests

- 3 new tests (2 linker, 1 runtime) for dynamic-import discovery and specifier
  canonicalization.
- 3 new runtime tests for the dynamic-component JSX tag (a local value as a bare
  `<C/>` tag, a tag with props+children, and a component passed as a prop then
  rendered as `<P/>`).
- 3 new runtime tests for the namespace-member JSX tag (`<m.Widget/>` bare, with
  props+children, and a namespace object passed as a prop then rendered as
  `<P.Widget/>`). Suite is now 257 green, clippy clean.
- test262-aligned conformance harness (M2-T15): `crates/r2n-runtime/tests/
  test262_subset.rs` pins 130 ECMA-262 semantics authored in the dialect
  (upstream test262 files need `var`/loops/`assert()` the engine doesn't have),
  with 14 category tests, an env-gated triage dump (`R2N_TRIAGE=1`), and a
  `published_scorecard_matches_harness` consistency test that fails CI unless
  docs/COMPATIBILITY.md agrees with the harness's computed score.
- Published compatibility scorecard (M2-T15): `docs/COMPATIBILITY.md` records
  **117/130 = 90%** of the test262-aligned subset, the 13 known gaps/divergences
  with reasons (BigInt+Number TypeError, `-`/`<` ToNumber coercion, ≥1e21 and
  <1e-6 exponent formatting, string-index `undefined`, broken function identity
  `f === f`, and the six deliberate M2-T05 divergences), and the out-of-scope
  surface. Suite is now 273 green, clippy clean.


## v0.1.0 — 2026-08-30

First audited public checkpoint. Every claim below is enforced by the test
suite (254 tests) and the CI pipeline (fmt, clippy `-D warnings`, tests,
audit-claim verification, dependency-boundary check).

### Added

- Real compiler vertical slice: hand-written lexer (JSX-text aware,
  byte-offset tracking) → recursive-descent parser (precedence climbing,
  arrows with expression and block bodies, JSX elements with raw-text
  children, expression statements) → interlinked three-layer IR
  (`r2n-ir`: JS IR, React IR, Runtime IR) → serde JSON artifact.
- Zero-JS runtime (`r2n-runtime`): closed ABI value set, `useState` /
  `useEffect` through the per-instance frame protocol, keyed
  `(type, key, path)` reconciliation emitting a minimal `Patch` stream,
  event dispatch (`Runtime::dispatch`) executing `on*` handler closures
  against their owning frame and saved scope, then flushing to a clean tree.
- Memory renderer (`r2n-renderer-memory`) consuming the same patch stream
  every future backend will.
- Event-driven reactive loop proven end-to-end: one click → exactly one
  `SetText` patch, no parent recreation; two instances hold independent
  state; `useEffect` runs on mount and only on dependency change; keyed
  list append reconciles without removals.
- Architecture guards in CI: `r2n-runtime` may not depend on parser/AST/
  compiler (test + static manifest check); roadmap/README claims are
  re-derived from source by `scripts/verify-audit-claims.sh` on every PR.

### Known gaps (tracked, not claimed)

- Scheduler is a dirty-flag loop, not the specified FIFO queue with dedup
  (issue: M0.2-T04).
- The formal 14-criterion M0.2 acceptance sweep is not enumerated as a
  test suite (issue: M0.2-T13).
- Artifact lacks format/ABI version stamps (issue: M0.3-T09).
- Everything in M1–M7 (hooks beyond state/effect, full JS semantics,
  specialization, non-memory renderers, Go/Elixir runtimes, ecosystem
  compatibility, productionization).
