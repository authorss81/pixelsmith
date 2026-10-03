#!/usr/bin/env bash
# =============================================================================
# phase_runner.sh — run ONE invocation of a phase and record its state.
#
# This script NEVER sleeps inside a job. Retries are driven by the GitHub Actions
# schedule in .github/workflows/automation.yml — each scheduled run invokes this
# script for the current phase. If the model is rate-limited, the phase is marked
# DEFERRED and the script exits fast, costing near-zero minutes. The next tick
# retries it.
#
# State markers, all in workspace/<phase>/:
#   .attempted  this run produced real work and it was committed; awaiting verify
#   .done       verification passed (written by the workflow, NOT by this script)
#   .deferred   hit a rate limit, retry on a later tick
#   .blocked    needs a human
#   .attempts   failed-attempt counter (real failures only)
#   .deferred_attempts  rate-limit counter
#   .no_work    opencode exited 0 but changed nothing
#   .session    session id to resume
#   .timeout    per-phase job budget in minutes (default 60)
#   .checkpoint prior partial work was merged from a WIP branch
#
# Usage:
#   phase_runner.sh PHASE_NAME
#   phase_runner.sh PHASE_NAME --review-only
# =============================================================================

set -euo pipefail

PHASE="${1:-}"
MODE="${2:-}"

if [ -z "${PHASE}" ]; then
  echo "ERROR: no phase given." >&2
  exit 2
fi

# --- Model fallback chain ------------------------------------------------------
# Ranked by coding ability among live free models. Entries are tried one at a
# time via --model; the raw list is never passed to opencode verbatim.
MODEL_LIST="${OPENCODE_MODEL:-opencode/space-bunny-free,opencode-go/space-bunny-free,opencode/space-bunny-free}"
MODELS=()
while IFS=',' read -r -a _RAW; do
  for _m in "${_RAW[@]}"; do
    _m="${_m#"${_m%%[![:space:]]*}"}"
    _m="${_m%"${_m##*[![:space:]]}"}"
    [ -n "${_m}" ] && MODELS+=("${_m}")
  done
done <<< "${MODEL_LIST}"
if [ "${#MODELS[@]}" -eq 0 ]; then MODELS=("opencode/space-bunny-free"); fi

LOG_DIR="logs"
PHASE_DIR="workspace/${PHASE}"
PROMPT_FILE="${PHASE_DIR}/PROMPT.md"
ATTEMPTED_FILE="${PHASE_DIR}/.attempted"
DONE_FILE="${PHASE_DIR}/.done"
DEFERRED_FILE="${PHASE_DIR}/.deferred"
SESSION_FILE="${PHASE_DIR}/.session"
BLOCKED_FILE="${PHASE_DIR}/.blocked"
ATTEMPTS_FILE="${PHASE_DIR}/.attempts"
DEFERRED_ATTEMPTS_FILE="${PHASE_DIR}/.deferred_attempts"
NOWORK_FILE="${PHASE_DIR}/.no_work"
VERIFY_FAILURES_FILE="${PHASE_DIR}/.verify_failures"
STOP_FILE="workspace/.stop"

MAX_ATTEMPTS="${MAX_ATTEMPTS:-3}"
MAX_DEFERRALS="${MAX_DEFERRALS:-5}"
MAX_VERIFY_FAILURES="${MAX_VERIFY_FAILURES:-3}"
CHECKPOINT_INTERVAL="${CHECKPOINT_INTERVAL_SECONDS:-300}"
WIP_BRANCH="px-wip/${PHASE}"
REVIEWER_AGENT="${REVIEWER_AGENT:-reviewer}"

[ -f "${PROMPT_FILE}" ] || { echo "ERROR: no PROMPT.md for ${PHASE}" >&2; exit 2; }
mkdir -p "${LOG_DIR}"

# --- Halt switch ---------------------------------------------------------------
if [ -f "${STOP_FILE}" ]; then
  echo "== [phase] STOP marker present — pipeline halted. Remove workspace/.stop to resume."
  exit 0
fi

# --- Log classification --------------------------------------------------------
# Only explicit HTTP/quota markers count. The bare word "retry" must NOT match,
# or normal agent chatter about retrying a build would be misread as a limit.
log_is_rate_limited() {
  grep -qiE "HTTP[ /]?429|429[^0-9]|too many requests|rate[ _-]?limit(ed| exceeded)?|insufficient[ _-]?quota|quota exceeded|(per[ -]?minute|per[ -]?day).*(limit|exceeded)" "$1" 2>/dev/null
}

log_is_model_error() {
  grep -qiE "model not found|unknown model|invalid model|no such model|unexpected server error|upstream error|temporarily overloaded|streaming response failed|502|503|504|overloaded" "$1" 2>/dev/null
}

# Run opencode once per model in the chain. Advances the chain ONLY on
# model-level failure (unknown id / rate limit / provider outage / zero output).
# A real work failure returns immediately — re-running the whole task with
# another model inside one tick would double-spend minutes; the attempt cap
# handles retries.
ACTIVE_MODEL=""
run_models() {
  local logfile="$1"; shift
  local m code pre post growth
  : > "${logfile}"
  for m in "${MODELS[@]}"; do
    echo "== [models] trying ${m} ==" >> "${logfile}"
    pre="$(wc -c < "${logfile}" 2>/dev/null || echo 0)"
    # Re-open the context file per attempt. Piping it once from the outer scope
    # leaves stdin exhausted after the first model, so the fallback could never
    # actually retry.
    if [ -f "${LOG_DIR}/${PHASE}.ctx" ]; then
      opencode run --model "${m}" "$@" < "${LOG_DIR}/${PHASE}.ctx" >> "${logfile}" 2>&1 || code=$?
    else
      opencode run --model "${m}" "$@" >> "${logfile}" 2>&1 || code=$?
    fi
    code="${code:-0}"
    post="$(wc -c < "${logfile}" 2>/dev/null || echo 0)"
    growth=$((post - pre))
    # NOTE: do not `unset code` here. `code` is declared `local`, so it starts
    # unset, and the default above covers that. An earlier revision unset it
    # after reading it and then read it again on the next line, which `set -u`
    # turns into "code: unbound variable" — killing every phase before opencode
    # was ever invoked.
    if [ "${code}" -eq 0 ]; then
      ACTIVE_MODEL="${m}"
      echo "== [models] ${m} succeeded =="
      return 0
    fi
    if log_is_rate_limited "${logfile}" || log_is_model_error "${logfile}" || [ "${growth}" -lt 200 ]; then
      echo "== [models] ${m} unusable (exit ${code}) — advancing chain =="
      continue
    fi
    echo "== [models] ${m} failed with a real work error (exit ${code}) — keeping result =="
    ACTIVE_MODEL="${m}"
    return "${code}"
  done
  echo "== [models] every model in the chain was skipped/unusable =="
  return 1
}

# --- Environment pre-flight ----------------------------------------------------
# Do not burn an attempt on a missing binary or a bad key.
env_check() {
  if ! command -v opencode >/dev/null 2>&1; then
    echo "== [env] opencode binary missing — retryable, not counted =="
    return 1
  fi
  if [ -z "${OPENCODE_API_KEY:-}" ]; then
    echo "== [env] OPENCODE_API_KEY unset — BLOCKED =="
    touch "${BLOCKED_FILE}"
    rm -f "${DEFERRED_FILE}" "${SESSION_FILE}" "${ATTEMPTS_FILE}" "${DEFERRED_ATTEMPTS_FILE}"
    return 2
  fi
  if ! opencode models >/dev/null 2>&1; then
    echo "== [env] auth probe failed (bad key?) — BLOCKED =="
    touch "${BLOCKED_FILE}"
    rm -f "${DEFERRED_FILE}" "${SESSION_FILE}" "${ATTEMPTS_FILE}" "${DEFERRED_ATTEMPTS_FILE}"
    return 2
  fi
  return 0
}

# Newest session whose title matches this phase, so a retry resumes the same
# thread instead of the newest unrelated one.
find_session() {
  if command -v python3 >/dev/null 2>&1; then
    opencode session list --format json 2>/dev/null | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
prefix = 'px-${PHASE}'
best, best_updated = None, -1
for s in data:
    t = s.get('title', '') or ''
    if t.startswith(prefix) and int(s.get('updated', 0) or 0) > best_updated:
        best, best_updated = s.get('id', ''), int(s.get('updated', 0))
if best:
    sys.stdout.write(best)
" 2>/dev/null || true
  else
    opencode session list --format json 2>/dev/null \
      | grep -o '"id":"[^"]*"' | head -n1 | cut -d'"' -f4 || true
  fi
}

build_context_header() {
  {
    echo "# PIPELINE CONTEXT (injected by phase_runner.sh — do not delete)"
    echo ""
    echo "## Hard rules (AGENTS.md) — read the file, this is only a summary"
    if [ -f AGENTS.md ]; then
      sed -n '/^## Hard rules/,/^## Conventions/p' AGENTS.md | head -n 80
    fi
    echo ""
    echo "## Orientation (read these yourself — do NOT rely on memory)"
    echo "- docs/ARCHITECTURE.md   current code shape, module map, gotchas. READ IT."
    echo "- docs/phase-status.md   truth table. READ IT, then UPDATE your row."
    echo "- workspace/PHASES.md    the full phase table."
    echo "- ROADMAP.md             the intent behind all of this."
    echo ""
    if [ -f "${PHASE_DIR}/.checkpoint" ]; then
      echo "## CONTINUATION MODE"
      echo "A previous run started this phase; its partial work is already"
      echo "committed (see the working tree and recent commits). DO NOT start"
      echo "over. Inspect what exists, CONTINUE from it, and finish the phase."
      echo ""
    fi
    echo "## Your task"
    echo "Execute the phase described in ${PROMPT_FILE}. Read that file now and"
    echo "complete it fully. This context is orientation only."
    echo ""
  } > "${LOG_DIR}/${PHASE}.ctx" 2>/dev/null
  echo "== [phase] context header built"
}

# --- Evidence gate ------------------------------------------------------------
# opencode exits 0 even when a session does nothing at all. A phase is only
# marked ATTEMPTED when the run left real changes. Pipeline artefacts (logs,
# hidden markers) are not evidence.
tree_work() {
  git status --porcelain 2>/dev/null \
    | sed -E 's/^(.{2})[[:space:]]+//' \
    | grep -vE "^logs/|^workspace/${PHASE}/\.|^workspace/\.[^/]*($|/)" || true
}

has_new_work() {
  local before="$1" after new
  after="$(tree_work)"
  new="$(comm -13 <(printf '%s\n' "${before}" | sort -u) <(printf '%s\n' "${after}" | sort -u))"
  [ -n "${new}" ]
}

has_new_commits() {
  local before="$1"
  [ -n "${before}" ] || return 1
  [ "${before}" = "$(git rev-parse HEAD 2>/dev/null)" ] && return 1
  git diff --name-only "${before}"..HEAD 2>/dev/null \
    | grep -vE "^logs/|^workspace/${PHASE}/\.|^workspace/\.[^/]*($|/)" | grep -q .
}

git_available() { git rev-parse --git-dir >/dev/null 2>&1; }

# --- Checkpoint machinery -----------------------------------------------------
# A job timeout kills the VM and erases everything since the last push. Snapshot
# to a WIP branch every CHECKPOINT_INTERVAL so the next tick can merge and carry
# on rather than restart.
checkpoint_wip() {
  if ! git status --porcelain 2>/dev/null \
       | grep -vE '^\?\? (logs/|workspace/\.)' | grep -q .; then
    return 0
  fi
  git add -A 2>/dev/null || true
  local tree commit base
  tree="$(git write-tree 2>/dev/null)" || return 0
  base="$(git rev-parse HEAD 2>/dev/null || echo HEAD)"
  commit="$(git commit-tree "${tree}" -p "${base}" -m "px: ${PHASE} checkpoint $(date -u +%s)" 2>/dev/null)" || return 0
  if git push origin "${commit}:refs/heads/${WIP_BRANCH}" --force 2>/dev/null; then
    echo "== [checkpoint] WIP pushed: ${WIP_BRANCH} @ ${commit:0:8} =="
  fi
  git reset -q 2>/dev/null || true
}

checkpoint_loop() {
  while true; do
    sleep "${CHECKPOINT_INTERVAL}"
    checkpoint_wip
  done
}

resume_wip() {
  if git ls-remote --exit-code origin "refs/heads/${WIP_BRANCH}" >/dev/null 2>&1; then
    echo "== [phase] continuing from prior partial work (${WIP_BRANCH}) =="
    git fetch origin "${WIP_BRANCH}" 2>/dev/null || true
    git merge --no-edit FETCH_HEAD 2>/dev/null || true
    git push origin main 2>/dev/null || true
    touch "${PHASE_DIR}/.checkpoint"
  fi
}

clear_wip() {
  git push origin --delete "${WIP_BRANCH}" 2>/dev/null || true
  rm -f "${PHASE_DIR}/.checkpoint"
}

run_phase() {
  echo "== [phase] Running: ${PHASE} =="
  resume_wip
  build_context_header
  local SESSION_ARGS=()
  if [ -f "${SESSION_FILE}" ]; then
    local sid; sid="$(cat "${SESSION_FILE}")"
    [ -n "${sid}" ] && SESSION_ARGS=(--session "${sid}")
  fi
  # Commit the full ctx+PROMPT as an audit record, but send only the compact
  # context header over stdin; it points the agent at the PROMPT file, which the
  # agent reads itself. Keeps the per-phase context footprint small.
  { cat "${LOG_DIR}/${PHASE}.ctx"; cat "${PROMPT_FILE}"; } > "${LOG_DIR}/${PHASE}.prompt"

  set +e
  checkpoint_loop &
  local CHECK_PID=$!
  # ${arr[@]+"${arr[@]}"} is the portable idiom for expanding an array that may
  # be empty under `set -u`. Plain "${SESSION_ARGS[@]}" is safe on bash 4.4+ but
  # hard-fails on older bash, and a phase that dies before opencode runs costs a
  # whole job to discover.
  run_models "${LOG_DIR}/${PHASE}.log" --agent build \
    ${SESSION_ARGS[@]+"${SESSION_ARGS[@]}"} --title "px-${PHASE}"
  local code=$?
  kill "${CHECK_PID}" 2>/dev/null || true
  set -e

  [ -n "${ACTIVE_MODEL}" ] && echo "== [phase] model used: ${ACTIVE_MODEL} =="

  # Sessions live on the runner's local disk, which is wiped between runs. A
  # committed .session id then points at nothing and every retry fails with
  # "Session not found". Clear it and return a retryable code.
  if [ "${code}" -ne 0 ] && grep -qi "Session not found" "${LOG_DIR}/${PHASE}.log"; then
    echo "== [phase] STALE SESSION — clearing marker, retrying fresh next tick =="
    rm -f "${SESSION_FILE}"
    return 5
  fi
  return "${code}"
}

run_review() {
  echo "== [review] Running reviewer subagent =="
  set +e
  run_models "${LOG_DIR}/${PHASE}.review.log" --agent "${REVIEWER_AGENT}" \
    "Review all changes made in phase '${PHASE}'. Output numbered FINDINGS."
  local code=$?
  set -e
  echo "== [review] exit: ${code} =="

  if grep -qiE "FINDINGS:[[:space:]]*[0-9]+|^[[:space:]]*[0-9]+\." "${LOG_DIR}/${PHASE}.review.log"; then
    echo "== [fix] Applying review findings =="
    set +e
    checkpoint_loop &
    local CHECK_PID=$!
    run_models "${LOG_DIR}/${PHASE}.fix.log" --agent build --continue \
      "Apply the fixes for the review FINDINGS above. Do not break other code. After applying every fix, commit and push them yourself: git add -A; git commit -m 'px: ${PHASE} review fixes'; git push (pull --rebase on rejection). If the working tree is clean, push nothing."
    code=$?
    kill "${CHECK_PID}" 2>/dev/null || true
    set -e
    echo "== [fix] exit: ${code} =="
  fi

  # Safety net: if the fix agent committed but could not push, or left fixes
  # uncommitted, commit and push here so a later timeout cannot lose them.
  if [ "$(git status --porcelain 2>/dev/null | grep -vE '^\?\? (logs/|workspace/\.)' | wc -l)" -gt 0 ]; then
    echo "== [review] committing + pushing review fixes (safety net) =="
    git config user.name "pixelsmith-bot"
    git config user.email "pixelsmith-bot@users.noreply.github.com"
    git add -A
    git commit -m "px: ${PHASE} review fixes" 2>/dev/null || true
    local pushed=0 i
    for i in 1 2 3 4 5; do
      if git push origin main 2>/dev/null; then pushed=1; break; fi
      git pull --rebase origin main 2>/dev/null || true
      sleep 3
    done
    [ "${pushed}" = "0" ] && echo "::warning::review fixes not pushed after 5 attempts"
  fi
}

# --- Entry points -------------------------------------------------------------
env_check
ENV_CODE=$?
case "${ENV_CODE}" in
  2) echo "== [phase] BLOCKED during env pre-flight: ${PHASE}"; exit 3 ;;
  1) echo "== [phase] ENV ERROR (opencode missing): ${PHASE} — retryable, not counted"; exit 5 ;;
esac

if [ "${MODE}" = "--review-only" ]; then
  if [ -f "${DONE_FILE}" ]; then
    echo "== [phase] ${PHASE} already verified — running review only =="
    run_review
  else
    echo "== [phase] ${PHASE} not done yet — nothing to review =="
  fi
  exit 0
fi

# A verified phase must never re-run, even if a stale deferred marker got it
# re-selected. Clear the stale markers so the selector stops picking it.
if [ -f "${DONE_FILE}" ]; then
  echo "== [phase] ${PHASE} already DONE — skipping, clearing stale markers =="
  rm -f "${DEFERRED_FILE}" "${SESSION_FILE}" "${BLOCKED_FILE}" "${ATTEMPTS_FILE}" \
        "${DEFERRED_ATTEMPTS_FILE}" "${NOWORK_FILE}" "${VERIFY_FAILURES_FILE}"
  exit 0
fi

WORK_BEFORE="$(tree_work)"
HEAD_BEFORE="$(git rev-parse HEAD 2>/dev/null || true)"

RUN_OK=0
if run_phase; then RUN_OK=1; else
  RUN_CODE=$?
  if [ "${RUN_CODE}" = "5" ]; then
    echo "== [phase] RETRYABLE INFRA ERROR (exit 5): ${PHASE} — not counted =="
    exit 5
  fi
fi

if [ "${RUN_OK}" = "1" ]; then
  if ! git_available; then
    echo "== [phase] WARNING: git unavailable — evidence gate skipped, flagging for review =="
    touch "${ATTEMPTED_FILE}"
    exit 0
  fi
  if has_new_work "${WORK_BEFORE}" || has_new_commits "${HEAD_BEFORE}"; then
    echo "== [phase] SUCCESS + evidence gate passed — awaiting verification =="
    # NOT .done. The workflow writes that only after scripts/verify.sh passes.
    touch "${ATTEMPTED_FILE}"
    rm -f "${DEFERRED_FILE}" "${NOWORK_FILE}" "${SESSION_FILE}" "${ATTEMPTS_FILE}" "${DEFERRED_ATTEMPTS_FILE}"
    clear_wip
    exit 0
  fi
  echo "== [phase] NO-WORK FAILURE: opencode exited 0 but changed nothing — not marking attempted =="
  touch "${NOWORK_FILE}"
else
  echo "== [phase] opencode exited non-zero: ${PHASE}"
fi

# --- Failure classification ---------------------------------------------------
if log_is_rate_limited "${LOG_DIR}/${PHASE}.log"; then
  DEFERRED_ATTEMPT=0
  [ -f "${DEFERRED_ATTEMPTS_FILE}" ] && DEFERRED_ATTEMPT="$(cat "${DEFERRED_ATTEMPTS_FILE}" 2>/dev/null || echo 0)"
  DEFERRED_ATTEMPT=$((DEFERRED_ATTEMPT + 1))
  printf '%s' "${DEFERRED_ATTEMPT}" > "${DEFERRED_ATTEMPTS_FILE}"

  if [ "${DEFERRED_ATTEMPT}" -ge "${MAX_DEFERRALS}" ]; then
    echo "== [phase] RATE-LIMITED ${DEFERRED_ATTEMPT}/${MAX_DEFERRALS}: ${PHASE} BLOCKED =="
    touch "${BLOCKED_FILE}"
    rm -f "${DEFERRED_FILE}" "${SESSION_FILE}" "${ATTEMPTS_FILE}" "${DEFERRED_ATTEMPTS_FILE}"
    exit 3
  fi

  echo "== [phase] RATE-LIMITED ${DEFERRED_ATTEMPT}/${MAX_DEFERRALS}: deferred, will retry =="
  touch "${DEFERRED_FILE}"
  LAST_SID="$(find_session)"
  [ -n "${LAST_SID}" ] && printf '%s' "${LAST_SID}" > "${SESSION_FILE}"
  exit 42
fi

ATTEMPT=0
[ -f "${ATTEMPTS_FILE}" ] && ATTEMPT="$(cat "${ATTEMPTS_FILE}" 2>/dev/null || echo 0)"
ATTEMPT=$((ATTEMPT + 1))
printf '%s' "${ATTEMPT}" > "${ATTEMPTS_FILE}"

if [ "${ATTEMPT}" -ge "${MAX_ATTEMPTS}" ]; then
  echo "== [phase] FAILED ${ATTEMPT}/${MAX_ATTEMPTS}: ${PHASE} BLOCKED — manual intervention =="
  touch "${BLOCKED_FILE}"
  rm -f "${DEFERRED_FILE}" "${SESSION_FILE}" "${DEFERRED_ATTEMPTS_FILE}"
  exit 3
fi

echo "== [phase] FAILED attempt ${ATTEMPT}/${MAX_ATTEMPTS}: ${PHASE} — see ${LOG_DIR}/${PHASE}.log"
exit 1