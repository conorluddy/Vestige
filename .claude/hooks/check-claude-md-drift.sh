#!/usr/bin/env bash
# SessionEnd hook: nudge if CLAUDE.md looks stale vs. recent code changes.
# Never blocks; stderr-only; exit 0 always.

set -u
cd "$(dirname "$0")/../.." || exit 0
[ -f CLAUDE.md ] || exit 0
command -v git >/dev/null 2>&1 || exit 0
git rev-parse --git-dir >/dev/null 2>&1 || exit 0

last=$(git log -1 --format=%H -- CLAUDE.md 2>/dev/null)
[ -z "$last" ] && exit 0

watched=(
  'Cargo.toml'
  'crates/*/Cargo.toml'
  'crates/*/src'
  'crates/vestige-store/src/migrations'
  'crates/vestige-cli/src/commands'
  'crates/vestige-mcp/src/tools'
  'vestige_prd.md'
  'docs/prd'
)

count=$(git log --oneline "${last}..HEAD" -- "${watched[@]}" 2>/dev/null | wc -l | tr -d ' ')
[ "$count" = "0" ] && exit 0

since=$(git log -1 --format=%cs "$last" 2>/dev/null)
top=$(git log --name-only --pretty=format: "${last}..HEAD" -- "${watched[@]}" 2>/dev/null \
  | grep -v '^$' | sort | uniq -c | sort -rn | head -5 | awk '{print $2}' | paste -sd ',' - | sed 's/,/, /g')

printf '\n⚠ CLAUDE.md may be stale — %s commit(s) since %s touched watched paths.\n   Top paths: %s\n   Review: git log --since=%s -- %s\n\n' \
  "$count" "$since" "$top" "$since" "${watched[*]}" >&2

exit 0
