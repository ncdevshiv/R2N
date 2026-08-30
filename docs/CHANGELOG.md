# Changelog

All notable changes. The format is based on Keep a Changelog, and releases
correspond to git tags that must point at a green CI commit on main.

## v0.1.0 — 2026-08-30

First audited public checkpoint. Every claim below is enforced by the test
suite (30 tests) and the CI pipeline (fmt, clippy `-D warnings`, tests,
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
