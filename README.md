# R2N — React to Native

**A native compiler + runtime platform that executes existing React applications with zero JavaScript at runtime.**

R2N compiles React/JSX source into a language-neutral artifact (`RuntimeTemplate` IR), then executes it on a zero-JS runtime: no JS engine, no Node.js, no JavaScript anywhere in the shipped runtime. Renderers consume a single `Patch` stream — the ABI boundary — so memory, native, WASM, and terminal backends all receive identical reconciliation output.

```
source (.r2n)  →  Lexer  →  Parser (AST)  →  Lowering  →  JS IR ─┐
                                                               React IR ─┤
                                                      RuntimeTemplate IR ─┘
                                                                        │
                                                     zero-JS runtime ←──┘
                                                     event → handler → state
                                                     → dirty → render → diff
                                                                        │
                                                        Patch stream (ABI)
                                                                        │
                                              Memory · Native · WASM · Terminal
```

## Status — audited 2026-08-30

| Milestone | State | Progress |
|---|---|---|
| M0.1 Foundation — workspace & vertical slice | **DONE** | 13/13 |
| M0.2 Reactive runtime loop | **DONE** | 14/14 |
| M0.3 Compiler frontend (JS/JSX → IR) | **DONE** | 9/9 |
| M1 React compatibility — hooks, keys, context, effects | in progress | 5/18 |
| M2–M7 | planned | — |

**Overall: 41/106 roadmap tasks.** Every claim above is backed by the test suite (`cargo test`) and the architecture-guard tests. See [roadmap/CHECKLIST.md](roadmap/CHECKLIST.md) for the task-level record, [JOURNAL.md](JOURNAL.md) for decision history, and [docs/AUDIT.md](docs/AUDIT.md) for the latest audit report.

### What works today (all test-verified)

- **Compiler pipeline**: real hand-written lexer (JSX-text aware, byte-offset tracking) → recursive-descent parser (precedence climbing, arrows, block bodies, JSX children incl. raw text) → three-layer interlinked IR (JS IR → React IR → Runtime IR), serde-serializable to JSON.
- **Zero-JS runtime**: closed-set ABI values (null/bool/f64/UTF-16 string/array/map/setter/handler), `useState`/`useEffect` via the per-instance frame protocol, keyed `(type, key, path)` reconciliation producing minimal `Patch[]`, event dispatch running handler closures against their owning frame + saved scope.
- **Event-driven reactive loop**: `onClick={() => setN(n + 1)}` → `dispatch` → setter → dirty → flush → exactly **one** `SetText` patch, no parent recreation. Two component instances hold independent state.
- **Memory renderer**: applies the same patch stream any backend would; XML-style tree serialization for tests.
- **CLI**: `r2n build` (JSON artifact) · `r2n render` (initial tree) · `r2n run file 3` (fires real click events).

### What is deliberately not done yet

Everything in M1–M7: the full React hook set and behavioral conformance suite, full ECMAScript semantics, the specialization/optimization pipeline, non-memory renderers (native/WASM/terminal), the Go/Elixir runtimes, npm/browser-API compatibility, and productionization. The [issues board](https://github.com/ncdevshiv/R2N/issues) tracks all of it; nothing is claimed that isn't tested.

## Architecture rules (enforced by CI, not by convention)

1. **The runtime never sees source code** — `r2n-runtime` has zero dependencies on parser/AST/compiler; `tests/architecture.rs` fails the build if that changes.
2. **Renderers consume only the `Patch` stream** — same ABI for every backend.
3. **The artifact is language-neutral data** — JSON-serializable `RuntimeTemplate`; closures cross boundaries as (path, IR) pairs, never Rust function pointers.
4. **No stubs** — a milestone closes only when its acceptance tests pass, never because an API exists.

## Build & test

```bash
cargo build --workspace
cargo test --workspace          # 103 tests incl. architecture + acceptance guards
cargo clippy --workspace --all-targets -- -D warnings
scripts/verify-audit-claims.sh  # re-derives every README/roadmap claim from source
```

## Layout

```
crates/r2n-ast            source-level AST (Expr, Element, Stmt, Program)
crates/r2n-parser        hand-written lexer + recursive-descent parser
crates/r2n-ir            JS IR + React IR + Runtime IR + lowering + JSON artifact
crates/r2n-runtime       zero-JS evaluator, hooks (frame protocol), reconciler, events
crates/r2n-renderer-memory  reference renderer consuming the Patch stream
crates/r2n-compiler      orchestration: parse → lower → artifact
crates/r2n-cli           r2n build / render / run
roadmap/                 ROADMAP.md, CHECKLIST.md, roadmap.yaml/toml (source of truth)
scripts/                 CI verification scripts (auditable claims)
docs/                    audit reports
examples/                counter / hello / list
```

## License

MIT OR Apache-2.0 (dual-licensed, like the Rust ecosystem).
