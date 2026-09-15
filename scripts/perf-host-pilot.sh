#!/usr/bin/env bash
# Measure the perf gate's stability on this host, with contemporaneous
# CPU-pressure telemetry.
#
# Why this exists: `extension/execute/*` returned `inconclusive` on twelve
# consecutive A/A comparisons against an identical tree, with p50 steady
# (208-235 us) and p95 swinging 285 us to 4241 us. Sample count was excluded
# as a cause (docs/project/records/2026-09-14-execute-row-spread-diagnosis.md).
# The open question is whether an isolated host produces tighter p95 spread,
# and that cannot be answered without load data recorded *during* each run.
#
# This script is host-agnostic on purpose: run it unchanged here and on a
# candidate host, then compare the two JSON summaries. It writes nothing into
# the repository and changes no source.
#
# Usage:
#   scripts/perf-host-pilot.sh [--runs N] [--samples N] [--filter GLOB]
#                              [--label NAME] [--out FILE]
#
# Defaults reproduce the comparison that is currently blocked: 6 A/A runs of
# the extension rows at 20 samples, which is what the existing records used.

set -euo pipefail

RUNS=6
SAMPLES=20
FILTER='extension/*'
LABEL="$(hostname -s 2>/dev/null || echo host)"
OUT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs) RUNS="$2"; shift 2 ;;
    --samples) SAMPLES="$2"; shift 2 ;;
    --filter) FILTER="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.."
REPO="$PWD"
[[ -n "$OUT" ]] || OUT="/tmp/perf-host-pilot-${LABEL}.json"

# Raw A/B documents and telemetry logs are copied here before cleanup wipes
# the scratch directory. On a disposable host the summary alone is not
# auditable: it would reference paths that no longer exist, and the build
# provenance, timestamps and per-sample intervals would be gone with them.
ARTIFACTS="${OUT%.json}-artifacts"

# One base revision for both the clean-tree guard and `perf ab --base`, so
# the tree being verified is the tree being compared against. `main` matches
# what `just perf` uses.
BASE_REV="main"

# --- preconditions -----------------------------------------------------------
# The A/A comparison is only meaningful when both sides build identical
# measured code: a real diff there makes a nonzero delta legitimate rather
# than noise. Changes to docs and to this script cannot reach a workload, so
# they are ignored -- otherwise the script could never run from an
# uncommitted copy of itself.
if ! command -v jj >/dev/null 2>&1; then
  echo "error: jj not found; this script compares @ against $BASE_REV" >&2
  exit 1
fi
if ! DIFF_PATHS="$(jj diff --from "$BASE_REV" --to @ --name-only 2>/dev/null)"; then
  echo "error: cannot diff @ against $BASE_REV" >&2
  exit 1
fi
MEASURED_DIFF="$(printf '%s\n' "$DIFF_PATHS" \
  | grep -v '^$' \
  | grep -v '^docs/' \
  | grep -v '^scripts/' \
  || true)"
if [[ -n "${MEASURED_DIFF//[[:space:]]/}" ]]; then
  echo "error: working copy differs from $BASE_REV in measured code, so deltas" >&2
  echo "       would not be pure noise:" >&2
  printf '         %s\n' $MEASURED_DIFF >&2
  echo "hint: run from a clean checkout of main; docs/ and scripts/ changes are ignored" >&2
  exit 1
fi

if [[ ! -r /proc/pressure/cpu ]]; then
  echo "warning: /proc/pressure/cpu unreadable; PSI fields will be null" >&2
fi

# --- host facts --------------------------------------------------------------
# Captured once. `virt` and `steal` are the fields that distinguish an
# isolated host from a contended guest, and neither is in the result schema.
host_facts() {
  local cores model virt steal_ticks mem_total
  cores="$(nproc 2>/dev/null || echo 0)"
  model="$(awk -F': ' '/^model name/{print $2; exit}' /proc/cpuinfo 2>/dev/null || echo unknown)"
  virt="$(systemd-detect-virt 2>/dev/null || echo unknown)"
  steal_ticks="$(awk '/^cpu /{print $9}' /proc/stat 2>/dev/null || echo 0)"
  mem_total="$(awk '/^MemTotal/{print $2}' /proc/meminfo 2>/dev/null || echo 0)"
  printf '{"label":"%s","cores":%s,"cpu":"%s","virt":"%s","steal_ticks_at_start":%s,"mem_total_kb":%s,"kernel":"%s"}' \
    "$LABEL" "$cores" "$model" "$virt" "$steal_ticks" "$mem_total" "$(uname -r)"
}

# --- per-run telemetry -------------------------------------------------------
# Sampled at 1 Hz *during* each run. A post-run snapshot cannot attribute load
# to a measurement window, which is the mistake the diagnosis record corrects.
#
# The PID goes to a file, never to stdout: capturing it with `$(...)` would
# make the background loop inherit the command substitution's pipe, and the
# read would block forever because an endless loop never closes it.
sampler_start() {
  local path="$1" pidfile="$2"
  : >"$path"
  (
    while :; do
      local ts psi_some psi_full mem_some steal idle
      ts="$(date +%s)"
      psi_some="$(awk '/^some/{for(i=1;i<=NF;i++) if($i ~ /^avg10=/){sub("avg10=","",$i); print $i}}' /proc/pressure/cpu 2>/dev/null || echo null)"
      psi_full="$(awk '/^full/{for(i=1;i<=NF;i++) if($i ~ /^avg10=/){sub("avg10=","",$i); print $i}}' /proc/pressure/cpu 2>/dev/null || echo null)"
      mem_some="$(awk '/^some/{for(i=1;i<=NF;i++) if($i ~ /^avg10=/){sub("avg10=","",$i); print $i}}' /proc/pressure/memory 2>/dev/null || echo null)"
      read -r steal idle < <(awk '/^cpu /{print $9, $5}' /proc/stat)
      printf '%s %s %s %s %s %s\n' "$ts" "${psi_some:-null}" "${psi_full:-null}" "${mem_some:-null}" "$steal" "$idle" >>"$path"
      # Sleep as a tracked child rather than a builtin wait, so the stopper
      # can end the in-flight interval without signalling anything it does
      # not own. Never `kill 0` here: this subshell shares the pilot's
      # process group, so that would signal the pilot itself.
      sleep 1 &
      printf '%s' "$!" >"$pidfile.sleep"
      wait "$!" 2>/dev/null || exit 0
    done
  ) >/dev/null 2>&1 &
  printf '%s' "$!" >"$pidfile"
}

sampler_stop() {
  local pidfile="$1" pid sleep_pid waited=0
  [[ -r "$pidfile" ]] || return 0
  pid="$(cat "$pidfile" 2>/dev/null || true)"
  [[ -n "$pid" ]] || return 0
  # Only ever the sampler and the one sleep it recorded: no process groups,
  # no wildcards. The sampler shares this script's group, so a group signal
  # here would hit the script itself.
  kill -TERM "$pid" 2>/dev/null || true
  if [[ -r "$pidfile.sleep" ]]; then
    sleep_pid="$(cat "$pidfile.sleep" 2>/dev/null || true)"
    [[ -n "$sleep_pid" ]] && kill -TERM "$sleep_pid" 2>/dev/null || true
  fi
  # Bounded: a sampler blocked in a /proc read must not stall cleanup.
  while (( waited < 3 )); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
    waited=$((waited + 1))
  done
  kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

# --- run ---------------------------------------------------------------------
WORK="$(mktemp -d)"
ACTIVE_SAMPLER=""
ACTIVE_PERF=""

# TERM/INT during a run would otherwise leave the sampler looping against a
# deleted directory and the measurement child running. Cleanup touches only
# PIDs this script started and recorded -- never a pattern match, which on a
# shared host could signal unrelated work.
# Stop an owned process group: TERM, then KILL if the group has not gone
# within the grace period.
#
# Escalation is decided by *group* existence, not the leader's. `just` exits
# promptly on TERM while a grandchild may not, and polling the leader would
# then break out of the loop and skip the KILL while the group survived --
# verified with a leader that exits on TERM and a child that ignores it.
stop_group() {
  local pid="$1" grace="${2:-5}" waited=0
  [[ -n "$pid" ]] || return 0
  # Negative PID targets the process group `setsid` created for this command,
  # so `just` plus its recipe shell, cargo and yach-bench descendants all
  # receive it. Falls back to the bare PID if no group exists.
  kill -TERM "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  while (( waited < grace )); do
    kill -0 -- "-$pid" 2>/dev/null || break
    sleep 1
    waited=$((waited + 1))
  done
  if kill -0 -- "-$pid" 2>/dev/null; then
    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
  fi
}

cleanup() {
  if [[ -n "$ACTIVE_PERF" ]]; then
    stop_group "$ACTIVE_PERF" 5
    ACTIVE_PERF=""
  fi
  if [[ -n "$ACTIVE_SAMPLER" ]]; then
    sampler_stop "$ACTIVE_SAMPLER"
    ACTIVE_SAMPLER=""
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT
trap 'trap - EXIT; cleanup; exit 130' INT
trap 'trap - EXIT; cleanup; exit 143' TERM

echo "host: $LABEL ($(nproc 2>/dev/null || echo '?') cores, virt=$(systemd-detect-virt 2>/dev/null || echo unknown))" >&2
echo "plan: $RUNS runs, --samples $SAMPLES, --filter '$FILTER'" >&2

# Each invocation writes to a path this script chooses, so a result is never
# discovered by mtime. `just perf` appends its own `--out` last, which would
# win over ours, so `perf ab` is invoked through `just dev` -- the
# repository's own dev-shell entry point -- rather than duplicating its
# environment dispatch here.
#
# `exec setsid` matters: without `exec`, `perf_ab &` makes `$!` the helper's
# subshell while `setsid` starts a *different* session, so `kill -- "-$!"`
# targets a group that does not exist and the tree survives. Verified: a
# group signal to the subshell PID failed outright and the child kept
# running. `exec` replaces that subshell, so `$!` is the session leader and
# PID == PGID == SID.
perf_ab() {
  local out="$1" samples="$2"
  exec setsid just dev cargo run -p yach-bench --release --locked -- \
    perf ab --base "$BASE_REV" --filter "$FILTER" \
    --samples "$samples" --out "$out"
}

# Warm the build once so the first measured run does not pay compilation
# inside its window. This is not quick: it materializes a base checkout and
# builds both sides in release -- on this host ~1.6 GB and several minutes,
# and considerably longer on a fresh host with a cold cargo cache. Output is
# captured and replayed rather than streamed, because streaming through a
# pipeline would block signal handling for the build's whole duration.
echo "warm-up: materializing base checkout and building both sides (release)." >&2
echo "         first run on a fresh host can take 10+ minutes; output follows after." >&2
warm_start="$(date +%s)"
set +e
perf_ab "$WORK/warm.json" 1 >"$WORK/warm.log" 2>&1 &
ACTIVE_PERF=$!
wait "$ACTIVE_PERF"
warm_exit=$?
ACTIVE_PERF=""
set -e
sed 's/^/  warm| /' <"$WORK/warm.log" >&2 || true
if [[ "$warm_exit" -ne 0 ]]; then
  echo "warning: warm-up run exited $warm_exit; continuing (a verdict is not needed here)" >&2
fi
echo "warm-up finished in $(( $(date +%s) - warm_start ))s" >&2

RUN_JSON="$WORK/runs.jsonl"
: >"$RUN_JSON"

for ((i = 1; i <= RUNS; i++)); do
  psi_log="$WORK/psi-$i.txt"
  psi_pid="$WORK/psi-$i.pid"
  sampler_start "$psi_log" "$psi_pid"
  ACTIVE_SAMPLER="$psi_pid"
  start_epoch="$(date +%s)"

  # Tracked background child plus `wait`: bash defers trap handling while a
  # foreground child runs, so a TERM during a multi-minute run would be
  # ignored until it returned, leaving the sampler and workdir behind.
  ab="$WORK/run-$i.json"
  rm -f "$ab"
  set +e
  perf_ab "$ab" "$SAMPLES" >"$WORK/run-$i.log" 2>&1 &
  ACTIVE_PERF=$!
  wait "$ACTIVE_PERF"
  perf_exit=$?
  ACTIVE_PERF=""
  set -e

  end_epoch="$(date +%s)"
  sampler_stop "$psi_pid"
  ACTIVE_SAMPLER=""

  # Require the exact file this invocation was told to write. Discovering a
  # result by mtime could silently adopt the warm-up's or a previous run's
  # output when a run fails immediately, turning a failure into a
  # "measurement".
  if [[ ! -r "$ab" ]]; then
    echo "error: run $i did not write $ab (exit $perf_exit); see below" >&2
    tail -20 "$WORK/run-$i.log" >&2
    exit 1
  fi
  # An external-mode comparison measures only the five shipping-binary rows,
  # so extension rows would be `no_base_worker` and the run would be
  # meaningless for this question.
  mode="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get('base_mode',''))" "$ab" 2>/dev/null || true)"
  if [[ "$mode" != "worker" ]]; then
    echo "error: run $i produced base_mode='$mode', expected 'worker'" >&2
    echo "hint: the base checkout must build a yach-bench that answers --schema-probe" >&2
    tail -20 "$WORK/run-$i.log" >&2
    exit 1
  fi

  # Preserve the raw evidence outside the scratch directory: the exact A/B
  # document (with its build provenance, timestamps and per-sample
  # intervals), the telemetry samples, and the run log. Without this the
  # summary's `ab_doc` paths dangle the moment cleanup runs, and a
  # disposable host takes the primary data with it.
  mkdir -p "$ARTIFACTS"
  cp "$ab" "$ARTIFACTS/run-$i-ab.json"
  cp "$psi_log" "$ARTIFACTS/run-$i-telemetry.txt"
  cp "$WORK/run-$i.log" "$ARTIFACTS/run-$i.log"

  # `perf ab` exits 1 on regressed/error and 2 on inconclusive. On an
  # identical tree both are results worth recording, not script failures.
  python3 - "$ab" "$psi_log" "$i" "$perf_exit" "$start_epoch" "$end_epoch" \
    "$ARTIFACTS/run-$i-ab.json" >>"$RUN_JSON" <<'PY'
import json, sys

ab_path, psi_path, run_idx, perf_exit, t0, t1 = sys.argv[1:7]
retained = sys.argv[7]
doc = json.load(open(ab_path))

rows = {}
for v in doc.get("verdicts", []):
    detail = v.get("detail") or {}
    rows[v["id"]] = {
        "verdict": v.get("verdict"),
        "median_delta_pct": detail.get("median_delta_pct"),
        "sign_agreement": detail.get("sign_agreement"),
        "base_spread_pct": detail.get("base_spread_pct"),
        "base_p95_ns": v.get("base_summary"),
        "current_p95_ns": v.get("current_summary"),
        "budget_latency_pct": (v.get("budget") or {}).get("latency_pct"),
    }

# Per-sample distribution from the raw intervals the worker already emits,
# so tail shape is visible rather than inferred from p95 alone.
dist = {}
for side in ("base", "current"):
    for rnd in doc.get(side, []):
        for w in rnd.get("workloads", []):
            raw = w.get("samples_ns")
            if not raw:
                continue
            us = sorted(v / 1000.0 for v in raw)
            entry = dist.setdefault(w["id"], [])
            # Take p50/p95 from the worker, never recompute them here. The
            # worker uses nearest-rank (`latency.rs:52-60`,
            # `ceil(n*pct/100) - 1`), so at n=20 p95 is index 18. A naive
            # `int(n*0.95)` picks index 19 -- the maximum -- which would
            # overstate precisely the tail this pilot exists to compare.
            entry.append({
                "side": side,
                "n": len(us),
                "min_us": round(us[0], 1),
                "p50_us": round(w["p50_ns"] / 1000.0, 1) if w.get("p50_ns") else None,
                "p95_us": round(w["p95_ns"] / 1000.0, 1) if w.get("p95_ns") else None,
                "p99_us": round(w["p99_ns"] / 1000.0, 1) if w.get("p99_ns") else None,
                "max_us": round(us[-1], 1),
            })

psi = []
for line in open(psi_path):
    parts = line.split()
    if len(parts) != 6:
        continue
    def num(x):
        try:
            return float(x)
        except ValueError:
            return None
    psi.append({
        "t": int(parts[0]),
        "cpu_some_avg10": num(parts[1]),
        "cpu_full_avg10": num(parts[2]),
        "mem_some_avg10": num(parts[3]),
        "steal": num(parts[4]),
        "idle": num(parts[5]),
    })

def summarize(key):
    vals = [p[key] for p in psi if p.get(key) is not None]
    if not vals:
        return None
    return {"mean": round(sum(vals) / len(vals), 2), "max": round(max(vals), 2)}

steal_delta = None
if len(psi) >= 2 and psi[0]["steal"] is not None and psi[-1]["steal"] is not None:
    steal_delta = psi[-1]["steal"] - psi[0]["steal"]

# Build provenance and timestamps come straight from the A/B document, so a
# summary can be matched to the exact binaries that produced it after the
# host is gone.
sides = {}
for side in ("base", "current"):
    rounds = doc.get(side) or []
    if not rounds:
        continue
    first = rounds[0]
    sides[side] = {
        "build": first.get("build"),
        "host": first.get("host"),
        "started_at": [r.get("started_at") for r in rounds],
    }

print(json.dumps({
    "run": int(run_idx),
    "perf_exit": int(perf_exit),
    "duration_s": int(t1) - int(t0),
    "started_epoch": int(t0),
    "ab_doc_retained": retained,
    "base_mode": doc.get("base_mode"),
    "provenance": sides,
    "rows": rows,
    "distribution": dist,
    "load": {
        "samples": len(psi),
        "cpu_some_avg10": summarize("cpu_some_avg10"),
        "cpu_full_avg10": summarize("cpu_full_avg10"),
        "mem_some_avg10": summarize("mem_some_avg10"),
        "steal_ticks_delta": steal_delta,
    },
}))
PY

  spread="$(python3 -c "
import json,sys
d=json.loads(open('$RUN_JSON').read().strip().split(chr(10))[-1])
vals=[(k.split('/')[-1], r['base_spread_pct'], r['verdict']) for k,r in d['rows'].items() if r['base_spread_pct'] is not None]
print('  '.join(f'{k}={v:.0f}%/{w[:5]}' for k,v,w in vals) or 'no latency rows')
")"
  echo "run $i/$RUNS: exit=$perf_exit ${end_epoch}s-${start_epoch}s  $spread" >&2
done

# --- summary -----------------------------------------------------------------
python3 - "$RUN_JSON" "$OUT" "$(host_facts)" "$ARTIFACTS" <<'PY'
import json, sys, statistics as st

runs = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
out_path, host_json = sys.argv[2], sys.argv[3]

ids = sorted({k for r in runs for k in r["rows"]})
per_row = {}
for wid in ids:
    spreads = [r["rows"][wid]["base_spread_pct"] for r in runs
               if r["rows"].get(wid, {}).get("base_spread_pct") is not None]
    verdicts = [r["rows"][wid]["verdict"] for r in runs if wid in r["rows"]]
    p50s, p95s = [], []
    for r in runs:
        for d in r["distribution"].get(wid, []):
            if d.get("p50_us") is not None:
                p50s.append(d["p50_us"])
            if d.get("p95_us") is not None:
                p95s.append(d["p95_us"])
    per_row[wid] = {
        "runs": len(verdicts),
        "verdicts": {v: verdicts.count(v) for v in sorted(set(verdicts))},
        "base_spread_pct": {
            "min": round(min(spreads), 1), "max": round(max(spreads), 1),
            "median": round(st.median(spreads), 1),
        } if spreads else None,
        "p50_us": {"min": min(p50s), "max": max(p50s),
                   "median": round(st.median(p50s), 1)} if p50s else None,
        "p95_us": {"min": min(p95s), "max": max(p95s),
                   "median": round(st.median(p95s), 1)} if p95s else None,
        "budget_latency_pct": next(
            (r["rows"][wid]["budget_latency_pct"] for r in runs if wid in r["rows"]), None),
    }

summary = {
    "host": json.loads(host_json),
    "artifacts_dir": sys.argv[4],
    "runs": runs,
    "per_row": per_row,
}
with open(out_path, "w") as f:
    json.dump(summary, f, indent=2)

print()
print(f"host: {summary['host']['label']}  cores={summary['host']['cores']}  virt={summary['host']['virt']}")
print()
hdr = f"{'row':44} {'runs':>4} {'spread% min/med/max':>22} {'p50 us':>9} {'p95 us med/max':>16}  verdicts"
print(hdr)
print("-" * len(hdr))
for wid, s in per_row.items():
    sp = s["base_spread_pct"]
    sp_s = f"{sp['min']:.0f}/{sp['median']:.0f}/{sp['max']:.0f}" if sp else "-"
    p50 = f"{s['p50_us']['median']:.0f}" if s["p50_us"] else "-"
    p95 = f"{s['p95_us']['median']:.0f}/{s['p95_us']['max']:.0f}" if s["p95_us"] else "-"
    verdicts = ",".join(f"{k}x{v}" for k, v in s["verdicts"].items())
    print(f"{wid.replace('hashline_ext/',''):44} {s['runs']:>4} {sp_s:>22} {p50:>9} {p95:>16}  {verdicts}")

loads = [r["load"] for r in runs]
def agg(key):
    vals = [l[key]["mean"] for l in loads if l.get(key)]
    return f"{min(vals):.2f}-{max(vals):.2f}" if vals else "n/a"
steals = [l["steal_ticks_delta"] for l in loads if l.get("steal_ticks_delta") is not None]
print()
print(f"cpu PSI some avg10 across runs: {agg('cpu_some_avg10')}")
print(f"cpu PSI full avg10 across runs: {agg('cpu_full_avg10')}")
print(f"mem PSI some avg10 across runs: {agg('mem_some_avg10')}")
print(f"steal ticks per run: {min(steals)}-{max(steals)}" if steals else "steal: n/a")
print()
print(f"written: {out_path}")
print(f"raw artifacts: {sys.argv[4]}")
print("  per-run A/B documents, telemetry samples and run logs, retained")
print("  outside the scratch directory so this pilot stays auditable after")
print("  a disposable host is destroyed.")
print()
print("To compare two hosts, run this unchanged on each and diff the per_row")
print("base_spread_pct and p95_us blocks. A host is better for gating if its")
print("spread median drops below the row budget while p50 stays comparable --")
print("a lower p50 with equally wide spread means faster hardware, not a")
print("more stable gate.")
PY
