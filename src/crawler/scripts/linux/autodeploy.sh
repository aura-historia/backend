#!/usr/bin/env bash
# Deploys the newest published crawler release, for crawler-autodeploy.timer. It asks GitHub for
# the newest crawler-<sha> release, skips it when it is the one it last handled, and otherwise runs
# deploy.sh --unattended on it. A release that adds migrations, or a rollback over applied
# migrations, is left for an operator, and the newest release is handled once: after a manual
# rollback the timer stays quiet until a newer release is published. It never downgrades: a release
# whose commit does not come after the running one on develop (an older commit released later) is
# skipped. Stop it with: systemctl disable --now crawler-autodeploy.timer
# Contract: docs/crawler/operations.md#automatic-deploys
set -euo pipefail

ROOT="${CRAWLER_ROOT:-/opt/crawler}"
REPOSITORY="${CRAWLER_RELEASE_REPOSITORY:-aura-historia/backend}"
API_URL="${CRAWLER_RELEASES_API_URL:-https://api.github.com/repos/${REPOSITORY}/releases?per_page=30}"
COMPARE_URL="${CRAWLER_COMPARE_API_URL:-https://api.github.com/repos/${REPOSITORY}/compare}"
SEEN_FILE="${ROOT}/autodeploy.handled"
DEPLOY="$(dirname -- "$(readlink -f -- "${BASH_SOURCE[0]}")")/deploy.sh"

# The API lists releases newest first; only releases of this workflow count.
# A failed request fails the unit (it shows in systemctl --failed) and is retried at the next run.
releases="$(curl -fsSL --retry 3 --max-time 30 "$API_URL")" \
  || { echo "autodeploy: could not list the releases; retrying at the next run" >&2; exit 1; }
latest="$(grep -o '"tag_name": *"crawler-[a-f0-9]\{40\}"' <<< "$releases" | head -n1 | grep -o '[a-f0-9]\{40\}' || true)"
[[ -n "$latest" ]] || { echo "autodeploy: no crawler release found"; exit 0; }

running=""
[[ ! -f "${ROOT}/bin/SOURCE_SHA" ]] || running="$(cat "${ROOT}/bin/SOURCE_SHA")"
handled=""
[[ ! -f "$SEEN_FILE" ]] || handled="$(cat "$SEEN_FILE")"
if [[ "$latest" == "$running" || "$latest" == "$handled" ]]; then
  exit 0
fi

# The host has no Git, so GitHub says whether the release's commit comes after the running one.
# A legacy build carries no SOURCE_SHA and is always replaced.
if [[ -n "$running" ]]; then
  comparison="$(curl -fsSL --retry 3 --max-time 30 "${COMPARE_URL}/${running}...${latest}?per_page=1")" \
    || { echo "autodeploy: could not compare ${latest} with the running ${running}; retrying at the next run" >&2; exit 1; }
  ahead="$(grep -o '"ahead_by": *[0-9]*' <<< "$comparison" | head -n1 | grep -o '[0-9]*$' || true)"
  behind="$(grep -o '"behind_by": *[0-9]*' <<< "$comparison" | head -n1 | grep -o '[0-9]*$' || true)"
  [[ -n "$ahead" && -n "$behind" ]] \
    || { echo "autodeploy: unreadable comparison of ${latest} with ${running}; retrying at the next run" >&2; exit 1; }
  if ((behind > 0 || ahead == 0)); then
    printf '%s\n' "$latest" > "$SEEN_FILE"
    echo "autodeploy: ${latest} does not come after the running ${running}; not downgrading (deploy it by hand)"
    exit 0
  fi
fi

echo "autodeploy: deploying ${latest}"
status=0
"$DEPLOY" --unattended "$latest" || status=$?
case "$status" in
  0) printf '%s\n' "$latest" > "$SEEN_FILE" ;;
  3|4)
    # Decided about (needs an operator, or unhealthy and rolled back), not to be retried every
    # tick: the unit stays failed until an operator acts. Exit 1 (a lock conflict) is retried.
    printf '%s\n' "$latest" > "$SEEN_FILE"
    echo "autodeploy: ${latest} needs an operator (exit ${status}); see the journal" >&2
    exit 1 ;;
  *) echo "autodeploy: ${latest} not deployed (exit ${status}); retrying at the next run" >&2; exit 1 ;;
esac
