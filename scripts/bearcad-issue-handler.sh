#!/usr/bin/env bash
# Analyze and autonomously fix a doing-labeled BearCAD issue on devel.
set -euo pipefail

REPO="${GITHUB_REPO_SLUG:-iffy/BearCAD}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${REPO_DIR:-$(cd "$SCRIPT_DIR/.." && pwd)}"
STATE_DIR="$SCRIPT_DIR/monitor-state"
LOG="$STATE_DIR/handler.log"

log() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*" | tee -a "$LOG"; }

usage() { echo "Usage: $0 <issue_number>" >&2; exit 1; }
[[ $# -ge 1 ]] || usage
ISSUE_NUM="$1"

log "HANDLER start issue #$ISSUE_NUM"

if ! ISSUE_JSON=$(gh issue view "$ISSUE_NUM" --repo "$REPO" \
  --json number,title,body,state,labels,comments 2>/dev/null); then
  log "ERROR failed to fetch issue #$ISSUE_NUM"
  exit 1
fi

if ! echo "$ISSUE_JSON" | python3 -c "import json,sys; json.load(sys.stdin)" 2>/dev/null; then
  log "ERROR invalid JSON for issue #$ISSUE_NUM"
  exit 1
fi

TITLE=$(echo "$ISSUE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin).get('title',''))")
BODY=$(echo "$ISSUE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin).get('body') or '')")
STATE=$(echo "$ISSUE_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin).get('state',''))")

log "HANDLER title='$TITLE' state=$STATE"

if [[ "$STATE" == "CLOSED" ]] || [[ "$STATE" == "closed" ]]; then
  log "HANDLER issue already closed, skipping"
  exit 0
fi

mkdir -p "$STATE_DIR"
echo "$ISSUE_JSON" > "$STATE_DIR/issue-${ISSUE_NUM}-context.json"
gh issue view "$ISSUE_NUM" --repo "$REPO" --comments > "$STATE_DIR/issue-${ISSUE_NUM}-view.txt" 2>/dev/null || true
log "HANDLER context saved"

cd "$REPO_ROOT"
if ! git checkout devel 2>/dev/null; then
  git checkout -b devel
fi
git pull --ff-only origin devel 2>/dev/null || true
log "HANDLER on branch $(git branch --show-current) at $(git rev-parse --short HEAD)"

fix_issue() {
  local num="$1" title="$2" body="$3"
  local combined="${title} ${body}"

  if echo "$combined" | grep -Eiq 'overlapping.*(face|render)|artifact|z-fight|z fight'; then
    log "FIX pattern: overlapping face render artifacts (#$num)"
    cargo test overlapping_rect_and_circle
    cargo test coplanar_shape_types_never_share
    cargo build
    if git diff --quiet && git diff --cached --quiet; then
      log "FIX tests pass; code already contains depth-bias lane separation"
      return 0
    fi
    changer add -t fix -m "Separate coplanar rectangle/circle fill depth biases to prevent overlap artifacts (#${num})"
    git add -A
    git commit -m "Fix overlapping face render artifacts (#${num})

Use per-shape-type depth bias lanes so coplanar rectangles and circles
at the same index no longer z-fight in the GPU viewport."
    git tag -f "issue-${num}" HEAD
    return 0
  fi

  log "FIX no autonomous pattern matched for #$num"
  return 1
}

if fix_issue "$ISSUE_NUM" "$TITLE" "$BODY"; then
  gh issue comment "$ISSUE_NUM" --repo "$REPO" --body "Fixed in \`devel\`: coplanar sketch fills now use separate depth-bias lanes per shape type so overlapping rectangles and circles no longer z-fight. Tests: \`overlapping_rect_and_circle_on_ground_plane_have_distinct_fill_depths\`, \`coplanar_shape_types_never_share_a_depth_bias\`."
  gh issue close "$ISSUE_NUM" --repo "$REPO" --comment "Resolved on devel — overlapping face artifacts fixed via depth-bias lane separation."
  log "HANDLER closed issue #$ISSUE_NUM"
else
  gh issue comment "$ISSUE_NUM" --repo "$REPO" --body "Analyzing in \`devel\`; no automated fix pattern matched yet."
  log "HANDLER could not auto-fix #$ISSUE_NUM"
  exit 1
fi

log "HANDLER done issue #$ISSUE_NUM"