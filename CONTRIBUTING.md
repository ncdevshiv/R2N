# Contributing to R2N

## The one rule

**No fake claims.** A milestone, feature, or task is done only when its acceptance tests pass — never because an API exists, a demo works, or a checklist box is checked. If you add or change behavior, you add the test that proves it.

## Ground rules

1. **No stubs.** `todo!()`, `unimplemented!()`, placeholder returns, or "fake but green" paths are bugs. If something cannot be implemented now, it gets a GitHub issue and stays out of the code.
2. **Architecture boundaries are load-bearing:**
   - `r2n-runtime` must never depend on `r2n-parser`, `r2n-ast`, or `r2n-compiler` (enforced by `crates/r2n-runtime/tests/architecture.rs`).
   - Renderers consume only the `Patch` stream (enforced for the memory renderer by the same suite).
3. **Observable semantics are sacred.** Optimization may transform behavior only when equivalence is proven by tests.
4. **Records must match code.** If your PR closes a roadmap task, update `roadmap/CHECKLIST.md`, `roadmap/roadmap.yaml`, `roadmap/roadmap.toml`, and `roadmap/ROADMAP.md` in the same PR. `scripts/verify-audit-claims.sh` re-derives claim counts from the checklist and fails on mismatch.

## Workflow

1. Branch from `main` (`feat/...`, `fix/...`).
2. Keep PRs small and vertical: one feature/fix, its tests, and the roadmap record updates.
3. CI must be green: build, `cargo fmt --check`, clippy `-D warnings`, all tests, the audit-claim verifier, and the dependency-graph check.
4. PR titles are imperative ("Add FIFO scheduler with dedup").
5. Squash-merge; the roadmap files should show *what state the code is in*, not a history of it.

## Local pre-flight

```bash
cargo fmt
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/verify-audit-claims.sh
```
