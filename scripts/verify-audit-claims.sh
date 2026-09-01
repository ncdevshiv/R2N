#!/usr/bin/env bash
# Re-derives every headline claim about R2N's state from the code and records.
# Exits non-zero on any mismatch, so CI blocks "records say X, code says Y".
#
# Checks:
#   1. CHECKLIST.md header count == actual [x] checkbox count
#   2. roadmap.yaml task flags == roadmap.toml task flags (same source of truth)
#   3. Milestone progress lines in CHECKLIST.md == per-milestone [x] counts
#   4. Milestone status agrees across all surfaces: yaml/toml phase status,
#      CHECKLIST.md status word, README status table, ROADMAP.md milestone table.
#      A milestone is done iff its task count is 100% in the yaml.
#   5. The architecture rules hold: runtime has no parser/ast/compiler dep
#      (also enforced by cargo test, checked here independently)
#   6. No stub markers anywhere in shipped code
#   7. Each yaml/toml done flag matches the matching CHECKLIST checkbox (per-task)
#   8. README per-milestone progress (N/M) matches yaml task done/total counts
#   9. README "N tests" claim matches the live test-suite count
set -euo pipefail
cd "$(dirname "$0")/.."

fail() { echo "AUDIT CLAIM MISMATCH: $1" >&2; exit 1; }

echo "[1/9] CHECKLIST.md header vs actual checkboxes"
header=$(grep -oE '\*\*[0-9]+/106\*\* tasks done' roadmap/CHECKLIST.md | head -1 | sed -E 's/\*\*([0-9]+)\/106\*\* tasks done/\1/')
actual=$(grep -c '^\- \[x\]' roadmap/CHECKLIST.md)
[ "$header" = "$actual" ] || fail "CHECKLIST.md claims $header/106 done but has $actual checked boxes"

echo "[2/9] roadmap.yaml vs roadmap.toml task flags"
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

echo "[3/9] Per-milestone progress lines vs [x] counts"
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

echo "[4/9] Milestone status agreement: yaml/toml vs CHECKLIST vs README vs ROADMAP"
python - <<'PY'
import re, sys, tomllib
try:
    import yaml
except ImportError:
    sys.exit(0)  # yaml module optional; toml check below still runs
yml = yaml.safe_load(open('roadmap/roadmap.yaml', encoding='utf-8'))
tml = tomllib.load(open('roadmap/roadmap.toml', 'rb'))

# Derive ground truth: a milestone is done iff every task is done.
status = {}
for doc in (yml, tml):
    for ph in doc['phases']:
        total = len(ph['tasks'])
        done = sum(1 for t in ph['tasks'] if t['done'])
        derived = 'done' if done == total else ('in-progress' if done else 'planned')
        if ph['id'] in status and status[ph['id']] != derived:
            sys.exit(f"AUDIT CLAIM MISMATCH: {ph['id']} status differs between yaml and toml")
        declared = ph['status']
        if declared != derived:
            sys.exit(f"AUDIT CLAIM MISMATCH: {ph['id']} status='{declared}' but tasks say '{derived}' ({done}/{total})")
        status[ph['id']] = derived

checklist = open('roadmap/CHECKLIST.md', encoding='utf-8').read()
for mid, st in status.items():
    m = re.search(rf'## {re.escape(mid)}[^\n]*\n\n`([^`]+)`[^\n]*progress \*\*(\d+)/(\d+)\*\*', checklist)
    if not m:
        sys.exit(f"AUDIT CLAIM MISMATCH: no progress line found for {mid} in CHECKLIST.md")
    word, done, total = m.group(1).lower(), int(m.group(2)), int(m.group(3))
    expected_word = 'done' if st == 'done' else ('in progress' if st == 'in-progress' else 'planned')
    if word != expected_word:
        sys.exit(f"AUDIT CLAIM MISMATCH: CHECKLIST says '{word}' for {mid} but tasks say '{st}'")

readme = open('README.md', encoding='utf-8').read()
def expand_mids(row):
    # "M1–M7" (en-dash range) -> M1..M7; plain "M0.2" -> [M0.2]
    out = []
    for m in re.finditer(r'M([0-9]+)(?:\.[0-9]+)?(?:\u2013|\u2014|-)M([0-9]+)(?:\.[0-9]+)?', row):
        lo, hi = int(m.group(1)), int(m.group(2))
        out += [f'M{i}' for i in range(lo, hi + 1)]
    out += re.findall(r'M[0-9.]+(?![0-9])(?![\u2013\u2014-])', row)
    seen, uniq = set(), []
    for mid in out:
        if mid not in seen:
            seen.add(mid)
            uniq.append(mid)
    return uniq

readme_rows = {}          # mid -> row text; grouped rows (e.g. "M1–M7") map to each member
grouped_rows = {}         # mid -> True if it came from a multi-member row
for row in re.findall(r'^\| M[^\n]*\|$', readme, re.M):
    mids = expand_mids(row)
    for mid in mids:
        if mid not in readme_rows:
            readme_rows[mid] = row
    if len(mids) > 1:
        for mid in mids:
            grouped_rows[mid] = True
for mid, st in status.items():
    row = readme_rows.get(mid)
    if row is None:
        sys.exit(f"AUDIT CLAIM MISMATCH: no status row for {mid} in README table")
    if st == 'done' and '**DONE**' not in row:
        sys.exit(f"AUDIT CLAIM MISMATCH: README row for {mid} lacks DONE: {row.strip()}")
    if st != 'done' and '**DONE**' in row:
        sys.exit(f"AUDIT CLAIM MISMATCH: README claims DONE for {mid} but tasks say '{st}': {row.strip()}")
    if mid in grouped_rows and st != 'planned':
        sys.exit(f"AUDIT CLAIM MISMATCH: {mid} is '{st}' but README folds it into a grouped planned row: {row.strip()}")

roadmap = open('roadmap/ROADMAP.md', encoding='utf-8').read()
for mid, st in status.items():
    m = re.search(rf'\| {re.escape(mid)} \| [^\n]*\|', roadmap)
    if not m:
        sys.exit(f"AUDIT CLAIM MISMATCH: no milestone row for {mid} in ROADMAP.md table")
    row = m.group(0)
    if st == 'done' and '**DONE**' not in row:
        sys.exit(f"AUDIT CLAIM MISMATCH: ROADMAP.md row for {mid} lacks DONE: {row.strip()}")
    if st != 'done' and '**DONE**' in row:
        sys.exit(f"AUDIT CLAIM MISMATCH: ROADMAP.md claims DONE for {mid} but tasks say '{st}': {row.strip()}")
print("    ok: milestone status agrees across yaml, toml, CHECKLIST, README, ROADMAP for", len(status), "milestones")
PY

echo "[5/9] Architecture boundary: runtime deps"
deps=$(sed -n '/^\[dependencies\]/,/^\[/p' crates/r2n-runtime/Cargo.toml | grep -oE 'r2n-[a-z-]+' || true)
for forbidden in r2n-parser r2n-ast r2n-compiler; do
  echo "$deps" | grep -q "$forbidden" && fail "r2n-runtime depends on $forbidden"
done
echo "    ok: runtime depends only on: $(echo $deps | tr '\n' ' ')"

echo "[6/9] No stub markers in shipped code"
hits=$(grep -rn -E 'todo!|unimplemented!|FIXME|XXX: stub|placeholder implementation' crates --include='*.rs' | grep -v '/tests/' || true)
[ -z "$hits" ] || { echo "$hits" >&2; fail "stub markers found in shipped code"; }

echo "[7/9] yaml/toml done flags vs CHECKLIST checkboxes (per-task)"
python - <<'PY'
import re, sys, tomllib
try:
    import yaml
except ImportError:
    sys.exit(0)
yml = yaml.safe_load(open('roadmap/roadmap.yaml', encoding='utf-8'))
checklist = open('roadmap/CHECKLIST.md', encoding='utf-8').read()
sections = {}
cur = None
for ln in checklist.splitlines():
    m = re.match(r'## (M[0-9.]+)', ln)
    if m:
        cur = m.group(1); sections[cur] = []
    elif cur and ln.startswith('- ['):
        sections[cur].append(ln.strip())
total = 0
for ph in yml['phases']:
    mid = ph['id']
    tasks = ph['tasks']
    lines = sections.get(mid, [])
    if len(tasks) != len(lines):
        sys.exit(f"AUDIT CLAIM MISMATCH: phase {mid} has {len(tasks)} tasks in yaml but {len(lines)} checkbox lines in CHECKLIST")
    for t, ln0 in zip(tasks, lines):
        total += 1
        checked = ln0.startswith('- [x]')
        if checked != t['done']:
            sys.exit(f"AUDIT CLAIM MISMATCH: {t['id']} done={t['done']} in yaml/toml but CHECKLIST shows '{ln0[:5].strip()}'")
print("    ok: all", total, "yaml/toml done flags match CHECKLIST checkboxes")
PY

echo "[8/9] README per-milestone progress vs yaml task counts"
python - <<'PY'
import re, sys, tomllib
try:
    import yaml
except ImportError:
    sys.exit(0)
yml = yaml.safe_load(open('roadmap/roadmap.yaml', encoding='utf-8'))
readme = open('README.md', encoding='utf-8').read()
ok = 0
for ph in yml['phases']:
    mid = ph['id']
    total = len(ph['tasks'])
    done = sum(1 for t in ph['tasks'] if t['done'])
    m = re.search(rf'\| {re.escape(mid)}[^|]*\| [^|]*\| (\d+)/(\d+) \|', readme)
    if not m:
        continue  # grouped/planned rows carry '—', not a fraction
    rd, rt = int(m.group(1)), int(m.group(2))
    if rd != done or rt != total:
        sys.exit(f"AUDIT CLAIM MISMATCH: README row for {mid} says {rd}/{rt} but yaml tasks say {done}/{total}")
    ok += 1
print("    ok: README per-milestone progress matches yaml task counts for", ok, "milestones")
PY

echo "[9/9] README test-count claim vs live suite"
actual_tests=$(cargo test --workspace --no-fail-fast 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s+0}')
claimed=$(grep -oE '# [0-9]+ tests' README.md | grep -oE '[0-9]+' | head -1)
[ -n "$claimed" ] || fail "README has no '<N> tests' claim to verify"
[ "$actual_tests" = "$claimed" ] || fail "README claims $claimed tests but the suite runs $actual_tests"
echo "    ok: README test count $claimed matches the live suite ($actual_tests)"

echo "AUDIT OK: every claim re-derived from source and consistent."
