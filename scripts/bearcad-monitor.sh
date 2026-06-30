#!/usr/bin/env bash
# Poll iffy/BearCAD for activity on issues labeled `doing` and dispatch the handler.
set -euo pipefail

REPO="${GITHUB_REPO_SLUG:-iffy/BearCAD}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
STATE_DIR="$SCRIPT_DIR/monitor-state"
LOG="$STATE_DIR/monitor.log"
CHECKPOINT="$STATE_DIR/checkpoint.json"
ACTIVITY="$STATE_DIR/activity.jsonl"
PIDFILE="$STATE_DIR/monitor.pid"
POLL_INTERVAL="${POLL_INTERVAL:-60}"
MAX_CYCLES="${MAX_CYCLES:-0}"

mkdir -p "$STATE_DIR"

log() {
  echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*" >> "$LOG"
}

acquire_lock() {
  if [[ -f "$PIDFILE" ]]; then
    local old_pid
    old_pid=$(cat "$PIDFILE")
    if kill -0 "$old_pid" 2>/dev/null; then
      echo "Monitor already running (pid $old_pid)" >&2
      exit 0
    fi
  fi
  echo $$ > "$PIDFILE"
  trap 'rm -f "$PIDFILE"' EXIT
}

init_checkpoint() {
  if [[ ! -f "$CHECKPOINT" ]]; then
    echo '{"issues":{},"last_poll":null}' > "$CHECKPOINT"
  fi
}

fetch_doing_issues() {
  gh issue list --repo "$REPO" --label doing --state all \
    --json number,title,state,updatedAt,labels,comments,createdAt
}

detect_changes() {
  local current="$1"
  python3 <<PY
import json
from datetime import datetime, timezone

checkpoint_path = "$CHECKPOINT"
current = json.loads('''$current''')
with open(checkpoint_path) as f:
    cp = json.load(f)
issues = cp.get("issues", {})
changes = []
for iss in current:
    num = str(iss["number"])
    key_data = {
        "updatedAt": iss.get("updatedAt"),
        "state": iss.get("state"),
        "labels": sorted(l["name"] for l in iss.get("labels", [])),
        "comments": iss.get("comments", 0),
    }
    if num not in issues:
        changes.append({"type": "created", "number": iss["number"], "title": iss.get("title", "")})
    else:
        old = issues[num]
        if old.get("updatedAt") != key_data["updatedAt"]:
            if old.get("state") != key_data["state"]:
                changes.append({"type": "state_change", "number": iss["number"],
                    "from": old.get("state"), "to": key_data["state"]})
            if old.get("labels") != key_data["labels"]:
                changes.append({"type": "label_update", "number": iss["number"],
                    "from": old.get("labels"), "to": key_data["labels"]})
            if old.get("comments", 0) != key_data["comments"]:
                changes.append({"type": "new_comment", "number": iss["number"],
                    "from": old.get("comments", 0), "to": key_data["comments"]})
            if not any(c["number"] == iss["number"] for c in changes):
                changes.append({"type": "updated", "number": iss["number"]})
    issues[num] = key_data
current_nums = {str(i["number"]) for i in current}
for num in list(issues.keys()):
    if num not in current_nums:
        changes.append({"type": "removed_from_doing", "number": int(num)})
        del issues[num]
cp["issues"] = issues
cp["last_poll"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
with open(checkpoint_path, "w") as f:
    json.dump(cp, f)
print(json.dumps(changes))
PY
}

record_activity() {
  local payload="$1"
  echo "$payload" >> "$ACTIVITY"
}

poll_once() {
  local cycle="$1"
  log "POLL cycle=$cycle repo=$REPO label=doing"
  local issues changes change_count
  issues=$(fetch_doing_issues)
  local count
  count=$(echo "$issues" | python3 -c "import json,sys; print(len(json.load(sys.stdin)))")
  log "POLL found $count doing-labeled issue(s)"
  changes=$(detect_changes "$issues")
  change_count=$(echo "$changes" | python3 -c "import json,sys; print(len(json.load(sys.stdin)))")
  if [[ "$change_count" -gt 0 ]]; then
    log "DETECT $change_count change(s): $changes"
    echo "$changes" | python3 -c "
import json, sys
seen = set()
for c in json.load(sys.stdin):
    n = c.get('number')
    if n and n not in seen:
        seen.add(n)
        print(n)
" | while read -r num; do
      [[ -z "$num" ]] && continue
      if ! REPO_DIR="$REPO_ROOT" "$SCRIPT_DIR/bearcad-issue-handler.sh" "$num"; then
        log "WARN handler failed for #$num"
      fi
    done
  else
    log "DETECT no changes"
  fi
}

main() {
  acquire_lock
  init_checkpoint
  log "START BearCAD doing-issue monitor repo=$REPO interval=${POLL_INTERVAL}s max_cycles=$MAX_CYCLES"
  local cycle=0
  while true; do
    cycle=$((cycle + 1))
    if ! poll_once "$cycle"; then
      log "WARN poll cycle $cycle failed (continuing)"
    fi
    if [[ "$MAX_CYCLES" -gt 0 && "$cycle" -ge "$MAX_CYCLES" ]]; then
      log "STOP reached max_cycles=$MAX_CYCLES"
      break
    fi
    sleep "$POLL_INTERVAL"
  done
}

main "$@"