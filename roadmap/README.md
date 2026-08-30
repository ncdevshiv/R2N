# R2N Roadmap Folder

**React to Native** — roadmap, plan, and checklist for the native compiler + runtime platform that executes existing React applications with **zero JavaScript at runtime**. Generated 2026-08-29.

## Files

| File | Purpose |
|---|---|
| **[index.html](index.html)** | Interactive, animated, realtime tracker — open in any browser. Progress ring, animated compile-pipeline diagram, per-phase checklists, Gantt timeline with a live TODAY marker, performance targets, search & filters. Check-offs save instantly to `localStorage`; export/import as JSON to move between machines. |
| **[ROADMAP.md](ROADMAP.md)** | The master document: north star, architecture, compatibility ladder L0–L6, all 10 milestones with exit criteria, performance engineering targets, hard truths, v1.0 definition of done. |
| **[PLAN.md](PLAN.md)** | Execution strategy: sequencing rules, repository layout & dependency law, the settled working designs (state identity, patch stream, core loop, first compiler transformation), M0.2's 14 acceptance gates, testing strategy, risk register. |
| **[CHECKLIST.md](CHECKLIST.md)** | The full 106-task checklist in plain markdown (`- [ ]`), grouped by milestone with priorities. |
| **[SPEC_CRITIQUE.md](SPEC_CRITIQUE.md)** | Adversarial review of the spec: 3 blockers, 8 majors, mods/minors — each with reasoning and fixes, plus a change list mapped to milestones. **Read before building past M0.2.** |
| **[roadmap.yaml](roadmap.yaml)** | Machine-readable roadmap (YAML) — same data as the tracker. |
| **[roadmap.toml](roadmap.toml)** | Machine-readable roadmap (TOML) — same data as the tracker. |

## Quick start

Open `index.html` in a browser (double-click — it is fully self-contained, no server, no network, no dependencies).

## How progress works

- The tracker ships with a **baseline**: 5/106 tasks pre-checked, exactly the design/spec work completed in the R2N architecture sessions (the three specs + the M0.1/M0.2 working designs). No implementation code exists yet, so everything else starts unchecked.
- Checking a task updates all stats, phase bars, and the progress ring **live**, and persists in that browser via `localStorage` (key `r2n.roadmap.progress.v1`).
- **Export** produces `r2n-progress.json`; **Import** restores it. **Reset** returns to the shipped baseline.
- The `.md` / `.yaml` / `.toml` files are the portable record — when a milestone closes, flip its tasks to `done` there and commit, so git history carries the milestone log.

## Milestones at a glance

`M0.1` workspace + vertical slice → `M0.2` reactive runtime loop → `M0.3` JS/JSX compiler frontend → `M1` React compatibility (L1) → `M2` JavaScript compatibility (L2) → `M3` optimization/specialization → `M4` renderers (native/WASM/terminal) → `M5` ABI freeze + Go/Elixir → `M6` ecosystem compatibility (L3–L5) → `M7` production 1.0.

Sequencing laws (from [PLAN.md](PLAN.md)): no Go/Elixir before the ABI is frozen and proven by Rust; no optimization before the conformance suite is green; benchmarks exist before optimization begins; the runtime never sees source code.
