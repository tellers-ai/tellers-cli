#!/usr/bin/env bash
# Print a Markdown changelog section for the commits since the latest vX.Y.Z tag,
# grouped by conventional-commit type.
#
# Usage: scripts/release_notes.sh <version> [repo]
#   version  e.g. v0.1.3 (used as the heading)
#   repo     owner/name used for PR links (default: $GITHUB_REPOSITORY or tellers-ai/tellers-cli)
set -euo pipefail

VERSION="${1:?usage: release_notes.sh <version> [repo]}"
REPO="${2:-${GITHUB_REPOSITORY:-tellers-ai/tellers-cli}}"

LAST_TAG=$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -n1)
RANGE="${LAST_TAG:+${LAST_TAG}..}HEAD"

features="" fixes="" perf="" refactor="" docs="" other=""
while IFS= read -r subject; do
  [ -z "$subject" ] && continue
  # Release bump commits don't belong in the notes.
  [[ "$subject" =~ ^chore\(release\) ]] && continue

  # "feat(scope)!: message" -> type=feat, scope=scope, message=message
  if [[ "$subject" =~ ^([a-z]+)(\(([^\)]*)\))?!?:\ (.*)$ ]]; then
    type="${BASH_REMATCH[1]}" scope="${BASH_REMATCH[3]}" msg="${BASH_REMATCH[4]}"
  else
    type="" scope="" msg="$subject"
  fi
  # Link "(#123)" to the PR.
  msg=$(sed -E "s|\(#([0-9]+)\)|([#\1](https://github.com/${REPO}/pull/\1))|g" <<< "$msg")
  line="- ${scope:+**${scope}:** }${msg}"$'\n'

  case "$type" in
    feat) features+="$line" ;;
    fix) fixes+="$line" ;;
    perf) perf+="$line" ;;
    refactor) refactor+="$line" ;;
    docs) docs+="$line" ;;
    *) other+="$line" ;;
  esac
done < <(git log --no-merges --format=%s "$RANGE")

echo "## ${VERSION} - $(date -u +%Y-%m-%d)"
echo
section() { if [ -n "$2" ]; then printf '### %s\n\n%s\n' "$1" "$2"; fi; }
section "Features" "$features"
section "Bug Fixes" "$fixes"
section "Performance" "$perf"
section "Refactoring" "$refactor"
section "Documentation" "$docs"
section "Other" "$other"
if [ -z "$features$fixes$perf$refactor$docs$other" ]; then
  printf 'No changes.\n\n'
fi
