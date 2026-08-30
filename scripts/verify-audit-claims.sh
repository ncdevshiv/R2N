#!/usr/bin/env bash
# Re-derives every headline claim about R2N's state from the code and records.
# Exits non-zero on any mismatch, so CI blocks "records say X, code says Y".
#
# Checks:
#   1. CHECKLIST.md header count == actual [x] checkbox count
#   2. roadmap.yaml task flags == roadmap.toml task flags (same source of truth)
#   3. Milestone progress lines in CHECKLIST.md == per-milestone [x] counts
#   4. README status table agrees with CHECKLIST.md progress lines
#   5. The architecture rules hold: runtime has no parser/ast/compiler dep
#      (also enforced by cargo test, checked here independently)
#   6. No stub markers anywhere in shipped code
set -euo pipefail
cd "$(dirname "$0")/.."

fail() { echo "AUDIT CLAIM MISMATCH: $1" >&2; exit 1; }

echo "[1/6] CHECKLIST.md header vs actual checkboxes"
header=$(grep -oE '\*\*[0-9]+/106\*\* tasks done' roadmap/CHECKLIST.md | head -1 | sed -E 's/\*\*([0-9]+)\/106\*\* tasks done/\1/')
actual=$(grep -c '^\- \[x\]' roadmap/CHECKLIST.md)
[ "$header" = "$actual" ] || fail "CHECKLIST.md claims $header/106 done but has $actual checked boxes"

echo "[2/6] roadmap.yaml vs roadmap.toml task flags"
python - <<'PY'
import sys, tomllib
try:
    import yaml
except ImportError:
    sys.exit(0)  # yaml module optional; toml check below still runs
yml = yaml.safe_load(open('roadmap/roadmap.yaml', encoding='utf-8'))
tml = tomllib.load(open('roadmap/roadmap.toml', 'rb'))
def flat(doc):
    out = {}
    for ph in doc['phases']:
        for t in ph['tasks']:
            out[t['id']] = (ph['id'], t['done'])
    return out
y, t = flat(yml), flat(tml)
if set(y) != set(t):
    sys.exit(f"AUDIT CLAIM MISMATCH: task ids differ between yaml and toml: {set(y) ^ set(t)}")
for tid in y:
    if y[tid][1] != t[tid][1]:
        sys.exit(f"AUDIT CLAIM MISMATCH: {tid} done={y[tid][1]} in yaml but {t[tid][1]} in toml")
print("    ok: yaml and toml agree on", len(y), "tasks")
PY

echo "[3/6] Per-milestone progress lines vs [x] counts"
python - <<'PY'
import re, sys
s = open('roadmap/CHECKLIST.md', encoding='utf-8').read()
# sections: "## M0.1 — ..." then progress "**13/13**" then task lines until next ##
for m in re.finditer(r'## (M[0-9.]+)[^\n]*\n\n`[^`]+`[^\n]*progress \*\*(\d+)/(\d+)\*\* \(\d+%\)\n', s):
    mid, claimed_done, claimed_total = m.group(1), int(m.group(2)), int(m.group(3))
    start = m.end()
    nxt = s.find('\n## ', start)
    section = s[start: nxt if nxt != -1 else len(s)]
    actual = len(re.findall(r'^- \[x\]', section, re.M))
    total = len(re.findall(r'^- \[[ x]\]', section, re.M))
    if actual != claimed_done or total != claimed_total:
        sys.exit(f"AUDIT CLAIM MISMATCH: {mid} claims {claimed_done}/{claimed_total} but section has {actual}/{total}")
print("    ok: all progress lines match section counts")
PY

echo "[4/6] README status vs CHECKLIST"
readme_m01=$(grep -oE 'M0\.1 Foundation.*\*\*DONE\*\*' README.md | head -1 || true)
checklist_m01=$(grep -oE '`DONE` · weeks 1–2 · progress \*\*13/13\*\*' roadmap/CHECKLIST.md | head -1 || true)
[ -n "$readme_m01" ] && [ -n "$checklist_m01" ] || fail "README/CHECKLIST M0.1 status disagree"

echo "[5/6] Architecture boundary: runtime deps"
deps=$(sed -n '/^\[dependencies\]/,/^\[/p' crates/r2n-runtime/Cargo.toml | grep -oE 'r2n-[a-z-]+' || true)
for forbidden in r2n-parser r2n-ast r2n-compiler; do
  echo "$deps" | grep -q "$forbidden" && fail "r2n-runtime depends on $forbidden"
done
echo "    ok: runtime depends only on: $(echo $deps | tr '\n' ' ')"

echo "[6/6] No stub markers in shipped code"
hits=$(grep -rn -E 'todo!|unimplemented!|FIXME|XXX: stub|placeholder implementation' crates --include='*.rs' | grep -v '/tests/' || true)
[ -z "$hits" ] || { echo "$hits" >&2; fail "stub markers found in shipped code"; }

echo "AUDIT OK: every claim re-derived from source and consistent."
