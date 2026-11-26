#!/usr/bin/env bash
# Post a release message to Slack (#dev-general).
#
#   slack_release_notify.sh <started|released|failed|cancelled> <name> <version> [notes.md]
#
#   started    the release run began; carries the release notes
#   released   the release is live (in prod, or published)
#   failed     the run stopped before that
#   cancelled  somebody cancelled it
#
# Environment:
#   SLACK_RELEASE_WEBHOOK_URL  incoming webhook of the channel. Unset: logs a
#                              skip and exits 0.
#   RELEASE_URL                optional link put on "released" (default: the
#                              GitHub release of <version>).
#   RELEASE_WHERE              optional, what "released" says after "is"
#                              (default: "live in production").
#   SLACK_DRY_RUN=1            print the payload instead of posting it.
#
# It never fails the job: a release must not stop, or look failed, because
# Slack did not answer. The same file is copied in every repository that
# releases (tellers-app, tellers-backend, tellers-cli, tellers-media-engine):
# change them together.
set -uo pipefail

kind="${1:?usage: $0 <started|released|failed|cancelled> <name> <version> [notes.md]}"
name="${2:?missing the product name}"
version="${3:?missing the version}"
notes_file="${4:-}"

# A section block holds 3000 characters; leave room for the header lines.
max_notes_chars=2500

server="${GITHUB_SERVER_URL:-https://github.com}"
repo="${GITHUB_REPOSITORY:-}"
run_url="$server/$repo/actions/runs/${GITHUB_RUN_ID:-}"
release_url="${RELEASE_URL:-$server/$repo/releases/tag/$version}"
actor="${GITHUB_TRIGGERING_ACTOR:-${GITHUB_ACTOR:-}}"

# GitHub markdown -> Slack mrkdwn. Slack wants &, < and > escaped, has one
# star for bold, no headings, and writes links as <url|text>: markdown links
# first, then the bare pull-request and compare URLs of generated notes.
to_mrkdwn() {
  sed -E \
    -e 's/\r$//' \
    -e '/^[[:space:]]*<!--.*-->[[:space:]]*$/d' \
    -e 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g' \
    -e 's/^#{1,6}[[:space:]]+(.*)$/*\1*/' \
    -e 's/^([[:space:]]*)[*-][[:space:]]+/\1• /' \
    -e 's/\*\*([^*]+)\*\*/*\1*/g' \
    -e 's#\[([^]]+)\]\((https?://[^)[:space:]]+)\)#<\2|\1>#g' \
    -e 's#(^|[^<])(https://github\.com/[^[:space:])|]+/pull/([0-9]+))#\1<\2|\#\3>#g' \
    -e 's#(^|[^<])(https://github\.com/[^[:space:])|]+/compare/([^[:space:])|]+))#\1<\2|\3>#g'
}

# Whole lines only, so a cut never lands inside a link.
truncate_lines() {
  awk -v max="$max_notes_chars" -v more="$1" '
    { total += length($0) + 1 }
    total <= max { print; next }
    { dropped++ }
    END { if (dropped) printf "_…and %d more lines: <%s|the full notes>_\n", dropped, more }
  '
}

case "$kind" in
  started)
    text=":rocket: *$name $version*: release started"
    [ -n "$actor" ] && text+=" by $actor"
    text+=" (<$run_url|run>)"
    ;;
  released)
    text=":white_check_mark: *$name $version* is ${RELEASE_WHERE:-live in production} (<$release_url|release notes>)"
    ;;
  failed)
    text=":x: *$name $version*: the release failed, nothing more will ship from this run (<$run_url|run>)"
    ;;
  cancelled)
    text=":no_entry_sign: *$name $version*: the release was cancelled (<$run_url|run>)"
    ;;
  *)
    echo "::warning::slack_release_notify: unknown kind '$kind', nothing sent."
    exit 0
    ;;
esac

if [ -n "$notes_file" ] && [ -s "$notes_file" ]; then
  notes="$(to_mrkdwn < "$notes_file" | cat -s | truncate_lines "$run_url")"
  [ -n "$notes" ] && text+=$'\n\n'"$notes"
fi

payload="$(jq -n --arg text "$text" '{
  text: $text,
  blocks: [{type: "section", text: {type: "mrkdwn", text: $text}}],
  unfurl_links: false,
  unfurl_media: false
}')" || {
  echo "::warning::slack_release_notify: could not build the payload, nothing sent."
  exit 0
}

if [ "${SLACK_DRY_RUN:-}" = "1" ]; then
  printf '%s\n' "$payload"
  exit 0
fi

if [ -z "${SLACK_RELEASE_WEBHOOK_URL:-}" ]; then
  echo "SLACK_RELEASE_WEBHOOK_URL is not set: skipping the Slack message ($kind $name $version)."
  exit 0
fi

# The URL is the credential: keep it out of the process list and the log.
if ! printf '%s' "$payload" | curl --silent --show-error --fail-with-body \
  --max-time 20 --retry 2 \
  -H 'Content-Type: application/json' --data-binary @- \
  --config <(printf 'url = "%s"\n' "$SLACK_RELEASE_WEBHOOK_URL"); then
  echo
  echo "::warning::Slack did not accept the '$kind' message for $name $version."
fi
exit 0
