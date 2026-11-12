#!/bin/bash
# Builds a throwaway HOME with dummy git-bug repos, for screenshots without real projects.
# Usage: scripts/demo-home.sh <dir> && HOME=<dir> cargo run --release
set -euo pipefail

[ $# -eq 1 ] || { echo "usage: $0 <dir>" >&2; exit 1; }
DEMO="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
MARKER=".gbtracker-demo"

if [ -e "$DEMO" ]; then
  [ -e "$DEMO/$MARKER" ] || { echo "$DEMO exists and is not a demo home; refusing to delete it" >&2; exit 1; }
  rm -rf "$DEMO"
fi
mkdir -p "$DEMO/projects"
touch "$DEMO/$MARKER"
export HOME="$DEMO"

git config --global user.name "Demo User"
git config --global user.email demo@example.com
git config --global init.defaultBranch main

repo() {
  mkdir -p "$HOME/projects/$1"
  cd "$HOME/projects/$1"
  git init -q
  git commit -q --allow-empty -m init
  git bug user new -n "Demo User" -e demo@example.com --non-interactive >/dev/null
}

# issue <kind> <title> [epic-id] [state] [closed]; prints the short id
issue() {
  local id
  id=$(git bug bug new --title="$2" --message="" --non-interactive | awk '{print $1}')
  git bug bug label new "$id" "kind:$1" >/dev/null
  [ -n "${3:-}" ] && git bug bug label new "$id" "epic:$3" >/dev/null
  [ -n "${4:-}" ] && git bug bug label new "$id" "state:$4" >/dev/null
  [ -n "${5:-}" ] && git bug bug status close "$id" >/dev/null
  echo "${id:0:7}"
}

repo orbit-api
E=$(issue epic "Public API v2")
issue task "L1 — Versioned routes under /v2" "$E" "" closed >/dev/null
issue task "L2 — Per-key rate limiting" "$E" in-progress >/dev/null
issue task "L3 — OpenAPI schema and generated client" "$E" >/dev/null
issue bug "Pagination cursor skips the last page" "" blocked >/dev/null
issue bug "500 on empty JSON body in POST /items" >/dev/null

repo pixel-notes
E=$(issue epic "Offline sync")
issue task "L1 — Local SQLite store" "$E" "" closed >/dev/null
issue task "L2 — Conflict resolution on reconnect" "$E" in-progress >/dev/null
issue task "L3 — Sync status indicator" "$E" >/dev/null
issue bug "Markdown preview loses scroll position" >/dev/null
issue task "Export notes as a ZIP archive" >/dev/null

repo weather-cli
issue task "Add --units flag for metric/imperial" "" in-progress >/dev/null
issue bug "Forecast table breaks on narrow terminals" >/dev/null
issue task "Cache API responses for 10 minutes" "" "" closed >/dev/null

CONFIG="$HOME/Library/Application Support/dev.arkye03.gbtracker"
mkdir -p "$CONFIG"
cat > "$CONFIG/workspace.json" <<JSON
{"roots":[{"path":"$HOME/projects","kind":"Scan"}],"scan_depth":4,"theme":"Dark","sidebar_collapsed":false}
JSON

echo "Demo home ready. Run: HOME=$DEMO cargo run --release"
